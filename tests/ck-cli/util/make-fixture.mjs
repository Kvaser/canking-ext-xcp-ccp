#!/usr/bin/env node
// Generate a replayable CAN log of DAQ DTO frames, plus the signal values that decoding it must produce.
//
//   node make-fixture.mjs <config.json> --out-dir DIR [--name NAME] [--rounds N]
//
// Writes <name>.asc and <name>.expected.json.
//
// Frame layout is derived from the config.
//
//   byte 0     PID  = daq_list.first_pid + odt_index
//   bytes 1..N timestamp, N = daq_info.timestamp_size   (first ODT only)
//   remaining  ODT entries in order, each `size` bytes
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join, basename } from "node:path";
import { parseArgs } from "node:util";
import { renderAsc, hex } from "./asc.mjs";
import { rawFor, encode as encodeValue, physical as physicalValue, checkEncodable as checkSigEncodable } from "./frame-values.mjs";
import { fail } from "./cli.mjs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: {
    "out-dir": { type: "string", default: "." },
    name: { type: "string" },
    rounds: { type: "string", default: "2" },
  },
});
const [configPath] = positionals;
if (!configPath) {
  console.error("usage: make-fixture.mjs <config.json> --out-dir DIR [--name NAME] [--rounds N]");
  process.exit(1);
}
const outDir = values["out-dir"];
const name = values.name ?? basename(configPath).replace(/\.config\.json$|\.json$/, "");
const rounds = Number(values.rounds);

const cfg = JSON.parse(readFileSync(configPath, "utf8"));
const proj = cfg.project;
const sigByName = new Map(proj.signals.map((s) => [s.name, s]));
const cmByName = new Map((proj.compu_methods ?? []).map((c) => [c.name, c]));
const defaultOrder = proj.default_byte_order ?? "MsbLast";

const isXcp = String(cfg.protocol).toLowerCase() === "xcp";
const dtoId = isXcp ? proj.xcp_data?.xcp_info?.can_id_slave : proj.ccp_data?.info?.dto_id;
if (dtoId === undefined || dtoId === null) fail("config has no DTO CAN id");
if (dtoId > 0x7ff) fail(`DTO id 0x${dtoId.toString(16)} is extended; the .asc writer here only emits standard ids`);

const tsSize = isXcp ? (proj.xcp_data?.daq_info?.timestamp_size ?? 0) : 0;

// ---- value encoding ---------------------------------------------------------

const encode = (...a) => {
  try {
    return encodeValue(...a);
  } catch (e) {
    return fail(e.message);
  }
};

const physical = (sig, raw) => {
  try {
    return physicalValue(sig, raw, cmByName);
  } catch (e) {
    return fail(e.message);
  }
};

const checkEncodable = (sig) => {
  try {
    checkSigEncodable(sig);
  } catch (e) {
    fail(e.message);
  }
};

// ---- build frames -----------------------------------------------------------

const frames = [];
let sigIndex = 0;
const indexOf = new Map();

for (let round = 0; round < rounds; round++) {
  for (const dl of cfg.daq_lists) {
    dl.odts.forEach((odt, odtIndex) => {
      const pid = dl.first_pid + odtIndex;
      const parts = [Buffer.from([pid])];
      const expected = {};

      if (odtIndex === 0 && tsSize > 0) {
        const ts = 1000 * (dl.id + 1) + round;
        // The timestamp rides in the config's byte order, same as entry data.
        parts.push(encode(ts, tsSize, defaultOrder === "MsbLast", false));
        expected[`Timestamp_${dl.first_pid}`] = ts;
      }

      for (const entry of odt.entries) {
        for (const sigName of entry.names) {
          const sig = sigByName.get(sigName);
          if (!sig) fail(`DAQ list references signal '${sigName}' absent from project.signals`);
          checkEncodable(sig);
          if (!indexOf.has(sigName)) indexOf.set(sigName, sigIndex++);
          const raw = rawFor(indexOf.get(sigName), round);
          const order = sig.byte_order ?? defaultOrder;
          if (order !== "MsbLast" && order !== "MsbFirst") fail(`unknown byte_order '${order}'`);
          parts.push(encode(raw, entry.size, order === "MsbLast", sig.is_signed));
          expected[sigName] = physical(sig, raw);
        }
      }

      const data = Buffer.concat(parts);
      if (data.length > 8) {
        fail(
          `PID ${pid} needs ${data.length} bytes (1 PID + ${odtIndex === 0 ? tsSize : 0} timestamp + entries); ` +
            `the .asc writer here only emits classic CAN frames up to 8 bytes`,
        );
      }
      frames.push({ pid, daqList: dl.id, odtIndex, data, expected });
    });
  }
}

if (!frames.length) fail("config produced no DAQ frames");

// ---- emit -------------------------------------------------------------------

mkdirSync(outDir, { recursive: true });
const ascPath = join(outDir, `${name}.asc`);
const expPath = join(outDir, `${name}.expected.json`);
writeFileSync(
  ascPath,
  renderAsc(frames.map((f, i) => ({ time: 0.01 * (i + 1), id: dtoId, data: [...f.data] }))),
);
writeFileSync(
  expPath,
  JSON.stringify(
    {
      dtoId,
      logChannelKey: "0",
      frames: frames.map((f) => ({
        pid: f.pid,
        daqList: f.daqList,
        odtIndex: f.odtIndex,
        data: hex(f.data),
        signals: f.expected,
      })),
    },
    null,
    2,
  ),
);

console.log(`fixture ${name}: ${frames.length} frames on DTO id 0x${dtoId.toString(16)}, timestamp_size=${tsSize}`);
for (const fr of frames) {
  console.log(`  PID ${String(fr.pid).padStart(3)}  [${hex(fr.data)}]  -> ${JSON.stringify(fr.expected)}`);
}
console.log(`  wrote ${ascPath}`);
console.log(`        ${expPath}`);
