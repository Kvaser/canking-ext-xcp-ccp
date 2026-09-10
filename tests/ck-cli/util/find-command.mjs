#!/usr/bin/env node
// Find a command the extension transmitted in a captured .asc log and print one byte of it.
//
//   node find-command.mjs <log.asc> --tx-on-id N --code HH [--match-byte i=N] --print-byte i
//
// Exits 1 when no matching command is in the log.
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { parseAsc } from "./asc.mjs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: {
    "tx-on-id": { type: "string" },
    code: { type: "string" },
    "match-byte": { type: "string" },
    "print-byte": { type: "string" },
  },
});
const [logPath] = positionals;

const id = Number(values["tx-on-id"]);
const code = parseInt(values.code, 16);
const printByte = Number(values["print-byte"]);
if (!logPath || !Number.isInteger(id) || !Number.isInteger(code) || !Number.isInteger(printByte)) {
  console.error(
    "usage: find-command.mjs <log.asc> --tx-on-id N --code HH [--match-byte i=N] --print-byte i",
  );
  process.exit(1);
}

// Optional extra constraint, e.g. --match-byte 2=1 for "byte 2 is DAQ list 1".
const matchByte = values["match-byte"];
const [matchIndex, matchValue] = matchByte ? matchByte.split("=").map(Number) : [];

for (const frame of parseAsc(readFileSync(logPath, "utf8"))) {
  if (frame.dir !== "Tx" || frame.id !== id) continue;
  if (frame.bytes[0] !== code) continue;
  if (matchByte !== undefined && frame.bytes[matchIndex] !== matchValue) continue;
  const value = frame.bytes[printByte];
  if (value === undefined) continue;
  console.log(value);
  process.exit(0);
}
process.exit(1);
