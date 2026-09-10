import { memo, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {Autocomplete, Box, Checkbox, Chip, Table, TableBody, TableCell, TableContainer, TableHead, TableRow, TableSortLabel, TextField, Tooltip, Typography} from "@mui/material";
import { useVirtualizer } from "@tanstack/react-virtual";
import { GroupInfo, SignalSummary } from "../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js";
import { SignalEventMap } from "./util";

const ROW_HEIGHT = 40;
const ALL_GROUPS = "";

// Column widths for the header row; table-layout
const COL_CHECKBOX_WIDTH = 48;
const COL_SIGNAL_WIDTH = "24%";
const COL_GROUP_WIDTH = "20%";
const COL_UNIT_WIDTH = "8%";
const COL_ADDRESS_WIDTH = "11%";
const COL_SIZE_WIDTH = "7%";

const MAX_VISIBLE_GROUPS = 2;

function formatAddress(address: bigint): string {
  return `0x${address.toString(16).toUpperCase()}`;
}

/// Group names in ROOT/SUB_GROUP tree order, each paired with its depth for indentation.
function groupOptions(groups: GroupInfo[]): { name: string; depth: number }[] {
  const byName = new Map(groups.map((g) => [g.name, g]));
  const childNames = new Set(groups.flatMap((g) => g.subGroups));
  const roots = groups.filter((g) => !childNames.has(g.name));

  const options: { name: string; depth: number }[] = [];
  const seen = new Set<string>();
  const visit = (name: string, depth: number) => {
    if (seen.has(name)) return; // guards against a cyclic SUB_GROUP reference
    seen.add(name);
    options.push({ name, depth });
    byName.get(name)?.subGroups.forEach((child) => visit(child, depth + 1));
  };
  roots.forEach((g) => visit(g.name, 0));
  return options;
}

/// `groupName` plus every group reachable through its SUB_GROUP tree.
function groupAndDescendants(groupName: string, groups: GroupInfo[]): Set<string> {
  const byName = new Map(groups.map((g) => [g.name, g]));
  const result = new Set<string>();
  const stack = [groupName];
  while (stack.length > 0) {
    const name = stack.pop()!;
    if (result.has(name)) continue;
    result.add(name);
    byName.get(name)?.subGroups.forEach((child) => stack.push(child));
  }
  return result;
}

interface SignalRowProps {
  signal: SignalSummary;
  checked: boolean;
  onToggle: (name: string) => void;
  showGroupColumn: boolean;
  onGroupClick: (group: string) => void;
}

const singleLineSx = {
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
} as const;

// Memoized to avoid unnecessary re-renders.
const SignalRow = memo(function SignalRow({
  signal,
  checked,
  onToggle,
  showGroupColumn,
  onGroupClick,
}: SignalRowProps) {
  const issue = signal.virtualIssue;

  return (
    <TableRow
      hover={!issue}
      onClick={issue ? undefined : () => onToggle(signal.name)}
      sx={{ height: ROW_HEIGHT, cursor: issue ? "default" : "pointer" }}
    >
      <TableCell padding="none" sx={{ pl: 0.5 }}>
        <Checkbox size="small" checked={checked} disabled={!!issue} />
      </TableCell>
      <TableCell
        sx={{
          ...singleLineSx,
          fontFamily: "monospace",
          color: issue ? "text.disabled" : undefined,
        }}
      >
        {signal.name}
        {signal.virtualInput && !issue && (
          <Chip
            label="virtual"
            size="small"
            variant="outlined"
            sx={{ ml: 1, height: 18, fontSize: "0.65rem" }}
          />
        )}
        {issue && (
          <Tooltip title={`cannot be measured: ${issue.detail}`}>
            <Chip
              label={issue.label}
              size="small"
              color="warning"
              variant="outlined"
              sx={{ ml: 1, height: 18, fontSize: "0.65rem" }}
            />
          </Tooltip>
        )}
      </TableCell>
      {showGroupColumn && (
        <TableCell sx={{ overflow: "hidden", whiteSpace: "nowrap" }}>
          {signal.groups.slice(0, MAX_VISIBLE_GROUPS).map((group) => (
            <Tooltip key={group} title={`Filter to ${group}`}>
              <Chip
                label={group}
                size="small"
                variant="outlined"
                onClick={(e) => {
                  e.stopPropagation();
                  onGroupClick(group);
                }}
                sx={{ mr: 0.5, height: 18, fontSize: "0.65rem" }}
              />
            </Tooltip>
          ))}
          {signal.groups.length > MAX_VISIBLE_GROUPS && (
            <Tooltip title={signal.groups.slice(MAX_VISIBLE_GROUPS).join(", ")}>
              <Chip
                label={`+${signal.groups.length - MAX_VISIBLE_GROUPS}`}
                size="small"
                variant="outlined"
                sx={{ height: 18, fontSize: "0.65rem" }}
              />
            </Tooltip>
          )}
        </TableCell>
      )}
      <TableCell sx={{ ...singleLineSx, color: issue ? "text.disabled" : undefined }}>
        {signal.description}
      </TableCell>
      <TableCell sx={singleLineSx}>{signal.unit}</TableCell>
      <TableCell align="right" sx={{ fontFamily: "monospace" }}>
        {issue ? (
          <Typography variant="body2" color="text.disabled" component="span">
            &mdash;
          </Typography>
        ) : (
          <Tooltip
            title={signal.virtualInput ? `computed from ${signal.virtualInput}` : ""}
            placement="left"
          >
            <span>{formatAddress(signal.ecuAddress)}</span>
          </Tooltip>
        )}
      </TableCell>
      <TableCell align="right">{signal.size}</TableCell>
    </TableRow>
  );
});

export interface SignalTableProps {
  signals: SignalSummary[];
  groups: GroupInfo[];
  signalEvents: SignalEventMap;
  onSignalEventsChange: (value: SignalEventMap) => void;
}

/**
 * Searchable signal list with selection checkboxes and signal metadata columns
 */
function SignalTable({
  signals,
  groups,
  signalEvents,
  onSignalEventsChange,
}: SignalTableProps) {
  const [filter, setFilter] = useState("");
  // Lets the text field update immediately while the table renders at deferred priority.
  const deferredFilter = useDeferredValue(filter);
  const [sizeSort, setSizeSort] = useState<"asc" | "desc" | null>(null);
  const [groupFilter, setGroupFilter] = useState(ALL_GROUPS);
  const deferredGroupFilter = useDeferredValue(groupFilter);

  const hasGroups = groups.length > 0;
  const groupChoices = useMemo(() => groupOptions(groups), [groups]);
  const groupFilterSet = useMemo(
    () =>
      deferredGroupFilter === ALL_GROUPS
        ? null
        : groupAndDescendants(deferredGroupFilter, groups),
    [deferredGroupFilter, groups],
  );

  // Always lowercase
  const searchIndex = useMemo(
    () =>
      signals.map((signal) => ({
        signal,
        haystack:
          `${signal.name}\n${signal.description}\n${signal.unit}\n${signal.virtualInput ?? ""}\n${signal.virtualIssue?.label ?? ""}\n${signal.groups.join("\n")}`.toLowerCase(),
      })),
    [signals],
  );

  const filtered = useMemo(() => {
    const needle = deferredFilter.trim().toLowerCase();
    let result =
      needle === ""
        ? signals
        : searchIndex
            .filter((entry) => entry.haystack.includes(needle))
            .map((entry) => entry.signal);
    if (groupFilterSet) {
      result = result.filter((signal) =>
        signal.groups.some((group) => groupFilterSet.has(group)),
      );
    }
    return result;
  }, [signals, searchIndex, deferredFilter, groupFilterSet]);

  // Stable sort, so equal sizes keep their A2L order.
  const sorted = useMemo(() => {
    if (sizeSort === null) {
      return filtered;
    }
    const copy = [...filtered];
    copy.sort((a, b) =>
      sizeSort === "asc" ? a.size - b.size : b.size - a.size,
    );
    return copy;
  }, [filtered, sizeSort]);

  // Renders only the rows scrolled into view.
  const scrollRef = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({
    count: sorted.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
  const paddingTop = virtualRows.length > 0 ? virtualRows[0].start : 0;
  const paddingBottom =
    virtualRows.length > 0
      ? rowVirtualizer.getTotalSize() - virtualRows[virtualRows.length - 1].end
      : 0;
  const columnCount = 6 + (hasGroups ? 1 : 0);

  const selectable = useMemo(
    () => filtered.filter((signal) => !signal.virtualIssue),
    [filtered],
  );
  const rejectedCount = useMemo(
    () => signals.reduce((n, s) => (s.virtualIssue ? n + 1 : n), 0),
    [signals],
  );

  const selectedCount = Object.keys(signalEvents).length;
  const selectedInFiltered = useMemo(() => {
    let count = 0;
    for (const signal of selectable) {
      if (signal.name in signalEvents) {
        count++;
      }
    }
    return count;
  }, [selectable, signalEvents]);
  const allFilteredSelected =
    selectable.length > 0 && selectedInFiltered === selectable.length;

  // Read the current map through a ref so toggleSignal stays stable and doesn't invalidate every memoized row on each selection.
  const signalEventsRef = useRef(signalEvents);
  useEffect(() => {
    signalEventsRef.current = signalEvents;
  }, [signalEvents]);

  const toggleSignal = useCallback(
    (name: string) => {
      const next = { ...signalEventsRef.current };
      if (name in next) {
        delete next[name];
      } else {
        next[name] = "";
      }
      onSignalEventsChange(next);
    },
    [onSignalEventsChange],
  );

  const toggleAllFiltered = () => {
    const next = { ...signalEvents };
    if (allFilteredSelected) {
      for (const signal of selectable) {
        delete next[signal.name];
      }
    } else {
      for (const signal of selectable) {
        if (!(signal.name in next)) {
          next[signal.name] = "";
        }
      }
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
      <Box sx={{ display: "flex", alignItems: "center", gap: 2, mb: 1 }}>
        <TextField
          size="small"
          label="Search name, description or unit"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          sx={{ width: 300 }}
        />
        {hasGroups && (
          <Autocomplete
            size="small"
            sx={{ width: 240 }}
            options={groupChoices}
            getOptionLabel={(option) => option.name}
            isOptionEqualToValue={(option, value) => option.name === value.name}
            value={groupChoices.find((g) => g.name === groupFilter) ?? null}
            onChange={(_, value) => setGroupFilter(value?.name ?? ALL_GROUPS)}
            renderOption={({ key, ...props }, option) => (
              <li key={key} {...props} style={{ paddingLeft: 16 + option.depth * 16 }}>
                {option.name}
              </li>
            )}
            renderInput={(params) => (
              <TextField {...params} label="Group" placeholder="All groups" />
            )}
          />
        )}
        <Typography variant="body2" color="text.secondary">
          {selectedCount} of {signals.length - rejectedCount} selected
        </Typography>
        {rejectedCount > 0 && (
          <Typography variant="body2" color="warning.main">
            {rejectedCount} virtual{" "}
            {rejectedCount === 1 ? "signal" : "signals"} cannot be measured
          </Typography>
        )}
      </Box>
      <TableContainer
        ref={scrollRef}
        sx={{ flex: 1, minHeight: 0, border: 1, borderColor: "divider" }}
      >
        <Table size="small" stickyHeader sx={{ tableLayout: "fixed" }}>
          <TableHead>
            <TableRow>
              <TableCell padding="none" sx={{ width: COL_CHECKBOX_WIDTH, pl: 0.5 }}>
                <Checkbox
                  size="small"
                  checked={allFilteredSelected}
                  indeterminate={!allFilteredSelected && selectedInFiltered > 0}
                  onChange={toggleAllFiltered}
                />
              </TableCell>
              <TableCell sx={{ width: COL_SIGNAL_WIDTH }}>Signal</TableCell>
              {hasGroups && <TableCell sx={{ width: COL_GROUP_WIDTH }}>Group</TableCell>}
              <TableCell>Description</TableCell>
              <TableCell sx={{ width: COL_UNIT_WIDTH }}>Unit</TableCell>
              <TableCell align="right" sx={{ width: COL_ADDRESS_WIDTH }}>
                Address
              </TableCell>
              <TableCell
                align="right"
                sx={{ width: COL_SIZE_WIDTH }}
                sortDirection={sizeSort ?? false}
              >
                <TableSortLabel
                  active={sizeSort !== null}
                  direction={sizeSort ?? "asc"}
                  onClick={() =>
                    setSizeSort(sizeSort === "asc" ? "desc" : "asc")
                  }
                >
                  Size
                </TableSortLabel>
              </TableCell>
            </TableRow>
          </TableHead>
          <TableBody>
            {paddingTop > 0 && (
              <TableRow>
                <TableCell
                  colSpan={columnCount}
                  sx={{ padding: 0, border: 0, height: paddingTop }}
                />
              </TableRow>
            )}
            {virtualRows.map((virtualRow) => {
              const signal = sorted[virtualRow.index];
              return (
                <SignalRow
                  key={signal.name}
                  signal={signal}
                  checked={signal.name in signalEvents}
                  onToggle={toggleSignal}
                  showGroupColumn={hasGroups}
                  onGroupClick={setGroupFilter}
                />
              );
            })}
            {paddingBottom > 0 && (
              <TableRow>
                <TableCell
                  colSpan={columnCount}
                  sx={{ padding: 0, border: 0, height: paddingBottom }}
                />
              </TableRow>
            )}
          </TableBody>
        </Table>
      </TableContainer>
    </Box>
  );
}

export default SignalTable;
