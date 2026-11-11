// What scripts can share about a config and the DAQ frames it describes.

import { readFileSync } from "node:fs";

// Loads a config and the necessary values from it. Throws when it has no DTO CAN id.
export function loadConfig(path) {
  const cfg = JSON.parse(readFileSync(path, "utf8"));
  const proj = cfg.project;
  const isXcp = String(cfg.protocol).toLowerCase() === "xcp";
  const dtoId = isXcp ? proj.xcp_data?.xcp_info?.can_id_slave : proj.ccp_data?.info?.dto_id;
  if (dtoId === undefined || dtoId === null) throw new Error("config has no DTO CAN id");
  return {
    cfg,
    dtoId,
    sigByName: new Map(proj.signals.map((s) => [s.name, s])),
    cmByName: new Map((proj.compu_methods ?? []).map((c) => [c.name, c])),
    defaultOrder: proj.default_byte_order ?? "MsbLast",
    // Timestamps are an XCP DAQ property and ride in the first ODT of a list only.
    tsSize: isXcp ? (proj.xcp_data?.daq_info?.timestamp_size ?? 0) : 0,
  };
}

// "MsbLast" (little-endian) or "MsbFirst".
export function byteOrder(sig, defaultOrder) {
  const order = sig.byte_order ?? defaultOrder;
  if (order !== "MsbLast" && order !== "MsbFirst") throw new Error(`unknown byte_order '${order}'`);
  return order;
}

export function checkFullMask(sig) {
  if (sig.bit_mask === null || sig.bit_mask === undefined) return;
  if (BigInt(sig.bit_mask) !== (1n << BigInt(8 * sig.size)) - 1n) {
    throw new Error(
      `signal '${sig.name}' has a partial bit_mask 0x${BigInt(sig.bit_mask).toString(16)} ` +
        `over ${sig.size} byte(s)`,
    );
  }
}

// The factor/offset of a signal's COMPU_METHOD. A formula is not modelled.
export function scaling(sig, cmByName) {
  const cm = sig.conversion ? cmByName.get(sig.conversion.identifier) : undefined;
  if (cm?.formula) throw new Error(`signal '${sig.name}' uses a COMPU_METHOD formula; only factor/offset are modelled`);
  return { factor: cm?.factor ?? 1, offset: cm?.offset ?? 0 };
}

// Floating-point tolerance for comparing a decoded value against an expectation.
export function close(a, b) {
  return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b));
}

// Deterministic, small, and positive so it is valid for both signed and unsigned entries.
function rawFor(i, round) {
  return 1 + ((i * 13 + 7 + round * 29) % 100);
}

function encode(raw, size, littleEndian, signed) {
  const buf = Buffer.alloc(size);
  let v = BigInt(raw);
  if (v < 0n) {
    if (!signed) throw new Error("negative raw value for an unsigned signal");
    v += 1n << BigInt(8 * size);
  }
  for (let i = 0; i < size; i++) {
    const shift = BigInt(8 * (littleEndian ? i : size - 1 - i));
    buf[i] = Number((v >> shift) & 0xffn);
  }
  return buf;
}

// One ODT's DTO payload filled with known values and what physical values it decodes to
//
//   byte 0     the PID
//   bytes 1..N timestamp, N = timestamp size   (first ODT only)
//   remaining  the ODT's entries in order, each `size` bytes
//
// index(name) numbers the signals for their raw values and round varies them between repeats.
// Throws on what this layout does not model: floats, partial bit masks, virtual signals.
export function odtPayload(layout, dl, odtIndex, pid, round, index) {
  const { sigByName, cmByName, defaultOrder, tsSize } = layout;
  const parts = [Buffer.from([pid])];
  const signals = {};

  if (odtIndex === 0 && tsSize > 0) {
    const ts = 1000 * (dl.id + 1) + round;
    // The timestamp rides in the config's byte order, same as entry data.
    parts.push(encode(ts, tsSize, defaultOrder === "MsbLast", false));
    signals[`Timestamp_${dl.first_pid}`] = ts;
  }

  for (const entry of dl.odts[odtIndex].entries) {
    for (const name of entry.names) {
      const sig = sigByName.get(name);
      if (!sig) throw new Error(`DAQ list ${dl.id} references signal '${name}' absent from project.signals`);
      if (sig.is_float) throw new Error(`signal '${name}' is a float; not supported by this generator yet`);
      checkFullMask(sig);
      if ((sig.virtual_inputs ?? []).length) {
        throw new Error(`signal '${name}' is a virtual/computed measurement; it has no place in a DTO`);
      }
      const raw = rawFor(index(name), round);
      parts.push(encode(raw, entry.size, byteOrder(sig, defaultOrder) === "MsbLast", sig.is_signed));
      const { factor, offset } = scaling(sig, cmByName);
      signals[name] = raw * factor + offset;
    }
  }
  return { data: Buffer.concat(parts), signals };
}
