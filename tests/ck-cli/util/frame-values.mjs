// Shared signal-value encoding for synthetic DAQ frames.

// Deterministic, small, and positive so it is valid for both signed and unsigned entries.
export function rawFor(i, round) {
  return 1 + ((i * 13 + 7 + round * 29) % 100);
}

export function encode(raw, size, littleEndian, signed) {
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

export function physical(sig, raw, cmByName) {
  const cm = sig.conversion ? cmByName.get(sig.conversion.identifier) : undefined;
  if (cm?.formula) {
    throw new Error(`signal '${sig.name}' uses a COMPU_METHOD formula; this generator only models factor/offset`);
  }
  const factor = cm?.factor ?? 1;
  const offset = cm?.offset ?? 0;
  return raw * factor + offset;
}

// Floating-point tolerance for comparing a decoded value against an expectation.
export function close(a, b) {
  return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b));
}

export function checkEncodable(sig) {
  if (sig.is_float) throw new Error(`signal '${sig.name}' is a float; not supported by this generator yet`);
  if (sig.bit_mask !== null && sig.bit_mask !== undefined) {
    const full = (1n << BigInt(8 * sig.size)) - 1n;
    if (BigInt(sig.bit_mask) !== full) {
      throw new Error(
        `signal '${sig.name}' has a partial bit_mask ` +
          `0x${BigInt(sig.bit_mask).toString(16)} over ${sig.size} byte(s); not supported by this generator yet`,
      );
    }
  }
  if ((sig.virtual_inputs ?? []).length) {
    throw new Error(`signal '${sig.name}' is a virtual/computed measurement; it has no place in a DTO`);
  }
}
