#!/usr/bin/env node
// Prints the canlib channel numbers that are virtual, one per line.
//
//   node virtual-channels.mjs <devices-all.json> <devices-no-virtuals.json>
import { readFileSync } from "node:fs";

const channelsOf = (path) => {
  const out = new Set();
  for (const dev of JSON.parse(readFileSync(path, "utf8"))) {
    for (const ch of dev.channels ?? []) out.add(ch.channelNumber);
  }
  return out;
};

const [allPath, physPath] = process.argv.slice(2);
const all = channelsOf(allPath);
const phys = channelsOf(physPath);
const byDiff = [...all].filter((c) => !phys.has(c)).sort((a, b) => a - b);

for (const c of byDiff) console.log(c);
