// The Vector ASCII (.asc) message log format, in one place.
//
// A frame as this module exchanges it: `data` is an array of byte values.
//
// Anything wider than 8 bytes is a CAN FD frame.

const HEADER = [
  "date Mon Feb 07:00:00 am 2000",
  "base hex timestamps relative",
  "// CAN channel: 1 2 3 4 5",
  "Begin Triggerblock",
  "// ;    time  can ident           attr dlc data ...  ",
];

// Bytes as uppercase space-joined hex.
const hex = (data) => Array.from(data, (b) => b.toString(16).padStart(2, "0").toUpperCase()).join(" ");

// One log line.
function frameLine({ time, channel = 1, dir = "Rx", id, data }) {
  const n = data.length;
  return (
    `   ${time.toFixed(9)} CANFD   ${channel} ${dir}        ${id.toString(16).padStart(3, "0")}` +
    `                                   0 0 ${n}  ${n} ${hex(data)}` +
    `        0    0   200000        0        0        0        0        0`
  );
}

// Render a whole log. `frames` are written in the order given.
export function renderAsc(frames) {
  return [...HEADER, ...frames.map(frameLine), "End Triggerblock", ""].join("\n");
}

// Parse one line, or null if it is not a data frame. `data` comes back as an uppercase space-joined hex string
export function parseAscLine(line) {
  const t = line.trim().split(/\s+/);
  if (t.length < 10) return null;
  if (!/^(CAN|CANFD)$/.test(t[1])) return null;
  if (!/^(Rx|Tx)$/.test(t[3])) return null;
  const len = Number(t[8]);
  if (!Number.isInteger(len) || len < 0 || len > 64) return null;
  const data = t.slice(9, 9 + len);
  if (data.length !== len || data.some((b) => !/^[0-9a-fA-F]{2}$/.test(b))) return null;
  return {
    time: Number(t[0]),
    channel: Number(t[2]),
    dir: t[3],
    id: parseInt(t[4], 16),
    data: data.map((b) => b.toUpperCase()).join(" "),
    bytes: data.map((b) => parseInt(b, 16)),
  };
}

// Every data frame in a log file, in order.
export function parseAsc(text) {
  return text.split(/\r?\n/).map(parseAscLine).filter(Boolean);
}

export { hex };
