#!/usr/bin/env node
// Did any of the given phrases show up in the service log for this run?
//
//   node log-contains.mjs <CanKing_Log.log> <nodeName> "phrase one" ["phrase two" ...]
//
// Only lines after the last "Added new data processor '<nodeName>'" count,
// since the service log is cumulative across the whole service lifetime.
//
// Exits 0 if any phrase is found, 1 otherwise.
import { readFileSync } from "node:fs";

const [logPath, nodeName, ...phrases] = process.argv.slice(2);
if (!logPath || !nodeName || !phrases.length) {
  console.error('usage: log-contains.mjs <log> <nodeName> "phrase" ...');
  process.exit(2);
}

const all = readFileSync(logPath, "latin1").split(/\r?\n/);
const anchor = all.reduce(
  (last, l, i) => (l.includes(`Added new data processor '${nodeName}'`) ? i : last),
  -1,
);
const tail = anchor < 0 ? all : all.slice(anchor);

process.exit(tail.some((l) => phrases.some((p) => l.includes(p))) ? 0 : 1);
