import { Box, Checkbox, FormControlLabel, TextField, Typography } from '@mui/material';
import { parseNonNegativeInt, TScriptOptions } from './util';

export interface TScriptSectionProps {
  options: TScriptOptions;
  onOptionsChange: (value: TScriptOptions) => void;
}

/**
 * Options for the generated t-script (the "Download t-script" artefact).
 * Bitrates, CAN FD mode, and the XCP master/slave ids come from the A2L;
 * these fields cover the rest.
 */
function TScriptSection({ options, onOptionsChange }: TScriptSectionProps) {
  const patch = (p: Partial<TScriptOptions>) => onOptionsChange({ ...options, ...p });

  const channelError = parseNonNegativeInt(options.channelIndex) === null;
  const pretriggerError = parseNonNegativeInt(options.pretriggerMs) === null;
  const timerError = options.timerStop && parseNonNegativeInt(options.timerMs) === null;

  return (
    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 1, my: 1 }}>
      <Typography variant="caption" color="text.secondary">
        Logging options for the downloadable t-script. Bus parameters are set in the CAN bus section; the master/slave
        ids come from the A2L.
      </Typography>
      <Box sx={{ display: 'flex', gap: 2, alignItems: 'center', flexWrap: 'wrap' }}>
        <TextField
          size="small"
          label="CAN channel index"
          value={options.channelIndex}
          error={channelError}
          helperText={channelError ? 'Not a valid number' : undefined}
          onChange={e => patch({ channelIndex: e.target.value })}
          sx={{ width: 160 }}
        />
        <TextField
          size="small"
          label="Pretrigger (ms)"
          value={options.pretriggerMs}
          error={pretriggerError}
          helperText={pretriggerError ? 'Not a valid number' : undefined}
          onChange={e => patch({ pretriggerMs: e.target.value })}
          sx={{ width: 160 }}
        />
        <FormControlLabel
          control={<Checkbox checked={options.timerStop} onChange={e => patch({ timerStop: e.target.checked })} />}
          label="Stop logging after"
        />
        <TextField
          size="small"
          label="Timer (ms)"
          value={options.timerMs}
          disabled={!options.timerStop}
          error={timerError}
          helperText={timerError ? 'Not a valid number' : undefined}
          onChange={e => patch({ timerMs: e.target.value })}
          sx={{ width: 160 }}
        />
      </Box>
      <FormControlLabel
        control={
          <Checkbox checked={options.standalone ?? false} onChange={e => patch({ standalone: e.target.checked })} />
        }
        label="Standalone t-script (no DBC needed, experimental feature)"
      />
      <Typography variant="caption" color="text.secondary">
        {options.standalone
          ? 'The script decodes the DAQ frames itself, so it compiles on its own: scc.exe <file>.t.'
          : 'Trigger signals are read through the generated DBC, which must be compiled in: scc.exe -dbase=<file>.dbc <file>.t.'}
      </Typography>
    </Box>
  );
}

export default TScriptSection;
