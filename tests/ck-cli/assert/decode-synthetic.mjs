#!/usr/bin/env node
// Compare `ck analysis frames-between --json` output against the values util/make-fixture.mjs said it mus decode to
//
//   node decode-synthetic.mjs <expected.json> <frames.json>
//
// Both sides derive the frame layout from the same config, so this checks the decode pipeline end to end.
//
// Exits 0 when every expected frame was found and decoded correctly, 1 otherwise.
import { readFileSync } from "node:fs";
import { close } from "../util/frame-values.mjs";

const [expPath, framesPath] = process.argv.slice(2);
if (!expPath || !framesPath) {
  console.error("usage: decode-synthetic.mjs <expected.json> <frames.json>");
  process.exit(1);
}

const expected = JSON.parse(readFileSync(expPath, "utf8"));
const frames = JSON.parse(readFileSync(framesPath, "utf8"));

// Uppercase for hex
const hexOf = (f) => f.data.map((b) => b.toString(16).padStart(2, "0").toUpperCase()).join(" ");

// Rx frames on the DTO id, in log order.
const candidates = frames.filter((f) => f.id === expected.dtoId && f.tx === false);

let cursor = 0;
let failures = 0;
let checked = 0;

for (const want of expected.frames) {
  let found = -1;
  for (let i = cursor; i < candidates.length; i++) {
    if (hexOf(candidates[i]) === want.data) {
      found = i;
      break;
    }
  }
  if (found < 0) {
    console.log(`PID ${want.pid}: no Rx frame [${want.data}] on id 0x${expected.dtoId.toString(16)}`);
    failures++;
    continue;
  }
  cursor = found + 1;
  const frame = candidates[found];
  const got = new Map(frame.signalValues.map((s) => [s.name, s.doubleValue]));

  if (!frame.signalValues.length) {
    console.log(
      `PID ${want.pid}: frame [${want.data}] decoded to no signals at all ` +
        `(messageName=${JSON.stringify(frame.messageName)}), usually a frame shorter than ` +
        `1 + timestamp + entries`,
    );
    failures++;
    continue;
  }

  for (const [sig, wantVal] of Object.entries(want.signals)) {
    checked++;
    if (!got.has(sig)) {
      console.log(`PID ${want.pid}: signal '${sig}' missing from decoded output`);
      failures++;
    } else if (!close(got.get(sig), wantVal)) {
      console.log(`PID ${want.pid}: '${sig}' = ${got.get(sig)}, expected ${wantVal}`);
      failures++;
    }
  }

  // The PID multiplexor should agree with the frame we sent.
  const pid = got.get("PID");
  if (pid !== undefined && pid !== want.pid) {
    console.log(`PID signal reports ${pid} for frame [${want.data}], expected ${want.pid}`);
    failures++;
  }
}

if (failures) {
  console.log(`${failures} assertion(s) failed (${checked} signal value(s) checked)`);
  process.exit(1);
}
console.log(`${checked} signal value(s) across ${expected.frames.length} frame(s) decoded correctly`);
