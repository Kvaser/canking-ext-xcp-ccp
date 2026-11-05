import { CanMode, EventInfo, TransportInfo } from '../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js';

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
  standalone?: boolean;
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

/** A DAQ list prescaler typed by the user: blank is 1; null unless it is a whole number from 1 to `max`. */
export function parsePrescaler(text: string, max: number): number | null {
  if (text.trim() === '') {
    return 1;
  }
  const value = parseNonNegativeInt(text);
  return value !== null && value >= 1 && value <= max ? value : null;
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

/** Format a CAN identifier the way the rest of this UI displays one. */
export function formatCanId(id: number): string {
  return `0x${id.toString(16).toUpperCase()}`;
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
