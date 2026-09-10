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
} from "@mui/material";

import { DaqList } from "../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js";
import { formatCanId, parseCanId } from "./util";

export interface DaqCanIdSectionProps {
  daqLists: DaqList[];
  /** DAQ list id (as a string key) -> the raw text the user typed for its CAN id override. */
  overrides: Record<string, string>;
  onOverridesChange: (value: Record<string, string>) => void;
}

/**
 * Step 5: shows each packed DAQ list's CAN identifier, and lets the user assign one to
 * any list the A2L declares as runtime-assignable (XCP `DAQ_LIST_CAN_ID ... VARIABLE`).
 *
 * A list left blank keeps today's behaviour: it shares the transport's default CAN id,
 * multiplexed by PID, and no `SET_DAQ_ID` is sent for it.
 */
function DaqCanIdSection({
  daqLists,
  overrides,
  onOverridesChange,
}: DaqCanIdSectionProps) {
  if (daqLists.length === 0) {
    return null;
  }

  const dynamicCount = daqLists.filter((l) => l.canId.tag === "dynamic").length;

  const setOverride = (listId: number, text: string) => {
    onOverridesChange({ ...overrides, [String(listId)]: text });
  };

  return (
    <Box sx={{ display: "flex", flexDirection: "column", gap: 1 }}>
      <Typography variant="body2" color="text.secondary">
        {dynamicCount > 0
          ? `${dynamicCount} of ${daqLists.length} DAQ list(s) accept a runtime-assigned CAN id ` +
            `(XCP SET_DAQ_ID). Leave one blank to keep it on the shared default id.`
          : `None of these ${daqLists.length} DAQ list(s) accept a runtime-assigned CAN id; ` +
            `each uses either the shared default id or one fixed by the A2L.`}
      </Typography>
      <TableContainer sx={{ border: 1, borderColor: "divider", maxWidth: 720 }}>
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell sx={{ width: 90 }}>DAQ list</TableCell>
              <TableCell>Event</TableCell>
              <TableCell sx={{ width: 280 }}>CAN identifier</TableCell>
            </TableRow>
          </TableHead>
          <TableBody>
            {daqLists.map((list) => {
              const raw = overrides[String(list.id)] ?? "";
              const invalid = raw.trim() !== "" && parseCanId(raw) === null;
              return (
                <TableRow key={list.id} hover>
                  <TableCell sx={{ fontFamily: "monospace" }}>{list.id}</TableCell>
                  <TableCell>{list.event ?? "(unassigned)"}</TableCell>
                  <TableCell>
                    {list.canId.tag === "shared" && (
                      <Typography variant="body2" color="text.secondary">
                        Shared (default, PID-multiplexed)
                      </Typography>
                    )}
                    {list.canId.tag === "fixed" && (
                      <Typography variant="body2" sx={{ fontFamily: "monospace" }}>
                        Fixed {formatCanId(list.canId.val)}
                      </Typography>
                    )}
                    {list.canId.tag === "dynamic" && (
                      <TextField
                        size="small"
                        fullWidth
                        placeholder="auto (shared default)"
                        value={raw}
                        error={invalid}
                        helperText={invalid ? "invalid CAN id" : undefined}
                        onChange={(e) => setOverride(list.id, e.target.value)}
                      />
                    )}
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </TableContainer>
    </Box>
  );
}

export default DaqCanIdSection;
