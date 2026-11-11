#!/usr/bin/env node
// Assertions over a Kvaser-written .asc message log.
//
//   node asc.mjs <log.asc> --rx-payloads <expected.json>
//   node asc.mjs <log.asc> --tx-on-id <hex-id> [--first-data-byte HH]
//   node asc.mjs <log.asc> --tx-on-id <hex-id> [--match-byte i=N ...] --print-byte i
//   node asc.mjs <log.asc> --rx-on-id <hex-id>
//
// These assertions test if the right bytes reach the right channel in the right direction.
// --print-byte instead prints byte i of the first Tx frame on the id whose --match-byte constraints all hold
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { parseAsc } from "../util/asc.mjs";

const { positionals, values } = parseArgs({
  args: process.argv.slice(2),
  allowPositionals: true,
  options: {
    "rx-payloads": { type: "string" },
    "tx-on-id": { type: "string" },
    "rx-on-id": { type: "string" },
    "first-data-byte": { type: "string" },
    "match-byte": { type: "string", multiple: true, default: [] },
    "print-byte": { type: "string" },
  },
});
const [logPath] = positionals;

const frames = parseAsc(readFileSync(logPath, "utf8"));

let failures = 0;
const bad = (m) => {
  console.log(m);
  failures++;
};

if (values["rx-payloads"] !== undefined) {
  const exp = JSON.parse(readFileSync(values["rx-payloads"], "utf8"));
  const rx = frames.filter((f) => f.id === exp.dtoId && f.dir === "Rx");
  if (!rx.length) {
    bad(
      `no Rx frames on DTO id 0x${exp.dtoId.toString(16)} in ${logPath}, ` +
        `the replay did not reach the extension's channel ` +
        `(check the logreplay channel-map key: it is the log's internal channel index)`,
    );
  }
  // Multiset compare
  const rxCounts = new Map();
  for (const f of rx) rxCounts.set(f.data, (rxCounts.get(f.data) ?? 0) + 1);

  const wantCounts = new Map();
  const pidOf = new Map();
  for (const want of exp.frames) {
    wantCounts.set(want.data, (wantCounts.get(want.data) ?? 0) + 1);
    pidOf.set(want.data, want.pid);
  }

  for (const [data, wantN] of wantCounts) {
    const gotN = rxCounts.get(data) ?? 0;
    if (gotN < wantN) {
      bad(
        `payload [${data}] (PID ${pidOf.get(data)}): ${wantN - gotN} of ${wantN} expected ` +
          `occurrence(s) never arrived as Rx on 0x${exp.dtoId.toString(16)}`,
      );
    }
  }
  if (!failures) {
    console.log(`${exp.frames.length} replayed payload(s) arrived as Rx on id 0x${exp.dtoId.toString(16)}`);
  }
}

// Check Tx frames on a given id
if (values["tx-on-id"] !== undefined) {
  const id = parseInt(values["tx-on-id"], 16);
  const firstByte = values["first-data-byte"];
  const tx = frames.filter((f) => f.id === id && f.dir === "Tx");
  if (values["print-byte"] !== undefined) {
    const at = Number(values["print-byte"]);
    const want = values["match-byte"].map((m) => m.split("=").map(Number));
    const hit = tx.find((f) => f.bytes[at] !== undefined && want.every(([i, v]) => f.bytes[i] === v));
    if (hit) console.log(hit.bytes[at]);
    else failures++;
  } else if (!tx.length) {
    bad(
      `expected at least 1 Tx frame on id 0x${id.toString(16)}, saw none. ` +
        `If the config declares can_fd, the CAN channel must be added with --bit-rate-fd, ` +
        `or the extension finds no usable channel and silently skips DAQ startup.`,
    );
  } else if (firstByte && !tx[0].data.startsWith(firstByte.toUpperCase())) {
    bad(`first Tx frame on 0x${id.toString(16)} is [${tx[0].data}], expected it to start with ${firstByte}`);
  } else {
    console.log(`${tx.length} command frame(s) transmitted on id 0x${id.toString(16)}:`);
    for (const f of tx) console.log(`    ${f.time.toFixed(6)}  ${f.data}`);
  }
}

// Check Rx frames on a given id
if (values["rx-on-id"] !== undefined) {
  const id = parseInt(values["rx-on-id"], 16);
  const rx = frames.filter((f) => f.id === id && f.dir === "Rx");
  if (!rx.length) {
    bad(`expected at least 1 Rx frame on id 0x${id.toString(16)}, saw none`);
  } else {
    console.log(`${rx.length} frame(s) received on id 0x${id.toString(16)}`);
  }
}

process.exit(failures ? 1 : 0);
