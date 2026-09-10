#!/usr/bin/env node
// Assert the extension's own log messages for the most recent run.
//
//   node eventlog.mjs <CanKing_Log.log> <nodeName> [--prefix "text"]
//
// The extension reports through the host's log_msg, and CanKing writes those
// lines into the service log prefixed with the data processor's node name.
//
// The service log is cumulative across the whole service lifetime, so we scope
// to the current run by taking only what follows the LAST "Added new data processor '<nodeName>'" line.
import { parseArgs } from "node:util";
import { readFileSync } from "node:fs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  options: { prefix: { type: "string" } },
  allowPositionals: true,
});
const [logPath, nodeName] = positionals;
const prefix = values.prefix ?? nodeName;

if (!logPath || !nodeName) {
  console.error("usage: eventlog.mjs <log> <nodeName> [--prefix \"text\"]");
  process.exit(1);
}

const all = readFileSync(logPath, "latin1").split(/\r?\n/);
const anchor = all.reduce(
  (last, l, i) => (l.includes(`Added new data processor '${nodeName}'`) ? i : last),
  -1,
);

// The caller passes a node name unique to this run, so a missing anchor means the log does not cover this run at all
if (anchor < 0) {
  console.log(
    `service log does not cover this run (no "Added new data processor '${nodeName}'").\n` +
      `The CanKing service was probably restarted; ck get event-log then returns the\n` +
      `previous session's log. Skipping the extension-log assertions.`,
  );
  process.exit(77);
}

// Lines this node emitted during the run we just performed.
const mine = all.slice(anchor).filter((l) => l.includes(`${prefix}: `));
const textOf = (l) => l.slice(l.indexOf(`${prefix}: `) + prefix.length + 2);

let failures = 0;

// Anything the extension flagged is bad
const problems = mine.filter((l) => l.includes("[Warning]") || l.includes("[Error]"));
for (const p of problems) {
  console.log(`unexpected extension ${p.includes("[Error]") ? "error" : "warning"}: ${textOf(p)}`);
  failures++;
}

if (failures) {
  process.exit(1);
}
console.log(`extension logged ${mine.length} line(s), no unexpected warnings`);
for (const l of mine) console.log(`    ${textOf(l)}`);
