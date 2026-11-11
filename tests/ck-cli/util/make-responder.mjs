#!/usr/bin/env node
// Write a .asc log that answers a DAQ-size command with a first PID of our choosing.
//
//   node make-responder.mjs --config CONFIG.JSON --daq-id N --counter N --first-pid N --out FILE --expected-out EXPECTED.JSON
//
// The first frame is a CCP positive response: ff, error 00, the echoed command counter, then the list's DAQ size (its ODT count) and the first PID the slave claims.
// A second frame follows: the list's ODT 0 at that PID, with known signal values, which go to --expected-out.
import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { renderAsc, hex } from "./asc.mjs";
import { loadConfig, odtPayload } from "./frame-values.mjs";
import { fail } from "./cli.mjs";

const flags = ["config", "daq-id", "counter", "first-pid", "out", "expected-out"];
const { values } = parseArgs({
  args: process.argv.slice(2),
  options: Object.fromEntries(flags.map((f) => [f, { type: "string" }])),
});
const missing = flags.filter((f) => values[f] === undefined);
if (missing.length) fail(`missing --${missing.join(", --")}`);
const [daqId, counter, firstPid] = ["daq-id", "counter", "first-pid"].map((f) => {
  const v = Number(values[f]);
  return Number.isInteger(v) ? v : fail(`--${f} needs an integer, got ${JSON.stringify(values[f])}`);
});

const layout = loadConfig(values.config);
const { dtoId } = layout;
const dl = layout.cfg.daq_lists.find((l) => l.id === daqId);
if (!dl) fail(`config has no DAQ list with id ${daqId}`);

let i = 0;
const { data, signals } = odtPayload(layout, dl, 0, firstPid, 0, () => i++);

writeFileSync(
  values.out,
  renderAsc([
    { time: 0.05, id: dtoId, data: [0xff, 0x00, counter, dl.odts.length, firstPid, 0x00, 0x00, 0x00] },
    { time: 0.06, id: dtoId, data: [...data] },
  ]),
);
writeFileSync(
  values["expected-out"],
  JSON.stringify({ dtoId, frames: [{ pid: firstPid, data: hex(data), signals }] }, null, 2),
);
console.log(`  + DAQ list ${daqId} ODT 0 at PID 0x${firstPid.toString(16)}: [${hex(data)}] -> ${JSON.stringify(signals)}`);
console.log(`responder: first PID 0x${firstPid.toString(16)} answering counter ${counter}`);
