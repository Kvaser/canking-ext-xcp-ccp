#!/usr/bin/env node
// Compare a freshly built config against the one the GUI previously built.
//
//   node config-golden.mjs <rebuilt.json> <recorded.json>
//
// The recorded config is the extension's real input from a real measurement. 
// This pins the whole A2L -> config path against a known-good output rather than against the generator's own idea of one.
// It is also what lets a recorded case stay A2L-driven since if the rebuild matches, the log's values should match
//
// Comparison is done with keys sorted.
import { readFileSync } from "node:fs";

const [rebuiltPath, recordedPath] = process.argv.slice(2);
if (!rebuiltPath || !recordedPath) {
  console.error("usage: config-golden.mjs <rebuilt.json> <recorded.json>");
  process.exit(1);
}

const sortKeys = (v) =>
  Array.isArray(v)
    ? v.map(sortKeys)
    : v && typeof v === "object"
      ? Object.fromEntries(
          Object.keys(v)
            .sort()
            .map((k) => [k, sortKeys(v[k])]),
        )
      : v;

const load = (p) => JSON.stringify(sortKeys(JSON.parse(readFileSync(p, "utf8"))), null, 1).split("\n");

const rebuilt = load(rebuiltPath);
const recorded = load(recordedPath);

if (rebuilt.join("\n") === recorded.join("\n")) {
  console.log(`config matches the recorded one (${recorded.length} lines)`);
  process.exit(0);
}

console.log("rebuilt config differs from the config recorded with the log");
let shown = 0;
for (let i = 0; i < Math.max(rebuilt.length, recorded.length) && shown < 20; i++) {
  if (rebuilt[i] === recorded[i]) continue;
  console.log(`  recorded: ${recorded[i] ?? "<end>"}`);
  console.log(`  rebuilt : ${rebuilt[i] ?? "<end>"}`);
  shown++;
}
console.log("a difference here is either an A2L -> config regression or an intentional");
console.log("change; if intentional, re-record the log or update the committed config.");
process.exit(1);
