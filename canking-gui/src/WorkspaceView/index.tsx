import { startTransition, useCallback, useMemo, useRef, useState } from 'react';
import { useMeasurementSetup, useProjectDataSlice, useSessionDataSlice } from '@kvaser/canking-api/hooks';
import { IExtensionWorkspaceViewProps, SectionControl, SizedBox } from '@kvaser/canking-api/controls';
import { addDataProcessor, showMessageBox, updateDataProcessor } from '@kvaser/canking-api/ipc';
import { DataProcessorNodeProperties, SourceNodeProperties } from '@kvaser/canking-api/models';
import {
  Alert,
  Box,
  Button,
  Checkbox,
  Chip,
  FormControlLabel,
  MenuItem,
  Step,
  StepButton,
  Stepper,
  TextField,
  Typography,
  alpha,
} from '@mui/material';

import { generator } from '../a2l-daq-wasm/a2l_daq.js';
import {
  A2lSummary,
  DaqList,
  Diagnostic,
  EventAssignment,
  Protocol,
} from '../a2l-daq-wasm/interfaces/kvaser-a2l-daq-generator.js';

import SignalTable from './SignalTable';
import EventAssignTable from './EventAssignTable';
import BusSection from './BusSection';
import TriggerSection from './TriggerSection';
import TScriptSection from './TScriptSection';
import DaqListSection from './DaqListSection';
import {
  DEFAULT_TSCRIPT_OPTIONS,
  FORM_MAX_WIDTH,
  panelSx,
  errorText,
  formatBitrate,
  parseCanId,
  parseNonNegativeInt,
  parseOptionalPositiveInt,
  parsePrescaler,
  readA2lText,
  SignalEventMap,
  TriggerCondition,
  TScriptOptions,
} from './util';

const DIALOG_TITLE = 'DAQ Configurator';

/** The filled-in conditions */
const toWitConditions = (conditions: TriggerCondition[]) =>
  conditions.filter(c => c.signal !== '').map(c => ({ signal: c.signal, op: c.operator, value: c.value }));

const STEPS = ['Load A2L', 'Select signals', 'Assign events', 'Configure t-script', 'Apply & export'];

interface A2lFile {
  name: string;
  text: string;
}
function diagnosticText(d: Diagnostic): string {
  switch (d.tag) {
    case 'unplaced-signal': {
      const event = d.val.event ? `event '${d.val.event}'` : 'no event';
      return (
        `Signal '${d.val.name}' could not be placed in any DAQ list ` +
        `(${event}, ${d.val.size} bytes, address 0x${d.val.ecuAddress.toString(16).toUpperCase()})`
      );
    }
    case 'dropped-signal':
      return (
        `Selected signal '${d.val}' is not in the parsed A2L ` +
        `(unknown name or unparsable MEASUREMENT block) and was dropped`
      );
    case 'unmeasurable-signal':
      return `Selected signal '${d.val.name}' cannot be measured: ${d.val.issue.detail}`;
    case 'unknown-signal':
      return `An assignment references unknown signal '${d.val}'`;
    case 'colliding-names':
      return (
        `Signals ${d.val.names.map(n => `'${n}'`).join(', ')} all become ` +
        `'${d.val.sanitized}' in the DBC and t-script, so they cannot be told apart`
      );
    case 'unsupported-conversion':
      return (
        `Signal '${d.val.name}' uses conversion '${d.val.conversion}' (${d.val.reason}), ` +
        `which is not supported: its values are raw instead of physical`
      );
  }
}

function WorkspaceView({ id, height, width }: IExtensionWorkspaceViewProps) {
  const measurementSetup = useMeasurementSetup();

  // Save to session data
  const [a2lFile, setA2lFile, a2lLoaded] = useSessionDataSlice<A2lFile>(id, 'a2lFile', { name: '', text: '' });

  // Configuration state stored in the CanKing project file.
  const [signalEvents, setSignalEvents, signalEventsLoaded] = useProjectDataSlice<SignalEventMap>(
    id,
    'signalEvents',
    {},
  );
  const [startDaq, setStartDaq, startDaqLoaded] = useProjectDataSlice<boolean>(id, 'startDaq', false);
  const [conditions, setConditions, conditionsLoaded] = useProjectDataSlice<TriggerCondition[]>(
    id,
    'triggerConditions',
    [],
  );
  const [stopConditions, setStopConditions, stopConditionsLoaded] = useProjectDataSlice<TriggerCondition[]>(
    id,
    'stopConditions',
    [],
  );
  const [tscriptOptions, setTscriptOptions, tscriptOptionsLoaded] = useProjectDataSlice<TScriptOptions>(
    id,
    'tscriptOptions',
    DEFAULT_TSCRIPT_OPTIONS,
  );
  const [protocol, setProtocol, protocolLoaded] = useProjectDataSlice<Protocol>(id, 'protocol', 'xcp');
  const [processorName, setProcessorName, processorNameLoaded] = useProjectDataSlice<string>(
    id,
    'processorName',
    'daq_decoder',
  );
  const [processorNodeId, setProcessorNodeId, processorNodeIdLoaded] = useProjectDataSlice<string>(
    id,
    'processorNodeId',
    '',
  );
  const [canIdOverrides, setCanIdOverrides, canIdOverridesLoaded] = useProjectDataSlice<Record<string, string>>(
    id,
    'daqCanIdOverrides',
    {},
  );
  const [prescalers, setPrescalers, prescalersLoaded] = useProjectDataSlice<Record<string, string>>(
    id,
    'daqPrescalers',
    {},
  );

  const [warnings, setWarnings, warningsLoaded] = useSessionDataSlice<string[]>(id, 'warnings', []);
  const [dbc, setDbc, dbcLoaded] = useSessionDataSlice<string>(id, 'dbc', '');
  const [configJson, setConfigJson, configJsonLoaded] = useSessionDataSlice<string>(id, 'configJson', '');
  const [tscript, setTscript, tscriptLoaded] = useSessionDataSlice<string>(id, 'tscript', '');
  const [activeStep, setActiveStep, activeStepLoaded] = useSessionDataSlice<number>(id, 'activeStep', 0);
  const [sourceId, setSourceId] = useState('');

  const dataLoaded =
    a2lLoaded &&
    signalEventsLoaded &&
    startDaqLoaded &&
    conditionsLoaded &&
    stopConditionsLoaded &&
    tscriptOptionsLoaded &&
    protocolLoaded &&
    processorNameLoaded &&
    processorNodeIdLoaded &&
    canIdOverridesLoaded &&
    prescalersLoaded &&
    warningsLoaded &&
    dbcLoaded &&
    configJsonLoaded &&
    tscriptLoaded &&
    activeStepLoaded;

  const fileInputRef = useRef<HTMLInputElement>(null);
  const exportRef = useRef<HTMLDivElement>(null);

  // Parse the A2L (in wasm) whenever its content changes.
  const summary = useMemo<{ ok: A2lSummary; error?: never } | { ok?: never; error: string } | null>(() => {
    if (a2lFile.text === '') {
      return null;
    }
    try {
      return { ok: generator.inspect(a2lFile.text) };
    } catch (e) {
      return { error: errorText(e) };
    }
  }, [a2lFile.text]);

  const measurableSignals = useMemo(() => summary?.ok?.signals.filter(s => !s.virtualIssue) ?? [], [summary]);

  const activeSignalEvents = useMemo(() => {
    if (!summary?.ok) {
      return signalEvents;
    }
    const known = new Set(measurableSignals.map(s => s.name));
    const hasStale = Object.keys(signalEvents).some(name => !known.has(name));
    if (!hasStale) {
      return signalEvents;
    }
    const next: SignalEventMap = {};
    for (const [name, event] of Object.entries(signalEvents)) {
      if (known.has(name)) {
        next[name] = event;
      }
    }
    return next;
  }, [signalEvents, summary, measurableSignals]);

  // Protocols the loaded A2L declares, in canonical (preferred-first) order.
  const detectedProtocols = useMemo(() => summary?.ok?.protocols ?? [], [summary]);
  const activeProtocol = useMemo<Protocol>(() => {
    if (detectedProtocols.includes(protocol)) {
      return protocol;
    }
    return detectedProtocols[0] ?? 'xcp';
  }, [protocol, detectedProtocols]);

  // A2L-declared bus params for the active protocol
  const detectedTransport = useMemo(
    () => summary?.ok?.transports.find(t => t.protocol === activeProtocol),
    [summary, activeProtocol],
  );
  // XCP sends the prescaler as a byte, CCP as a word
  const prescalerMax = detectedTransport?.prescaler ? (activeProtocol === 'xcp' ? 0xff : 0xffff) : undefined;

  const loadFile = useCallback(
    async (file: File) => {
      const text = await readA2lText(file);
      setA2lFile({ name: file.name, text });
      setWarnings([]);
      setDbc('');
      setConfigJson('');
      setTscript('');
      setActiveStep(0);
    },
    [setA2lFile, setWarnings, setDbc, setConfigJson, setTscript, setActiveStep],
  );

  const selectedSignals = useMemo(() => Object.keys(activeSignalEvents), [activeSignalEvents]);

  // Per-signal assignments grouped by event; "" means keep the A2L default.
  const eventAssignments = useMemo<EventAssignment[]>(() => {
    const byEvent = new Map<string, string[]>();
    for (const [signal, event] of Object.entries(activeSignalEvents)) {
      if (event !== '') {
        byEvent.set(event, [...(byEvent.get(event) ?? []), signal]);
      }
    }
    return [...byEvent.entries()].map(([event, signals]) => ({ event, signals }));
  }, [activeSignalEvents]);

  // Live preview of the packed DAQ lists
  const daqListPreview = useMemo<DaqList[]>(() => {
    if (a2lFile.text === '' || selectedSignals.length === 0) {
      return [];
    }
    try {
      return generator.build(a2lFile.text, a2lFile.name, selectedSignals, eventAssignments, activeProtocol).daqLists;
    } catch {
      return [];
    }
  }, [a2lFile, selectedSignals, eventAssignments, activeProtocol]);

  const goToStep = useCallback((step: number) => startTransition(() => setActiveStep(step)), [setActiveStep]);

  const onApply = useCallback(async () => {
    try {
      const assignments = eventAssignments;

      const dynamicIds = new Set(daqListPreview.filter(l => l.canId.tag === 'dynamic').map(l => l.id));
      const warningLines: string[] = [];
      const daqCanIdOverrides: [number, number][] = [];
      for (const list of daqListPreview) {
        if (!dynamicIds.has(list.id)) continue;
        const raw = canIdOverrides[String(list.id)];
        if (raw === undefined || raw.trim() === '') continue;
        const parsed = parseCanId(raw);
        if (parsed === null) {
          warningLines.push(`DAQ list ${list.id}: invalid CAN id "${raw}", using the shared default instead`);
        } else {
          daqCanIdOverrides.push([list.id, parsed]);
        }
      }

      const daqPrescalers: [number, number][] = [];
      for (const list of prescalerMax !== undefined ? daqListPreview : []) {
        const raw = prescalers[String(list.id)] ?? '';
        const parsed = parsePrescaler(raw, prescalerMax ?? 1);
        if (parsed === null) {
          warningLines.push(`DAQ list ${list.id}: invalid prescaler "${raw}", using 1 instead`);
        } else if (parsed > 1) {
          daqPrescalers.push([list.id, parsed]);
        }
      }

      const out = generator.buildConfig(
        a2lFile.text,
        a2lFile.name,
        selectedSignals,
        assignments,
        startDaq,
        daqCanIdOverrides,
        daqPrescalers,
        activeProtocol,
      );
      warningLines.push(...out.warnings.map(diagnosticText));
      setDbc(out.dbc);
      setConfigJson(out.configJson);

      // Try to generate t-script
      const channelIndex = parseNonNegativeInt(tscriptOptions.channelIndex);
      const pretriggerMs = parseNonNegativeInt(tscriptOptions.pretriggerMs);
      const timerMs = tscriptOptions.timerStop ? parseNonNegativeInt(tscriptOptions.timerMs) : undefined;
      const maxCaptures = tscriptOptions.timerStop
        ? parseOptionalPositiveInt(tscriptOptions.maxCaptures ?? DEFAULT_TSCRIPT_OPTIONS.maxCaptures ?? '')
        : undefined;
      const daqWatchdogMs = parseOptionalPositiveInt(
        tscriptOptions.watchdogMs ?? DEFAULT_TSCRIPT_OPTIONS.watchdogMs ?? '',
      );
      const stopCanIdText = (tscriptOptions.stopCanId ?? '').trim();
      const stopCanId = stopCanIdText === '' ? undefined : parseCanId(stopCanIdText);
      if (
        channelIndex === null ||
        pretriggerMs === null ||
        timerMs === null ||
        maxCaptures === null ||
        daqWatchdogMs === null ||
        stopCanId === null
      ) {
        setTscript('');
        warningLines.push('t-script not generated: invalid t-script options (see the T-script section)');
      } else {
        try {
          const tscriptOut = generator.buildTscript(
            a2lFile.text,
            a2lFile.name,
            selectedSignals,
            assignments,
            {
              channelIndex,
              pretriggerMs,
              timerMs,
              maxCaptures,
              daqWatchdogMs,
              conditions: toWitConditions(conditions),
              stopConditions: toWitConditions(stopConditions),
              stopCanId,
              bus: tscriptOptions.bus.enabled
                ? {
                    mode: tscriptOptions.bus.mode,
                    bitrate: tscriptOptions.bus.bitrate,
                    dataBitrate: tscriptOptions.bus.mode === 'can-fd' ? tscriptOptions.bus.dataBitrate : undefined,
                  }
                : undefined,
              signalAccess: tscriptOptions.standalone ? 'inline' : 'dbc',
            },
            daqCanIdOverrides,
            daqPrescalers,
            activeProtocol,
          );
          setTscript(tscriptOut.tscript);
          for (const line of tscriptOut.warnings.map(diagnosticText)) {
            if (!warningLines.includes(line)) {
              warningLines.push(line);
            }
          }
        } catch (e) {
          setTscript('');
          warningLines.push(`t-script not generated: ${errorText(e)}`);
        }
      }
      setWarnings(warningLines);
      exportRef.current?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

      const existing =
        measurementSetup?.dataProcessors.find(
          (dp: DataProcessorNodeProperties) => dp.baseNode?.identifier === processorNodeId,
        ) ??
        measurementSetup?.dataProcessors.find(
          (dp: DataProcessorNodeProperties) => dp.extensionSettings?.extensionName === processorName,
        );

      if (existing) {
        await updateDataProcessor({
          ...existing,
          baseNode: existing.baseNode && {
            ...existing.baseNode,
            description: [`A2L: ${a2lFile.name}`],
          },
          extensionSettings: {
            extensionName: processorName,
            configuration: out.configJson,
          },
        });
        setProcessorNodeId(existing.baseNode?.identifier ?? '');
      } else {
        const identifier = crypto.randomUUID();
        const node: DataProcessorNodeProperties = {
          baseNode: {
            identifier,
            name: 'DAQ Decoder',
            description: [`A2L: ${a2lFile.name}`],
            disabled: false,
            readOnlyConnections: false,
            readOnlyProperties: false,
          },
          connectedNodes: [],
          extensionSettings: {
            extensionName: processorName,
            configuration: out.configJson,
          },
        };
        await addDataProcessor(node, sourceId || undefined);
        setProcessorNodeId(identifier);
      }

      const summaryLines = [
        `Configuration sent to data processor '${processorName}'.`,
        `${selectedSignals.length} signals in ${out.configJson.length} bytes of config.`,
      ];
      if (warningLines.length > 0) {
        summaryLines.push(`${warningLines.length} warning(s) - see the view.`);
      }
      await showMessageBox(summaryLines.join('\n'), DIALOG_TITLE);
    } catch (e) {
      await showMessageBox(errorText(e), DIALOG_TITLE);
    }
  }, [
    a2lFile,
    selectedSignals,
    eventAssignments,
    daqListPreview,
    canIdOverrides,
    prescalers,
    prescalerMax,
    activeProtocol,
    startDaq,
    conditions,
    stopConditions,
    tscriptOptions,
    measurementSetup,
    processorName,
    processorNodeId,
    setProcessorNodeId,
    setDbc,
    setConfigJson,
    setTscript,
    setWarnings,
    sourceId,
  ]);

  // Base name for downloaded artefacts, derived from the loaded A2L.
  const downloadBaseName = a2lFile.name.replace(/\.a2l$/i, '') || 'daq';

  const downloadText = useCallback((fileName: string, text: string) => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = fileName;
    link.click();
    URL.revokeObjectURL(url);
  }, []);

  if (!dataLoaded) {
    return null;
  }

  const canApply =
    a2lFile.text !== '' && summary?.ok !== undefined && selectedSignals.length > 0 && processorName.trim() !== '';

  const hasSummary = summary?.ok !== undefined;
  const hasSignals = selectedSignals.length > 0;

  const stepDisabled = (index: number) => (index >= 1 && !hasSummary) || (index >= 2 && !hasSignals);
  const canGoNext = activeStep === 0 ? hasSummary : hasSignals;
  const isLastStep = activeStep === STEPS.length - 1;
  const blockedReason = !hasSummary
    ? 'Load an A2L file to continue'
    : !hasSignals && activeStep > 0
      ? 'Select at least one signal to continue'
      : isLastStep && processorName.trim() === ''
        ? 'Enter the service extension name'
        : undefined;
  const compactSteps = width < 760;

  const footer = (
    <Box
      sx={theme => ({
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: 2,
        mt: 1,
        pt: 1.5,
        borderTop: `1px solid ${alpha(theme.palette.text.primary, 0.12)}`,
        // Sizes as in the OBD2 extension's toolbar buttons
        '& .MuiButton-root': { minWidth: 112, height: 34, whiteSpace: 'nowrap' },
      })}
    >
      {/* Together on the left, under the content, so they stay in view on a wide pane */}
      <Button variant="outlined" disabled={activeStep === 0} onClick={() => goToStep(activeStep - 1)}>
        Back
      </Button>
      {isLastStep ? (
        <Button variant="contained" disabled={!canApply} onClick={() => void onApply()}>
          Apply to data processor
        </Button>
      ) : (
        <Button variant="contained" disabled={!canGoNext} onClick={() => goToStep(activeStep + 1)}>
          Next
        </Button>
      )}
      {blockedReason !== undefined && (
        <Typography variant="caption" color="text.secondary">
          {blockedReason}
        </Typography>
      )}
    </Box>
  );

  return (
    <SizedBox height={height} width={width} padding={4}>
      <Box
        sx={{
          height: '100%',
          display: 'flex',
          flexDirection: 'column',
          pr: 1,
        }}
        onDragOver={e => e.preventDefault()}
        onDrop={e => {
          e.preventDefault();
          const file = e.dataTransfer.files[0];
          if (file) {
            void loadFile(file);
          }
        }}
      >
        <Stepper
          nonLinear
          activeStep={activeStep}
          sx={{ mb: 2, flexShrink: 0, '& .MuiStepLabel-label': { whiteSpace: 'nowrap' } }}
        >
          {STEPS.map((label, index) => (
            <Step key={label} completed={index < activeStep}>
              <StepButton onClick={() => goToStep(index)} disabled={stepDisabled(index)} aria-label={label}>
                {compactSteps && index !== activeStep ? '' : label}
              </StepButton>
            </Step>
          ))}
        </Stepper>

        <Box
          sx={{
            flex: 1,
            minHeight: 0,
            display: 'flex',
            flexDirection: 'column',
            overflowY: 'auto',
          }}
        >
          {/* Step 1 Load A2L, review info, choose protocol */}
          {activeStep === 0 && (
            <>
              <SectionControl text="A2L file">
                <Box
                  sx={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 2,
                    my: 2,
                    maxWidth: 560,
                  }}
                >
                  <Box sx={{ display: 'flex', alignItems: 'center', gap: 2 }}>
                    <Button
                      variant={a2lFile.name === '' ? 'contained' : 'outlined'}
                      onClick={() => fileInputRef.current?.click()}
                      sx={{ whiteSpace: 'nowrap', flexShrink: 0 }}
                    >
                      Load A2L…
                    </Button>
                    <input
                      ref={fileInputRef}
                      type="file"
                      accept=".a2l,.txt"
                      hidden
                      onChange={e => {
                        const file = e.target.files?.[0];
                        if (file) {
                          void loadFile(file);
                        }
                        e.target.value = '';
                      }}
                    />
                    <Typography variant="body2" color="text.secondary">
                      {a2lFile.name === ''
                        ? 'No file loaded. Pick a file or drop one here. ' + 'It is kept in memory only.'
                        : summary?.ok
                          ? 'File parsed. Review the details below and pick the protocol.'
                          : a2lFile.name}
                    </Typography>
                  </Box>

                  {summary?.error !== undefined && <Alert severity="error">{summary.error}</Alert>}

                  {summary?.ok && (
                    <Box
                      sx={theme => ({
                        ...panelSx(theme),
                        p: 2,
                        display: 'grid',
                        gridTemplateColumns: 'auto 1fr',
                        columnGap: 3,
                        rowGap: 1,
                        alignItems: 'center',
                      })}
                    >
                      <Typography variant="body2" color="text.secondary">
                        File
                      </Typography>
                      <Typography variant="body2" sx={{ overflowWrap: 'anywhere' }}>
                        {a2lFile.name}
                      </Typography>

                      <Typography variant="body2" color="text.secondary">
                        Protocols
                      </Typography>
                      <Box sx={{ display: 'flex', gap: 1, flexWrap: 'wrap' }}>
                        {detectedProtocols.map(p => (
                          <Chip
                            key={p}
                            size="small"
                            label={p.toUpperCase()}
                            color={p === activeProtocol ? 'primary' : 'default'}
                            variant={p === activeProtocol ? 'filled' : 'outlined'}
                          />
                        ))}
                      </Box>

                      <Typography variant="body2" color="text.secondary">
                        Signals
                      </Typography>
                      <Typography variant="body2">{measurableSignals.length}</Typography>

                      <Typography variant="body2" color="text.secondary">
                        Events
                      </Typography>
                      <Typography variant="body2">{summary.ok.events.length}</Typography>

                      <Typography variant="body2" color="text.secondary">
                        Bus (from A2L)
                      </Typography>
                      <Typography variant="body2">
                        {detectedTransport
                          ? [
                              detectedTransport.canFd ? 'CAN FD' : 'CAN',
                              detectedTransport.bitrate !== undefined
                                ? `${formatBitrate(detectedTransport.bitrate)} arb`
                                : undefined,
                              detectedTransport.canFd && detectedTransport.dataBitrate !== undefined
                                ? `${formatBitrate(detectedTransport.dataBitrate)} data`
                                : undefined,
                            ]
                              .filter(Boolean)
                              .join(' · ')
                          : 'not declared'}
                      </Typography>
                    </Box>
                  )}

                  {summary?.ok && summary.ok.unread.length > 0 && (
                    <Alert severity="warning">
                      This A2L uses keywords the configurator does not read, so some outputs may be wrong:
                      <ul style={{ margin: 0, paddingLeft: 20 }}>
                        {summary.ok.unread.map(u => (
                          <li key={u.keyword}>
                            <code>{u.keyword}</code>
                            {u.count > 1 ? ` (${u.count}x)` : ''}: {u.effect}
                          </li>
                        ))}
                      </ul>
                    </Alert>
                  )}

                  {summary?.ok && detectedProtocols.length > 0 && (
                    <TextField
                      size="small"
                      select
                      label="Protocol"
                      value={activeProtocol}
                      onChange={e => setProtocol(e.target.value as Protocol)}
                      sx={{ width: 240 }}
                      helperText={
                        detectedProtocols.length > 1
                          ? 'Multiple protocols detected. Choose one'
                          : 'Detected from the A2L IF_DATA block'
                      }
                    >
                      {detectedProtocols.map(p => (
                        <MenuItem key={p} value={p}>
                          {p.toUpperCase()}
                        </MenuItem>
                      ))}
                    </TextField>
                  )}

                  {summary?.ok && (
                    <Typography variant="caption" color="text.secondary">
                      Loading a different A2L resets the steps below.
                    </Typography>
                  )}
                </Box>
              </SectionControl>
            </>
          )}

          {/* Step 2 Select signals */}
          {activeStep === 1 && summary?.ok && (
            <>
              <SignalTable
                signals={summary.ok.signals}
                groups={summary.ok.groups}
                signalEvents={activeSignalEvents}
                onSignalEventsChange={setSignalEvents}
              />
            </>
          )}

          {/* Step 3 Assign events */}
          {activeStep === 2 && summary?.ok && (
            <>
              <EventAssignTable
                events={summary.ok.events}
                signalEvents={activeSignalEvents}
                onSignalEventsChange={setSignalEvents}
              />
            </>
          )}

          {/* Step 4 Configure the generated t-script */}
          {activeStep === 3 && (
            <Box sx={{ maxWidth: FORM_MAX_WIDTH }}>
              <SectionControl text="CAN bus">
                <BusSection
                  bus={tscriptOptions.bus}
                  onBusChange={bus => setTscriptOptions({ ...tscriptOptions, bus })}
                  detected={detectedTransport}
                />
              </SectionControl>

              <SectionControl text="Start trigger">
                <TriggerSection
                  signalNames={selectedSignals}
                  conditions={conditions}
                  onConditionsChange={setConditions}
                  description="Starts the logger. Conditions are combined with AND (e.g. Rpm > 2500)."
                />
              </SectionControl>

              <SectionControl text="Stop">
                <TriggerSection
                  signalNames={selectedSignals}
                  conditions={stopConditions}
                  onConditionsChange={setStopConditions}
                  description="Stops logging and DAQ for good when all conditions hold, or when a frame with the stop CAN id arrives"
                />
                <TextField
                  size="small"
                  label="Stop frame CAN id"
                  placeholder="none"
                  value={tscriptOptions.stopCanId ?? ''}
                  error={
                    (tscriptOptions.stopCanId ?? '').trim() !== '' &&
                    parseCanId(tscriptOptions.stopCanId ?? '') === null
                  }
                  helperText="Hex with 0x, or decimal"
                  onChange={e => setTscriptOptions({ ...tscriptOptions, stopCanId: e.target.value })}
                  sx={{ width: 240, mt: 2, mb: 1 }}
                />
                <Typography variant="caption" color="text.secondary" component="p">
                  DAQ also stops after the last capture and when the script is stopped. If the logger loses power or is
                  unplugged while DAQ runs, the ECU keeps sending until something else stops it or it restarts.
                </Typography>
              </SectionControl>

              <SectionControl text="Logging">
                <TScriptSection options={tscriptOptions} onOptionsChange={setTscriptOptions} />
              </SectionControl>
            </Box>
          )}

          {/* Step 5 Apply to the data processor and export artefacts */}
          {activeStep === 4 && (
            <Box sx={{ maxWidth: FORM_MAX_WIDTH }}>
              <SectionControl text="Data processor">
                <Box sx={{ display: 'flex', flexDirection: 'column', gap: 2, my: 1 }}>
                  <TextField
                    size="small"
                    label="Service extension name"
                    helperText={
                      //TODO select among predefined extensions
                      "The name in the installed extension's ck-ext-info.json"
                    }
                    value={processorName}
                    onChange={e => setProcessorName(e.target.value)}
                    sx={{ width: 460, maxWidth: '100%' }}
                  />
                  {processorNodeId === '' && (
                    <TextField
                      size="small"
                      select
                      label="Connect after source"
                      value={sourceId}
                      onChange={e => setSourceId(e.target.value)}
                      sx={{ width: 460, maxWidth: '100%' }}
                      helperText="Where to insert the new data processor node"
                    >
                      <MenuItem value="">
                        <em>Not connected</em>
                      </MenuItem>
                      {(measurementSetup?.sources ?? []).map((source: SourceNodeProperties) => (
                        <MenuItem key={source.baseNode?.identifier ?? ''} value={source.baseNode?.identifier ?? ''}>
                          {source.baseNode?.name ?? 'Unnamed source'}
                        </MenuItem>
                      ))}
                    </TextField>
                  )}
                  <Box>
                    <FormControlLabel
                      control={<Checkbox checked={startDaq} onChange={e => setStartDaq(e.target.checked)} />}
                      label="Start DAQ on the ECU at measurement start"
                    />
                    <Typography variant="caption" color="text.secondary" component="p">
                      The data processor then sends the {activeProtocol.toUpperCase()} init sequence to the ECU when a
                      measurement starts.
                    </Typography>
                  </Box>
                </Box>
              </SectionControl>

              {daqListPreview.length > 0 && (
                <SectionControl text="DAQ lists">
                  <Box sx={{ display: 'flex', flexDirection: 'column', gap: 1, my: 1 }}>
                    <DaqListSection
                      daqLists={daqListPreview}
                      overrides={canIdOverrides}
                      onOverridesChange={setCanIdOverrides}
                      prescalerMax={prescalerMax}
                      prescalers={prescalers}
                      onPrescalersChange={setPrescalers}
                    />
                    {activeProtocol === 'xcp' && (
                      <Typography variant="caption" color="text.secondary">
                        Assigned ids (SET_DAQ_ID) and prescalers are sent with the init sequence, so they only take
                        effect with &quot;Start DAQ on the ECU&quot;.
                      </Typography>
                    )}
                  </Box>
                </SectionControl>
              )}

              <SectionControl text="Export">
                <Box ref={exportRef} sx={{ display: 'flex', flexDirection: 'column', gap: 1, my: 1 }}>
                  <Box
                    sx={{ display: 'flex', gap: 2, flexWrap: 'wrap', '& .MuiButton-root': { whiteSpace: 'nowrap' } }}
                  >
                    <Button
                      variant="outlined"
                      disabled={dbc === ''}
                      onClick={() => downloadText(`${downloadBaseName}.dbc`, dbc)}
                    >
                      Download DBC
                    </Button>
                    <Button
                      variant="outlined"
                      disabled={configJson === ''}
                      onClick={() => downloadText(`${downloadBaseName}.config.json`, configJson)}
                    >
                      Download config JSON
                    </Button>
                    <Button
                      variant="outlined"
                      disabled={tscript === ''}
                      onClick={() => downloadText(`${downloadBaseName}.t`, tscript)}
                    >
                      Download t-script
                    </Button>
                  </Box>
                  {dbc === '' && (
                    <Typography variant="caption" color="text.secondary">
                      Apply to the data processor first; that also generates the files.
                    </Typography>
                  )}
                  {warnings.length > 0 && (
                    <Alert severity="warning">
                      {warnings.map((w, i) => (
                        <div key={i}>{w}</div>
                      ))}
                    </Alert>
                  )}
                </Box>
              </SectionControl>
            </Box>
          )}
        </Box>
        {footer}
      </Box>
    </SizedBox>
  );
}

export default WorkspaceView;
