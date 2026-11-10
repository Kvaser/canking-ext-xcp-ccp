import { Box, Checkbox, FormControlLabel, TextField, Typography } from '@mui/material';
import { DEFAULT_TSCRIPT_OPTIONS, parseNonNegativeInt, parseOptionalPositiveInt, TScriptOptions } from './util';

export interface TScriptSectionProps {
  options: TScriptOptions;
  onOptionsChange: (value: TScriptOptions) => void;
}

const command = (text: string) => <code style={{ whiteSpace: 'nowrap' }}>{text}</code>;

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
  const maxCaptures = options.maxCaptures ?? DEFAULT_TSCRIPT_OPTIONS.maxCaptures ?? '';
  const watchdogMs = options.watchdogMs ?? DEFAULT_TSCRIPT_OPTIONS.watchdogMs ?? '';
  const capturesError = options.timerStop && parseOptionalPositiveInt(maxCaptures) === null;
  const watchdogError = parseOptionalPositiveInt(watchdogMs) === null;

  return (
    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 1, my: 1 }}>
      <Typography variant="caption" color="text.secondary">
        Logging options for the downloadable t-script. The bus is set under CAN bus; the master/slave ids come from the
        A2L.
      </Typography>
      <Box sx={{ display: 'flex', gap: 2, alignItems: 'flex-start', flexWrap: 'wrap' }}>
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
        {/* The checkbox and the fields it enables wrap as one group */}
        <Box sx={{ display: 'flex', gap: 2, alignItems: 'flex-start' }}>
          <FormControlLabel
            control={<Checkbox checked={options.timerStop} onChange={e => patch({ timerStop: e.target.checked })} />}
            label="Stop logging after"
            sx={{ whiteSpace: 'nowrap', mr: 0 }}
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
          <TextField
            size="small"
            label="Captures"
            placeholder="no limit"
            value={maxCaptures}
            disabled={!options.timerStop}
            error={capturesError}
            helperText={capturesError ? 'From 1, or blank' : undefined}
            onChange={e => patch({ maxCaptures: e.target.value })}
            sx={{ width: 160 }}
          />
        </Box>
      </Box>
      <Typography variant="caption" color="text.secondary">
        With a timer, each capture logs for the timer&apos;s length. The next one starts when the trigger condition has
        been false and turns true again, while DAQ keeps running. After the last capture the script stops DAQ.
      </Typography>
      <TextField
        size="small"
        label="Restart DAQ after silence (ms)"
        placeholder="never"
        value={watchdogMs}
        error={watchdogError}
        helperText={watchdogError ? 'From 1, or blank' : undefined}
        onChange={e => patch({ watchdogMs: e.target.value })}
        sx={{ width: 240, mt: 1 }}
      />
      <Typography variant="caption" color="text.secondary">
        If no DAQ frame arrives for this long, e.g. after the ECU restarts, the script runs the DAQ init again. A failed
        init always starts over after 5 s.
      </Typography>
      <FormControlLabel
        control={
          <Checkbox checked={options.standalone ?? false} onChange={e => patch({ standalone: e.target.checked })} />
        }
        label="Standalone t-script (no DBC needed, experimental feature)"
      />
      <Typography variant="caption" color="text.secondary">
        {options.standalone ? (
          <>The script decodes the DAQ frames itself, so it compiles on its own: {command('scc.exe <file>.t')}</>
        ) : (
          <>
            Trigger signals are read through the generated DBC, which must be compiled in:{' '}
            {command('scc.exe -dbase=<file>.dbc <file>.t')}
          </>
        )}
      </Typography>
    </Box>
  );
}

export default TScriptSection;
