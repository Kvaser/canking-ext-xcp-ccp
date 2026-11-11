#!/usr/bin/env node
// Compare a freshly built config against the one the GUI previously built.
//
//   node config-golden.mjs <rebuilt.json> <recorded.json>
//
// The recorded config is the extension's real input from a real measurement.
// This pins the whole A2L -> config path against a known-good output rather than against the generator's own idea of one.
// It is also what lets a recorded case stay A2L-driven since if the rebuild matches, the log's values should match
//
// Key order does not matter.
import { readFileSync } from "node:fs";
import { deepStrictEqual } from "node:assert";

const [rebuiltPath, recordedPath] = process.argv.slice(2);
if (!rebuiltPath || !recordedPath) {
  console.error("usage: config-golden.mjs <rebuilt.json> <recorded.json>");
  process.exit(1);
}

const rebuilt = JSON.parse(readFileSync(rebuiltPath, "utf8"));
const recorded = JSON.parse(readFileSync(recordedPath, "utf8"));

try {
  deepStrictEqual(rebuilt, recorded);
} catch (e) {
  console.log("rebuilt config differs from the config recorded with the log (+ rebuilt, - recorded):");
  console.log(e.message);
  console.log("a difference here is either an A2L -> config regression or an intentional");
  console.log("if intentional, re-record the log or update the committed config.");
  process.exit(1);
}
console.log("config matches the recorded one");
