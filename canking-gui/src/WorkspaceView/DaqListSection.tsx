import {
  Box,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  Typography,
} from '@mui/material';

import { DaqList } from '../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js';
import { formatCanId, parseCanId, parsePrescaler } from './util';

export interface DaqListSectionProps {
  daqLists: DaqList[];
  /** DAQ list id (as a string key) -> the raw text the user typed for its CAN id override. */
  overrides: Record<string, string>;
  onOverridesChange: (value: Record<string, string>) => void;
  /** Largest prescaler the protocol can send, or undefined when the lists don't accept one. */
  prescalerMax?: number;
  /** DAQ list id (as a string key) -> the raw text the user typed for its prescaler. */
  prescalers: Record<string, string>;
  onPrescalersChange: (value: Record<string, string>) => void;
}

/**
 * Step 5: each packed DAQ list with its CAN identifier and prescaler.
 *
 * - CAN id: a list the A2L declares runtime-assignable (XCP `DAQ_LIST_CAN_ID ... VARIABLE`) takes a user-chosen id,
 *   sent with `SET_DAQ_ID`. Left blank it shares the transport's default id, multiplexed by PID.
 * - Prescaler: the list transmits on every n-th cycle of its event. Blank is 1, every cycle.
 */
function DaqListSection({
  daqLists,
  overrides,
  onOverridesChange,
  prescalerMax,
  prescalers,
  onPrescalersChange,
}: DaqListSectionProps) {
  if (daqLists.length === 0) {
    return null;
  }

  const dynamicCount = daqLists.filter(l => l.canId.tag === 'dynamic').length;

  const setOverride = (listId: number, text: string) => {
    onOverridesChange({ ...overrides, [String(listId)]: text });
  };
  const setPrescaler = (listId: number, text: string) => {
    onPrescalersChange({ ...prescalers, [String(listId)]: text });
  };

  return (
    <Box sx={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
      <Typography variant="body2" color="text.secondary">
        {dynamicCount > 0
          ? `${dynamicCount} of ${daqLists.length} DAQ list(s) accept a runtime-assigned CAN id ` +
            `(XCP SET_DAQ_ID). Leave one blank to keep it on the shared default id.`
          : `None of these ${daqLists.length} DAQ list(s) accept a runtime-assigned CAN id; ` +
            `each uses either the shared default id or one fixed by the A2L.`}
        {prescalerMax !== undefined && ' A prescaler n sends a list on every n-th cycle of its event.'}
      </Typography>
      <TableContainer sx={{ border: 1, borderColor: 'divider', maxWidth: 860 }}>
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell sx={{ width: 90 }}>DAQ list</TableCell>
              <TableCell>Event</TableCell>
              <TableCell sx={{ width: 280 }}>CAN identifier</TableCell>
              {prescalerMax !== undefined && <TableCell sx={{ width: 140 }}>Prescaler</TableCell>}
            </TableRow>
          </TableHead>
          <TableBody>
            {daqLists.map(list => {
              const raw = overrides[String(list.id)] ?? '';
              const invalid = raw.trim() !== '' && parseCanId(raw) === null;
              const rawPrescaler = prescalers[String(list.id)] ?? '';
              const invalidPrescaler =
                prescalerMax !== undefined && parsePrescaler(rawPrescaler, prescalerMax) === null;
              return (
                <TableRow key={list.id} hover>
                  <TableCell sx={{ fontFamily: 'monospace' }}>{list.id}</TableCell>
                  <TableCell>{list.event ?? '(unassigned)'}</TableCell>
                  <TableCell>
                    {list.canId.tag === 'shared' && (
                      <Typography variant="body2" color="text.secondary">
                        Shared (default, PID-multiplexed)
                      </Typography>
                    )}
                    {list.canId.tag === 'fixed' && (
                      <Typography variant="body2" sx={{ fontFamily: 'monospace' }}>
                        Fixed {formatCanId(list.canId.val)}
                      </Typography>
                    )}
                    {list.canId.tag === 'dynamic' && (
                      <TextField
                        size="small"
                        fullWidth
                        placeholder="auto (shared default)"
                        value={raw}
                        error={invalid}
                        helperText={invalid ? 'invalid CAN id' : undefined}
                        onChange={e => setOverride(list.id, e.target.value)}
                      />
                    )}
                  </TableCell>
                  {prescalerMax !== undefined && (
                    <TableCell>
                      <TextField
                        size="small"
                        fullWidth
                        placeholder="1"
                        value={rawPrescaler}
                        error={invalidPrescaler}
                        helperText={invalidPrescaler ? `1 to ${prescalerMax}` : undefined}
                        onChange={e => setPrescaler(list.id, e.target.value)}
                      />
                    </TableCell>
                  )}
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </TableContainer>
    </Box>
  );
}

export default DaqListSection;
