import { useMemo, useState } from "react";
import {Box, Button, MenuItem, Select, Table, TableBody, TableCell, TableContainer, TableHead, TableRow, TextField, Typography} from "@mui/material";
import { EventInfo } from "../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js";
import { formatEvent, SignalEventMap } from "./util";

const MAX_ROWS = 400;

// Sentinel for "keep the event parsed from the A2L" (MenuItem value can't be "")
const A2L_DEFAULT = " a2l-default";

export interface EventAssignTableProps {
  events: EventInfo[];
  signalEvents: SignalEventMap;
  onSignalEventsChange: (value: SignalEventMap) => void;
}

/** Step 2: assign an XCP event to each selected signal, with bulk assignment. */
function EventAssignTable({
  events,
  signalEvents,
  onSignalEventsChange,
}: EventAssignTableProps) {
  const [filter, setFilter] = useState("");
  const [bulkEvent, setBulkEvent] = useState(A2L_DEFAULT);

  const selected = useMemo(
    () => Object.keys(signalEvents).sort(),
    [signalEvents],
  );

  const filtered = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (needle === "") {
      return selected;
    }
    return selected.filter((s) => s.toLowerCase().includes(needle));
  }, [selected, filter]);

  const visible = filtered.slice(0, MAX_ROWS);
  const defaultCount = selected.filter((s) => signalEvents[s] === "").length;

  const eventOptions = useMemo(
    () =>
      events.map((ev) => (
        <MenuItem key={ev.name} value={ev.name}>
          {formatEvent(ev)}
        </MenuItem>
      )),
    [events],
  );

  const assignEvent = (name: string, event: string) => {
    onSignalEventsChange({ ...signalEvents, [name]: event });
  };

  const bulkAssign = () => {
    const event = bulkEvent === A2L_DEFAULT ? "" : bulkEvent;
    const next = { ...signalEvents };
    for (const name of filtered) {
      next[name] = event;
    }
    onSignalEventsChange(next);
  };

  return (
    <Box
      sx={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          gap: 2,
          mb: 1,
          flexWrap: "wrap",
        }}
      >
        <TextField
          size="small"
          label="Search selected signals"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          sx={{ width: 240 }}
        />
        <Select
          size="small"
          value={bulkEvent}
          onChange={(e) => setBulkEvent(e.target.value)}
          sx={{ width: 260 }}
        >
          <MenuItem value={A2L_DEFAULT}>
            <em>From A2L</em>
          </MenuItem>
          {eventOptions}
        </Select>
        <Button size="small" variant="outlined" onClick={bulkAssign}>
          Assign to all {filter.trim() !== "" ? "matching" : ""} ({filtered.length})
        </Button>
      </Box>
      <Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
        {defaultCount > 0
          ? `${defaultCount} of ${selected.length} signals use the event from the A2L (if any).`
          : `All ${selected.length} signals have an explicit event.`}
      </Typography>
      <TableContainer
        sx={{ flex: 1, minHeight: 0, border: 1, borderColor: "divider" }}
      >
        <Table size="small" stickyHeader>
          <TableHead>
            <TableRow>
              <TableCell>Signal</TableCell>
              <TableCell sx={{ width: 300 }}>Event</TableCell>
            </TableRow>
          </TableHead>
          <TableBody>
            {visible.map((name) => (
              <TableRow key={name} hover>
                <TableCell sx={{ fontFamily: "monospace" }}>{name}</TableCell>
                <TableCell>
                  <Select
                    size="small"
                    fullWidth
                    value={
                      signalEvents[name] !== "" ? signalEvents[name] : A2L_DEFAULT
                    }
                    onChange={(e) =>
                      assignEvent(
                        name,
                        e.target.value === A2L_DEFAULT ? "" : e.target.value,
                      )
                    }
                  >
                    <MenuItem value={A2L_DEFAULT}>
                      <em>From A2L</em>
                    </MenuItem>
                    {eventOptions}
                  </Select>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableContainer>
      {filtered.length > MAX_ROWS && (
        <Typography variant="caption" color="text.secondary">
          Showing first {MAX_ROWS} of {filtered.length} matches, refine the search to see more.
        </Typography>
      )}
    </Box>
  );
}

export default EventAssignTable;
