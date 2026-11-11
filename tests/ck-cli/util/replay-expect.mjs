#!/usr/bin/env node
// Turn the frames `ck analysis` read out of a recorded log into the replay expectation checks can assert against.
//
//   node replay-expect.mjs <config.json> <frames.json> --out FILE
//
// Generated cases get this file from util/make-fixture.mjs, which knows what it put in the fixture. A recorded log has to be read instead
//
// Only the shape checks/40-live.sh needs is emitted: the DTO id, the log's internal channel index for the logreplay channel map, and the Rx payloads on that id in log order.
// Decoded values are not included; recorded cases assert those with assert/decode-log.mjs.
import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { hex } from "./asc.mjs";
import { loadConfig } from "./frame-values.mjs";
import { fail } from "./cli.mjs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: { out: { type: "string" } },
});
const [cfgPath, framesPath] = positionals;
const outPath = values.out ?? null;
if (!cfgPath || !framesPath || !outPath) {
  console.error("usage: replay-expect.mjs <config.json> <frames.json> --out FILE");
  process.exit(1);
}

const { dtoId } = loadConfig(cfgPath);
const frames = JSON.parse(readFileSync(framesPath, "utf8"));

const rx = frames.filter((f) => f.id === dtoId && !f.tx);
if (!rx.length) fail(`the log has no Rx frames on DTO id 0x${dtoId.toString(16)} to replay`);

// The logreplay channel map is keyed by the log file's internal channel index, which is what the service reports as each frame's source.
const sources = [...new Set(rx.map((f) => String(f.sourceId)))];
if (sources.length !== 1) {
  fail(`DTO frames arrive on ${sources.length} log channels [${sources}]; expected exactly one`);
}

const expected = {
  dtoId,
  logChannelKey: sources[0],
  frames: rx.map((f) => ({ pid: f.data[0], data: hex(f.data) })),
};

writeFileSync(outPath, JSON.stringify(expected, null, 2));
console.log(
  `replay expectation: ${expected.frames.length} Rx payload(s) on id 0x${dtoId.toString(16)}, ` +
    `log channel ${expected.logChannelKey}`,
);
