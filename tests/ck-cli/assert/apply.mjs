#!/usr/bin/env node
// Assert that what CanKing stored is the config that was applied.
//
//   node apply.mjs <sent.config.json> <ck-get-nodes-output.json> <extName> <nodeId>
//
// Compares parsed JSON
import { readFileSync } from "node:fs";

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

// Deep compare, reporting the first differing paths
const diffs = [];
(function walk(a, b, path) {
  if (diffs.length >= 10) return;
  if (a === b) return;
  const ta = a === null ? "null" : Array.isArray(a) ? "array" : typeof a;
  const tb = b === null ? "null" : Array.isArray(b) ? "array" : typeof b;
  if (ta !== tb) return void diffs.push(`${path}: sent ${ta}, stored ${tb}`);
  if (ta === "array") {
    if (a.length !== b.length) return void diffs.push(`${path}: sent ${a.length} items, stored ${b.length}`);
    a.forEach((v, i) => walk(v, b[i], `${path}[${i}]`));
    return;
  }
  if (ta === "object") {
    for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
      if (!(k in a)) diffs.push(`${path}.${k}: absent when sent, stored ${JSON.stringify(b[k])}`);
      else if (!(k in b)) diffs.push(`${path}.${k}: sent ${JSON.stringify(a[k])}, absent when stored`);
      else walk(a[k], b[k], `${path}.${k}`);
    }
    return;
  }
  diffs.push(`${path}: sent ${JSON.stringify(a)}, stored ${JSON.stringify(b)}`);
})(sent, got, "config");

for (const d of diffs) {
  console.log(d);
  failures++;
}

if (failures) process.exit(1);
const nsig = got.project?.signals?.length ?? 0;
console.log(`config identical after round-trip (${raw.length} B, ${nsig} signal(s), ${got.daq_lists?.length ?? 0} DAQ list(s))`);
