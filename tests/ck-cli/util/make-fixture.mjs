#!/usr/bin/env node
// Generate a replayable CAN log of DAQ DTO frames, plus the signal values that decoding it must produce.
//
//   node make-fixture.mjs <dir>/<name>.config.json
//
// Writes <dir>/<name>.asc and <dir>/<name>.expected.json: two rounds of every ODT with PID = daq_list.first_pid + odt_index.
import { writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { renderAsc, hex } from "./asc.mjs";
import { loadConfig, odtPayload } from "./frame-values.mjs";
import { fail } from "./cli.mjs";

const ROUNDS = 2;

const [configPath] = process.argv.slice(2);
if (!configPath) fail("usage: make-fixture.mjs <dir>/<name>.config.json");
const outDir = dirname(configPath);
const name = basename(configPath, ".config.json");

const layout = loadConfig(configPath);
const { cfg, dtoId, tsSize } = layout;
if (dtoId > 0x7ff) fail(`DTO id 0x${dtoId.toString(16)} is extended; the .asc writer here only emits standard ids`);

// ---- build frames -----------------------------------------------------------

const frames = [];
const indexOf = new Map();
const index = (sigName) => {
  if (!indexOf.has(sigName)) indexOf.set(sigName, indexOf.size);
  return indexOf.get(sigName);
};

for (let round = 0; round < ROUNDS; round++) {
  for (const dl of cfg.daq_lists) {
    dl.odts.forEach((_, odtIndex) => {
      const pid = dl.first_pid + odtIndex;
      const { data, signals } = odtPayload(layout, dl, odtIndex, pid, round, index);
      if (data.length > 8) {
        fail(
          `PID ${pid} needs ${data.length} bytes (1 PID + ${odtIndex === 0 ? tsSize : 0} timestamp + entries); ` +
            `the .asc writer here only emits classic CAN frames up to 8 bytes`,
        );
      }
      frames.push({ pid, daqList: dl.id, odtIndex, data, expected: signals });
    });
  }
}

if (!frames.length) fail("config produced no DAQ frames");

// ---- emit -------------------------------------------------------------------

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
