#!/usr/bin/env node
// Headless a2l -> Config JSON.
//
//   node build-config.mjs <case.json> --out-dir DIR [--strict]
//
// A case file in /cases is the replacement for the GUI's selection screens:
//
// Writes <out-dir>/<name>.config.json and <out-dir>/<name>.dbc. --strict turns component warnings into a non-zero exit.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve, join } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { fail } from "./cli.mjs";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const WASM_JS = join(REPO, "canking-gui", "src", "a2l-daq-wasm", "a2l_daq.js");

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: { "out-dir": { type: "string" }, strict: { type: "boolean", default: false } },
});
const [casePath] = positionals;
if (!casePath || !values["out-dir"]) fail("usage: build-config.mjs <case.json> --out-dir DIR [--strict]");
const strict = values.strict;
const outDir = resolve(values["out-dir"]);

const testCase = JSON.parse(readFileSync(casePath, "utf8"));
const name = testCase.name ?? "config";
const a2lPath = resolve(REPO, testCase.a2l);
const a2l = readFileSync(a2lPath, testCase.encoding ?? "latin1");

// The transpiled component resolves @bytecodealliance/preview2-shim relative to itself
const { generator } = await import(pathToFileURL(WASM_JS).href);

// ---- inspect: what the A2L declares ---------------------------------------
let summary;
try {
  summary = generator.inspect(a2l);
} catch (e) {
  fail(`inspect failed: ${e?.message ?? e}`);
}

const protocol = testCase.protocol ?? summary.protocols[0];
if (!protocol) fail("A2L declares no supported protocol");
if (!summary.protocols.includes(protocol)) {
  fail(`case asks for '${protocol}', A2L declares [${summary.protocols.join(", ")}]`);
}

// ---- selection + event assignment ------------------------------------------
const available = new Set(summary.signals.map((s) => s.name));
const selected = testCase.signals ?? summary.signals.map((s) => s.name);
const unknown = selected.filter((s) => !available.has(s));
if (unknown.length) fail(`case names signals absent from the A2L: ${unknown.join(", ")}`);

let assignments;
if (testCase.assignments) {
  assignments = Object.entries(testCase.assignments).map(([event, signals]) => ({ event, signals }));
  const events = new Set(summary.events.map((e) => e.name));
  const badEvents = assignments.map((a) => a.event).filter((e) => !events.has(e));
  if (badEvents.length) fail(`case names unknown events: ${badEvents.join(", ")}`);
} else {
  assignments = summary.events.map((e) => ({ event: e.name, signals: [] }));
  if (assignments.length) {
    selected.forEach((s, i) => assignments[i % assignments.length].signals.push(s));
  }
}

// ---- build ------------------------------------------------------------------
let out;
try {
  out = generator.buildConfig(
    a2l,
    testCase.a2l.split(/[\\/]/).pop(),
    selected,
    assignments,
    testCase.startDaq ?? false,
    testCase.daqCanIdOverrides ?? [],
    testCase.daqPrescalers ?? [],
    protocol,
  );
} catch (e) {
  fail(`buildConfig failed: ${e?.message ?? e}`);
}

// parse JSON
let cfg;
try {
  cfg = JSON.parse(out.configJson);
} catch (e) {
  fail(`buildConfig returned invalid JSON: ${e.message}`);
}

mkdirSync(outDir, { recursive: true });
const cfgPath = join(outDir, `${name}.config.json`);
const dbcPath = join(outDir, `${name}.dbc`);
writeFileSync(cfgPath, JSON.stringify(cfg)); 
writeFileSync(dbcPath, out.dbc);

// ---- report -----------------------------------------------------------------
const xcp = cfg.project.xcp_data?.xcp_info;
const ccp = cfg.project.ccp_data?.info;
console.log(`${name}: ${a2lPath}`);
console.log(`  A2L declares      ${summary.signals.length} signals, ${summary.events.length} events, protocols=[${summary.protocols.join(",")}]`);
console.log(`  protocol          ${cfg.protocol}   start_daq=${cfg.start_daq}`);
console.log(`  selected          ${selected.length} signals -> ${cfg.project.signals.length} in config`);
if (xcp) console.log(`  cro/dto id        0x${xcp.can_id_master?.toString(16)} / 0x${xcp.can_id_slave?.toString(16)}`);
if (ccp) console.log(`  cro/dto id        0x${ccp.cro_id?.toString(16)} / 0x${ccp.dto_id?.toString(16)}`);
const ts = cfg.project.xcp_data?.daq_info?.timestamp_size ?? 0;
console.log(`  timestamp_size    ${ts} byte(s)`);
console.log(`  daq_lists         ${cfg.daq_lists.length}`);
for (const dl of cfg.daq_lists) {
  const n = dl.odts.reduce((a, o) => a + o.entries.length, 0);
  console.log(`    id=${dl.id} first_pid=${dl.first_pid} event=${dl.event} odts=${dl.odts.length} entries=${n}`);
}
console.log(`  wrote             ${cfgPath} (${out.configJson.length} B)`);
console.log(`                    ${dbcPath} (${out.dbc.split("\n").length} lines)`);

if (out.warnings.length) {
  console.log(`  warnings          ${out.warnings.length}`);
  for (const w of out.warnings) console.log(`    - ${JSON.stringify(w)}`);
  if (strict) process.exit(1);
} else {
  console.log(`  warnings          none`);
}
