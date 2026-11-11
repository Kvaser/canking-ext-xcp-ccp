#!/usr/bin/env node
// Assert that what CanKing stored is the config that was applied.
//
//   node apply.mjs <sent.config.json> <ck-get-nodes-output.json> <extName> <nodeId>
//
// Compares parsed JSON
import { readFileSync } from "node:fs";
import { deepStrictEqual } from "node:assert";

const [sentPath, gotPath, extName, nodeId] = process.argv.slice(2);
const sent = JSON.parse(readFileSync(sentPath, "utf8"));
const node = JSON.parse(readFileSync(gotPath, "utf8"));

let failures = 0;
const check = (ok, msg) => {
  if (!ok) {
    console.log(msg);
    failures++;
  }
};

check(node.baseNode?.identifier?.toLowerCase() === nodeId.toLowerCase(),
  `node identifier is ${node.baseNode?.identifier}, expected ${nodeId}`);
check(node.extensionSettings?.extensionName === extName,
  `extensionName is ${JSON.stringify(node.extensionSettings?.extensionName)}, expected ${JSON.stringify(extName)}`);

const raw = node.extensionSettings?.configuration;
if (typeof raw !== "string" || raw.length === 0) {
  console.log("stored configuration is empty or not a string");
  process.exit(1);
}

let got;
try {
  got = JSON.parse(raw);
} catch (e) {
  console.log(`stored configuration is not valid JSON: ${e.message}`);
  process.exit(1);
}

try {
  deepStrictEqual(got, sent);
} catch (e) {
  console.log(`stored configuration differs from the one sent (+ stored, - sent):\n${e.message}`);
  failures++;
}

if (failures) process.exit(1);
const nsig = got.project?.signals?.length ?? 0;
console.log(`config identical after round-trip (${raw.length} B, ${nsig} signal(s), ${got.daq_lists?.length ?? 0} DAQ list(s))`);
