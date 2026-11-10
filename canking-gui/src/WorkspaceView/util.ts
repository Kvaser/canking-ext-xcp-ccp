import { alpha, Theme } from '@mui/material';
import { CanMode, EventInfo, TransportInfo } from '../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js';

export const panelSx = (theme: Theme) => ({
  border: `1px solid ${alpha(theme.palette.text.primary, 0.12)}`,
  background: alpha(theme.palette.text.primary, 0.03),
  borderRadius: 1.5,
});

/** Form steps stop growing and captions stay readable on wide panes. */
export const FORM_MAX_WIDTH = 900;

/**
 * A single trigger condition, e.g. `Rpm > 2500`.
 *
 */
export interface TriggerCondition {
  signal: string;
  operator: '>' | '>=' | '<' | '<=' | '==' | '!=';
  value: string;
}

export const TRIGGER_OPERATORS: TriggerCondition['operator'][] = ['>', '>=', '<', '<=', '==', '!='];

export async function readA2lText(file: File): Promise<string> {
  const buffer = await file.arrayBuffer();
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: false }).decode(buffer);
  } catch {
    return new TextDecoder('windows-1252').decode(buffer);
  }
}

export const ARBITRATION_BITRATES = [125_000, 250_000, 500_000, 1_000_000];
export const DATA_BITRATES = [500_000, 1_000_000, 2_000_000, 4_000_000, 8_000_000];

export const DEFAULT_TSCRIPT_OPTIONS: TScriptOptions = {
  channelIndex: '0',
  pretriggerMs: '200',
  timerStop: false,
  timerMs: '8000',
  maxCaptures: '1',
  watchdogMs: '10000',
  stopCanId: '',
  standalone: false,
  bus: {
    enabled: false,
    mode: 'can',
    bitrate: 500_000,
    dataBitrate: 2_000_000,
  },
};

/** Mapping a selected signal name to its assigned event name. Presence of a key means the signal is selected. */
export type SignalEventMap = Record<string, string>;

export interface BusOverride {
  enabled: boolean;
  mode: CanMode;
  bitrate: number;
  dataBitrate: number;
}

/** Format a bit rate in bit/s as a "500 kbit/s" / "2 Mbit/s" label. */
export function formatBitrate(bps: number): string {
  return bps >= 1_000_000 ? `${bps / 1_000_000} Mbit/s` : `${bps / 1_000} kbit/s`;
}

/** The bus params the A2L declares, e.g. "CAN FD, 500 kbit/s arbitration, 2 Mbit/s data". */
export function describeTransport(t: TransportInfo): string {
  const parts = [t.canFd ? 'CAN FD' : 'CAN'];
  if (t.bitrate !== undefined) {
    parts.push(`${formatBitrate(t.bitrate)} arbitration`);
  }
  if (t.canFd && t.dataBitrate !== undefined) {
    parts.push(`${formatBitrate(t.dataBitrate)} data`);
  }
  return parts.join(', ');
}

/**
 * Seed a {@link BusOverride} from the A2L-declared transport for the active protocol
 */
export function busOverrideFromTransport(transport: TransportInfo | undefined): BusOverride {
  const snap = (value: number | undefined, options: number[], fallback: number) =>
    value !== undefined && options.includes(value) ? value : fallback;
  return {
    enabled: true,
    mode: transport?.canFd ? 'can-fd' : 'can',
    bitrate: snap(transport?.bitrate, ARBITRATION_BITRATES, 500_000),
    dataBitrate: snap(transport?.dataBitrate, DATA_BITRATES, 2_000_000),
  };
}

/**
 * User-configurable options for the generated t-script. Numeric text fields are
 * kept as text so they can be edited freely (validate with {@link parseNonNegativeInt})
 */
export interface TScriptOptions {
  channelIndex: string;
  pretriggerMs: string;
  timerStop: boolean;
  timerMs: string;
  /** With the timer: captures before DAQ stops; blank for no limit. */
  maxCaptures: string;
  /** Restart the DAQ init after this many ms without a DAQ frame; blank to never. */
  watchdogMs: string;
  /** Stop logging and DAQ for good when a frame with this CAN id arrives; blank for none. */
  stopCanId: string;
  standalone: boolean;
  bus: BusOverride;
}

/** Parse a non-negative integer text field. Returns null when invalid. */
export function parseNonNegativeInt(text: string): number | null {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) {
    return null;
  }
  const value = Number.parseInt(trimmed, 10);
  return Number.isSafeInteger(value) ? value : null;
}

/** A whole number typed by the user: `blank` when empty; null unless it is from `min` to `max`. */
export function parseBoundedInt<B>(
  text: string,
  blank: B,
  min: number,
  max = Number.MAX_SAFE_INTEGER,
): number | B | null {
  if (text.trim() === '') {
    return blank;
  }
  const value = parseNonNegativeInt(text);
  return value !== null && value >= min && value <= max ? value : null;
}

/** The largest CAN identifier: 29 bits, i.e. the extended-id range. */
const MAX_CAN_ID = 0x1fff_ffff;

/**
 * Parse a CAN identifier text field: accepts `0x`-prefixed hex or plain decimal.
 * Returns null when invalid, empty, or out of the 29-bit range.
 */
export function parseCanId(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === '') {
    return null;
  }
  const hex = /^0x([0-9a-f]+)$/i.exec(trimmed);
  const value = hex ? Number.parseInt(hex[1], 16) : /^\d+$/.test(trimmed) ? Number.parseInt(trimmed, 10) : NaN;
  return Number.isSafeInteger(value) && value >= 0 && value <= MAX_CAN_ID ? value : null;
}

/** Format a CAN id or address as upper-case hex, e.g. "0x7FF". */
export function formatHex(n: number | bigint): string {
  return `0x${n.toString(16).toUpperCase()}`;
}

// XCP event channel time units (ASAM XCP TIMESTAMP/EVENT unit codes)
const TIME_UNITS = ['1 ns', '10 ns', '100 ns', '1 µs', '10 µs', '100 µs', '1 ms', '10 ms', '100 ms', '1 s', '10 s'];

/** Human-readable event label, e.g. "Task10ms (10 × 1 ms)". */
export function formatEvent(event: EventInfo): string {
  if (event.timeCycle === 0) {
    return `${event.name} (on demand)`;
  }
  const unit = TIME_UNITS[event.timeUnit] ?? `unit ${event.timeUnit}`;
  return `${event.name} (${event.timeCycle} x ${unit})`;
}

/** Extract a readable message from an error thrown by the wasm component. */
export function errorText(e: unknown): string {
  if (e && typeof e === 'object' && 'payload' in e) {
    return String((e as { payload: unknown }).payload);
  }
  return e instanceof Error ? e.message : String(e);
}
