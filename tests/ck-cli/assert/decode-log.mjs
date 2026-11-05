#!/usr/bin/env node
// Assert the decoding of a recorded log extracter from a memorator
// 
// Here the bytes come from the slave and the expected values are computed independently of the extension.
//
//   node decode-log.mjs <config.json> <frames.json> <case.json> [--no-pin] [--min-daq-frames N]
//
// Frames on the DTO id that are not DAQ frames must contribute no fresh values.
//
// --no-pin skips the case's recorded.expect values. Use it for a freshly captured live log
//
// --min-daq-frames overrides recorded.minDaqFrames, e.g. when a short live
// settle window can't reasonably reach the count a full recorded log has.

// Exits 0 when every layer holds, 1 otherwise.
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { close } from "../util/frame-values.mjs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: { "no-pin": { type: "boolean", default: false }, "min-daq-frames": { type: "string" } },
});
const noPin = values["no-pin"];
const minFramesOverride = values["min-daq-frames"] === undefined ? undefined : Number(values["min-daq-frames"]);

const [cfgPath, framesPath, casePath] = positionals;
if (!cfgPath || !framesPath || !casePath) {
  console.error("usage: decode-log.mjs <config.json> <frames.json> <case.json> [--no-pin] [--min-daq-frames N]");
  process.exit(1);
}

// A config-shape problem, not a decode mismatch. Report and bail immediately
const fail = (m) => {
  console.log(m);
  process.exit(1);
};

const cfg = JSON.parse(readFileSync(cfgPath, "utf8"));
const frames = JSON.parse(readFileSync(framesPath, "utf8"));
const recorded = JSON.parse(readFileSync(casePath, "utf8")).recorded ?? {};

const proj = cfg.project;
const sigByName = new Map(proj.signals.map((s) => [s.name, s]));
const cmByName = new Map((proj.compu_methods ?? []).map((c) => [c.name, c]));
const defaultOrder = proj.default_byte_order ?? "MsbLast";
const isXcp = String(cfg.protocol).toLowerCase() === "xcp";
const dtoId = isXcp ? proj.xcp_data?.xcp_info?.can_id_slave : proj.ccp_data?.info?.dto_id;
if (dtoId === undefined || dtoId === null) fail("config has no DTO CAN id");

// Timestamps are an XCP DAQ property and ride in the first ODT of a list only.
const tsSize = isXcp ? (proj.xcp_data?.daq_info?.timestamp_size ?? 0) : 0;

// ---- the layout this file expects, derived from the config -------------------
//
// PID -> the signals that frame carries, in payload order:
//   byte 0     PID = daq_list.first_pid + odt_index
//   bytes 1..N timestamp, N = daq_info.timestamp_size   (first ODT only)
//   remaining ODT entries in order, each `size` bytes; a signal sits at its address offset inside its entry
//             (an entry may start before it, padded to the ECU's granularity, or hold signals that overlap)

const layoutByPid = new Map();

for (const dl of cfg.daq_lists) {
  dl.odts.forEach((odt, odtIndex) => {
    const pid = (dl.first_pid + odtIndex) & 0xff;
    if (layoutByPid.has(pid)) {
      fail(`two ODTs both claim PID ${pid}; this assert cannot attribute its frames`);
    }
    const slots = [];
    let cursor = 1 + (odtIndex === 0 ? tsSize : 0);

    if (odtIndex === 0 && tsSize > 0) {
      slots.push({ name: `Timestamp_${pid}`, start: 1, size: tsSize, raw: true });
    }

    for (const entry of odt.entries) {
      const entryStart = cursor;
      for (const name of entry.names) {
        const sig = sigByName.get(name);
        if (!sig) fail(`DAQ list references signal '${name}' absent from project.signals`);
        const at = sig.ecu_address - entry.ecu_address;
        if (at >= 0 && at + sig.size <= entry.size) cursor = entryStart + at;
        if (sig.bit_mask !== null && sig.bit_mask !== undefined) {
          const full = (1n << BigInt(8 * sig.size)) - 1n;
          if (BigInt(sig.bit_mask) !== full) {
            fail(
              `signal '${name}' has a partial bit_mask 0x${BigInt(sig.bit_mask).toString(16)} ` +
                `over ${sig.size} byte(s); neither the decoder nor this assert models one`,
            );
          }
        }
        const cm = sig.conversion ? cmByName.get(sig.conversion.identifier) : undefined;
        if (cm?.formula) fail(`signal '${name}' uses a COMPU_METHOD formula; only factor/offset here`);
        const order = sig.byte_order ?? defaultOrder;
        if (order !== "MsbLast" && order !== "MsbFirst") fail(`unknown byte_order '${order}'`);

        slots.push({
          name,
          start: cursor,
          size: sig.size,
          bigEndian: order === "MsbFirst",
          isSigned: !!sig.is_signed,
          isFloat: !!sig.is_float,
          factor: cm?.factor ?? 1,
          offset: cm?.offset ?? 0,
        });
        cursor += sig.size;
      }
      cursor = entryStart + entry.size;
    }
    layoutByPid.set(pid, slots);
  });
}

if (!layoutByPid.size) fail("config declares no DAQ lists, so no frame can be attributed");

// ---- value extraction --------------------------------------------------------

function physicalValue(bytes, slot) {
  // Normalise to little-endian so one set of readers covers both byte orders.
  const buf = Buffer.from(slot.bigEndian ? [...bytes].reverse() : bytes);
  if (slot.raw) return buf.readUIntLE(0, buf.length);
  let raw;
  if (slot.isFloat) {
    if (buf.length === 4) raw = buf.readFloatLE(0);
    else if (buf.length === 8) raw = buf.readDoubleLE(0);
    else fail(`signal '${slot.name}' is a ${buf.length}-byte float; only 4 and 8 are IEEE`);
  } else if (slot.isSigned) {
    raw = buf.length <= 6 ? buf.readIntLE(0, buf.length) : Number(buf.readBigInt64LE(0));
  } else {
    raw = buf.length <= 6 ? buf.readUIntLE(0, buf.length) : Number(buf.readBigUInt64LE(0));
  }
  return raw * slot.factor + slot.offset;
}

// ---- walk the log ------------------------------------------------------------

const latched = new Map(); // signal -> the value it last decoded to
const firstByPid = new Map(); // PID -> the reported values of its first frame
let lastDaqPid = null;
let daqFrames = 0;
let nonDaqFrames = 0;
let valuesChecked = 0;
let latchedChecked = 0;
let failures = 0;

const report = (m) => {
  console.log(m);
  failures++;
};

for (const frame of frames) {
  if (frame.id !== dtoId || frame.tx) continue;
  if (!frame.data.length) continue;

  const pid = frame.data[0];
  const reported = new Map(frame.signalValues.map((s) => [s.name, s.doubleValue]));
  const slots = layoutByPid.get(pid);

  if (!firstByPid.has(pid)) firstByPid.set(pid, reported);

  if (!slots) {
    // Not a DAQ frame: a command response, or a PID this config does not claim.
    // It may only carry the latched snapshot forward.
    nonDaqFrames++;
    for (const [name, value] of reported) {
      if (name === "PID") {
        if (lastDaqPid !== null && value !== lastDaqPid) {
          report(`non-DAQ frame [${pid}]: PID reports ${value}, last DAQ PID was ${lastDaqPid}`);
        }
        continue;
      }
      latchedChecked++;
      if (!latched.has(name)) {
        report(`non-DAQ frame [${pid}]: '${name}' reported but never decoded from a DAQ frame`);
      } else if (!close(latched.get(name), value)) {
        report(
          `non-DAQ frame [${pid}]: '${name}' = ${value}, but it last decoded to ${latched.get(name)}`,
        );
      }
    }
    continue;
  }

  daqFrames++;
  const fresh = new Set();

  for (const slot of slots) {
    const end = slot.start + slot.size;
    if (end > frame.data.length) break;
    fresh.add(slot.name);
    valuesChecked++;
    const want = physicalValue(frame.data.slice(slot.start, end), slot);
    if (!reported.has(slot.name)) {
      report(`PID ${pid}: '${slot.name}' missing from the decoded output`);
    } else if (!close(reported.get(slot.name), want)) {
      report(`PID ${pid}: '${slot.name}' = ${reported.get(slot.name)}, expected ${want}`);
    }
    latched.set(slot.name, want);
  }

  if (!fresh.size) {
    report(`PID ${pid}: frame [${frame.data.join(" ")}] carries no complete entry`);
  }

  // Anything else on this frame is the replayed snapshot.
  for (const [name, value] of reported) {
    if (name === "PID" || fresh.has(name)) continue;
    latchedChecked++;
    if (!latched.has(name)) {
      report(`PID ${pid}: '${name}' reported but never decoded from a DAQ frame`);
    } else if (!close(latched.get(name), value)) {
      report(`PID ${pid}: latched '${name}' = ${value}, but it last decoded to ${latched.get(name)}`);
    }
  }

  if (reported.get("PID") !== pid) {
    report(`PID ${pid}: the PID multiplexor reports ${reported.get("PID")}`);
  }
  lastDaqPid = pid;
}

// ---- pinned expectations -----------------------------------------------------
//
// Checked against the first frame seen for each PID.

let pinned = 0;
for (const [pidText, wanted] of Object.entries(noPin ? {} : recorded.expect ?? {})) {
  const pid = Number(pidText);
  const reported = firstByPid.get(pid);
  if (!reported) {
    report(`no frame with PID ${pid} in the log, but the case pins values for it`);
    continue;
  }
  for (const [name, want] of Object.entries(wanted)) {
    pinned++;
    if (!reported.has(name)) {
      report(`pinned PID ${pid}: '${name}' missing from the first such frame`);
    } else if (!close(reported.get(name), want)) {
      report(`pinned PID ${pid}: '${name}' = ${reported.get(name)}, pinned as ${want}`);
    }
  }
}

// ---- guards ------------------------------------------------------------------
//
// Without these a log that produced nothing at all would report zero failures.

const minDaqFrames = minFramesOverride ?? recorded.minDaqFrames ?? 1;
if (daqFrames < minDaqFrames) {
  report(`only ${daqFrames} DAQ frame(s) decoded on id ${dtoId}, expected at least ${minDaqFrames}`);
}
for (const slots of layoutByPid.values()) {
  for (const slot of slots) {
    if (!latched.has(slot.name)) report(`'${slot.name}' never decoded anywhere in the log`);
  }
}
if (!pinned && !noPin) report("the case pins no expected values, so nothing anchors the layout");

// ---- report ------------------------------------------------------------------

const summary =
  `${daqFrames} DAQ frame(s) on id ${dtoId}, ${valuesChecked} value(s) recomputed, ` +
  `${pinned} pinned, ${latchedChecked} latched, ${nonDaqFrames} non-DAQ frame(s)`;

if (failures) {
  console.log(`${failures} assertion(s) failed (${summary})`);
  if (daqFrames === 0 && latchedChecked > 0) {
    // The extension reported values on frames whose PID no configured ODT claims, which is what re-keying looks like from here.
    const pids = [...layoutByPid.keys()].map((p) => `0x${p.toString(16)}`).join(", ");
    console.log(
      `no frame carried a configured PID (${pids}) yet the extension still decoded values:\n` +
        `the log's command responses re-keyed the decoder, so the case's config\n` +
        `and its log disagree about first_pid. Re-record the pair, or fix the case.`,
    );
  }
  process.exit(1);
}
console.log(`${summary}: all correct`);
