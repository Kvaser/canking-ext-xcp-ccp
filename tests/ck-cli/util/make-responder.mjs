#!/usr/bin/env node
// Write a .asc log that answers a DAQ-size command with a first PID of our choosing, so
// checks/45-live-pid-remap.sh can test the decoder re-key without a real ECU.
//
//   node make-responder.mjs --out FILE --dto-id N --counter N --odts N --first-pid N
//     [--config CONFIG.JSON --daq-id N --expected-out EXPECTED.JSON]
//
// The first frame is a CCP positive response: ff, error 00, the echoed command counter,
// then the DAQ size and first PID the slave claims.
//
// With --config/--daq-id/--expected-out also given, a second frame follows: a synthetic DAQ
// payload for that list's ODT 0, PID forced to --first-pid, with known signal values. 
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { renderAsc, hex } from "./asc.mjs";
import { rawFor, encode, physical, checkEncodable } from "./frame-values.mjs";
import { fail } from "./cli.mjs";

const usage =
  "usage: make-responder.mjs --out FILE --dto-id N --counter N --odts N --first-pid N " +
  "[--config CONFIG.JSON --daq-id N --expected-out EXPECTED.JSON]";

const { values } = parseArgs({
  args: process.argv.slice(2),
  options: {
    out: { type: "string" },
    "dto-id": { type: "string" },
    counter: { type: "string" },
    odts: { type: "string" },
    "first-pid": { type: "string" },
    config: { type: "string" },
    "daq-id": { type: "string" },
    "expected-out": { type: "string" },
  },
});
const num = (flag) => {
  if (values[flag] === undefined) {
    console.error(`error: missing --${flag}\n${usage}`);
    process.exit(1);
  }
  const v = Number(values[flag]);
  if (!Number.isInteger(v)) {
    console.error(`error: --${flag} needs an integer, got ${JSON.stringify(values[flag])}`);
    process.exit(1);
  }
  return v;
};
const outPath = values.out;
if (!outPath) {
  console.error(usage);
  process.exit(1);
}

const dtoId = num("dto-id");
const counter = num("counter");
const odts = num("odts");
const firstPid = num("first-pid");

const frames = [{ time: 0.05, id: dtoId, data: [0xff, 0x00, counter, odts, firstPid, 0x00, 0x00, 0x00] }];

const configPath = values.config;
const daqIdArg = values["daq-id"];
const expectedOut = values["expected-out"];
const given = [configPath, daqIdArg, expectedOut];
if (given.some(Boolean) && !given.every(Boolean)) {
  console.error("error: --config, --daq-id, and --expected-out must be given together");
  process.exit(1);
}

if (configPath) {
  const daqId = Number(daqIdArg);
  const cfg = JSON.parse(readFileSync(configPath, "utf8"));
  const dl = cfg.daq_lists.find((l) => l.id === daqId);
  if (!dl) fail(`config has no DAQ list with id ${daqId}`);

  const proj = cfg.project;
  const sigByName = new Map(proj.signals.map((s) => [s.name, s]));
  const cmByName = new Map((proj.compu_methods ?? []).map((c) => [c.name, c]));
  const defaultOrder = proj.default_byte_order ?? "MsbLast";
  const isXcp = String(cfg.protocol).toLowerCase() === "xcp";
  const tsSize = isXcp ? (proj.xcp_data?.daq_info?.timestamp_size ?? 0) : 0;

  const odt = dl.odts[0];
  const parts = [Buffer.from([firstPid])];
  const signals = {};

  if (tsSize > 0) {
    const ts = 1000 * (dl.id + 1);
    parts.push(encode(ts, tsSize, defaultOrder === "MsbLast", false));
    signals[`Timestamp_${dl.first_pid}`] = ts;
  }

  let i = 0;
  for (const entry of odt.entries) {
    for (const sigName of entry.names) {
      const sig = sigByName.get(sigName);
      if (!sig) fail(`DAQ list ${daqId} references signal '${sigName}' absent from project.signals`);
      try {
        checkEncodable(sig);
        const raw = rawFor(i++, 0);
        const order = sig.byte_order ?? defaultOrder;
        parts.push(encode(raw, entry.size, order === "MsbLast", sig.is_signed));
        signals[sigName] = physical(sig, raw, cmByName);
      } catch (e) {
        fail(e.message);
      }
    }
  }

  const data = Buffer.concat(parts);
  frames.push({ time: 0.06, id: dtoId, data: [...data] });
  writeFileSync(
    expectedOut,
    JSON.stringify({ dtoId, frames: [{ pid: firstPid, data: hex(data), signals }] }, null, 2),
  );
  console.log(`  + DAQ list ${daqId} ODT 0 at PID 0x${firstPid.toString(16)}: [${hex(data)}] -> ${JSON.stringify(signals)}`);
}

writeFileSync(outPath, renderAsc(frames));
console.log(`responder: first PID 0x${firstPid.toString(16)} answering counter ${counter}`);
