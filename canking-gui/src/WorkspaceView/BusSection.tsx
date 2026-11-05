import { Box, Checkbox, FormControlLabel, MenuItem, TextField, Typography } from '@mui/material';
import { TransportInfo } from '../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js';
import { ARBITRATION_BITRATES, BusOverride, busOverrideFromTransport, DATA_BITRATES, formatBitrate } from './util';

export interface BusSectionProps {
  bus: BusOverride;
  onBusChange: (value: BusOverride) => void;
  /** A2L-declared transport for the active protocol, used to pre-fill / show defaults. */
  detected?: TransportInfo;
}

/** Human-readable summary of the bus params the A2L declares. */
function describeTransport(t: TransportInfo | undefined): string {
  if (!t) {
    return 'The A2L does not declare bus parameters; defaults are used.';
  }
  const parts = [t.canFd ? 'CAN FD' : 'CAN'];
  if (t.bitrate !== undefined) {
    parts.push(`${formatBitrate(t.bitrate)} arbitration`);
  }
  if (t.canFd && t.dataBitrate !== undefined) {
    parts.push(`${formatBitrate(t.dataBitrate)} data`);
  }
  return `A2L declares: ${parts.join(', ')}.`;
}

/**
 * Optional manual override of the CAN bus parameters for the generated
 * t-script, for when the A2L's declared bit rate / framing is wrong.
 */
function BusSection({ bus, onBusChange, detected }: BusSectionProps) {
  const patch = (p: Partial<BusOverride>) => onBusChange({ ...bus, ...p });

  const toggle = (enabled: boolean) => {
    onBusChange(enabled ? busOverrideFromTransport(detected) : { ...bus, enabled: false });
  };

  return (
    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 1, my: 1 }}>
      <Typography variant="caption" color="text.secondary">
        {describeTransport(detected)} Enable the override only if these values are wrong for your setup.
      </Typography>
      <FormControlLabel
        control={<Checkbox checked={bus.enabled} onChange={e => toggle(e.target.checked)} />}
        label="Override A2L bus settings"
      />
      {bus.enabled && (
        <Box sx={{ display: 'flex', gap: 2, alignItems: 'center', flexWrap: 'wrap' }}>
          <TextField
            size="small"
            select
            label="CAN mode"
            value={bus.mode}
            onChange={e => patch({ mode: e.target.value as BusOverride['mode'] })}
            sx={{ width: 160 }}
          >
            <MenuItem value="can">CAN</MenuItem>
            <MenuItem value="can-fd">CAN FD</MenuItem>
          </TextField>
          <TextField
            size="small"
            select
            label="Arbitration bit rate"
            value={bus.bitrate}
            onChange={e => patch({ bitrate: Number(e.target.value) })}
            sx={{ width: 180 }}
          >
            {ARBITRATION_BITRATES.map(bps => (
              <MenuItem key={bps} value={bps}>
                {formatBitrate(bps)}
              </MenuItem>
            ))}
          </TextField>
          <TextField
            size="small"
            select
            label="Data bit rate"
            value={bus.dataBitrate}
            disabled={bus.mode !== 'can-fd'}
            helperText={bus.mode !== 'can-fd' ? 'CAN FD only' : undefined}
            onChange={e => patch({ dataBitrate: Number(e.target.value) })}
            sx={{ width: 180 }}
          >
            {DATA_BITRATES.map(bps => (
              <MenuItem key={bps} value={bps}>
                {formatBitrate(bps)}
              </MenuItem>
            ))}
          </TextField>
        </Box>
      )}
    </Box>
  );
}

export default BusSection;
