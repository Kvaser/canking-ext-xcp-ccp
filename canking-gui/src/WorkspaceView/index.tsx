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
import DaqCanIdSection from './DaqCanIdSection';
import {
  DEFAULT_TSCRIPT_OPTIONS,
  errorText,
  formatBitrate,
  parseCanId,
  parseNonNegativeInt,
  readA2lText,
  SignalEventMap,
  TriggerCondition,
  TScriptOptions,
} from './util';

const DIALOG_TITLE = 'DAQ Configurator';

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
    tscriptOptionsLoaded &&
    protocolLoaded &&
    processorNameLoaded &&
    processorNodeIdLoaded &&
    canIdOverridesLoaded &&
    warningsLoaded &&
    dbcLoaded &&
    configJsonLoaded &&
    tscriptLoaded &&
    activeStepLoaded;

  const fileInputRef = useRef<HTMLInputElement>(null);

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

      const out = generator.buildConfig(
        a2lFile.text,
        a2lFile.name,
        selectedSignals,
        assignments,
        startDaq,
        daqCanIdOverrides,
        activeProtocol,
      );
      warningLines.push(...out.warnings.map(diagnosticText));
      setDbc(out.dbc);
      setConfigJson(out.configJson);

      // Try to generate t-script
      const channelIndex = parseNonNegativeInt(tscriptOptions.channelIndex);
      const pretriggerMs = parseNonNegativeInt(tscriptOptions.pretriggerMs);
      const timerMs = tscriptOptions.timerStop ? parseNonNegativeInt(tscriptOptions.timerMs) : undefined;
      if (channelIndex === null || pretriggerMs === null || timerMs === null) {
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
              conditions: conditions
                .filter(c => c.signal !== '')
                .map(c => ({
                  signal: c.signal,
                  op: c.operator,
                  value: c.value,
                })),
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
    activeProtocol,
    startDaq,
    conditions,
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

  const stepNavigation = (
    <Box sx={{ display: 'flex', gap: 2, mt: 2 }}>
      <Button disabled={activeStep === 0} onClick={() => goToStep(activeStep - 1)}>
        Back
      </Button>
      {activeStep < STEPS.length - 1 && (
        <Button variant="contained" disabled={!canGoNext} onClick={() => goToStep(activeStep + 1)}>
          Next
        </Button>
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
        <Stepper nonLinear activeStep={activeStep} sx={{ mb: 2 }}>
          {STEPS.map((label, index) => (
            <Step key={label} completed={false}>
              <StepButton onClick={() => goToStep(index)} disabled={stepDisabled(index)}>
                {label}
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
                    <Button variant="contained" onClick={() => fileInputRef.current?.click()}>
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
                      sx={{
                        p: 2,
                        border: 1,
                        borderColor: 'divider',
                        borderRadius: 1,
                        display: 'grid',
                        gridTemplateColumns: 'auto 1fr',
                        columnGap: 3,
                        rowGap: 1,
                        alignItems: 'center',
                      }}
                    >
                      <Typography variant="body2" color="text.secondary">
                        File
                      </Typography>
                      <Typography variant="body2" sx={{ wordBreak: 'break-all' }}>
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
                      sx={{ width: 220 }}
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
              {stepNavigation}
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
              {stepNavigation}
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
              {stepNavigation}
            </>
          )}

          {/* Step 4 Configure the generated t-script */}
          {activeStep === 3 && (
            <>
              <SectionControl text="T-script · CAN bus">
                <BusSection
                  bus={tscriptOptions.bus}
                  onBusChange={bus => setTscriptOptions({ ...tscriptOptions, bus })}
                  detected={detectedTransport}
                />
              </SectionControl>

              <SectionControl text="T-script · Trigger">
                <TriggerSection
                  signalNames={selectedSignals}
                  conditions={conditions}
                  onConditionsChange={setConditions}
                />
              </SectionControl>

              <SectionControl text="T-script · Logging options">
                <TScriptSection options={tscriptOptions} onOptionsChange={setTscriptOptions} />
              </SectionControl>
              {stepNavigation}
            </>
          )}

          {/* Step 5 Apply to the data processor and export artefacts */}
          {activeStep === 4 && (
            <>
              <SectionControl text="Data processor">
                <Box
                  sx={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 2,
                    my: 1,
                  }}
                >
                  <TextField
                    size="small"
                    label="Service extension name"
                    helperText={
                      //TODO select among predefined extensions
                      'The registered extension name, see ck-ext-info.json of the installed extension'
                    }
                    value={processorName}
                    onChange={e => setProcessorName(e.target.value)}
                    sx={{ width: 460 }}
                  />
                  {processorNodeId === '' && (
                    <TextField
                      size="small"
                      select
                      label="Connect after source"
                      value={sourceId}
                      onChange={e => setSourceId(e.target.value)}
                      sx={{ width: 460 }}
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
                  <FormControlLabel
                    control={<Checkbox checked={startDaq} onChange={e => setStartDaq(e.target.checked)} />}
                    label="Start DAQ on the ECU at measurement start"
                  />
                  <Typography variant="caption" color="text.secondary" sx={{ mt: -1 }}>
                    When enabled, the extension sends the {activeProtocol.toUpperCase()} init/start-DAQ command sequence
                    to the ECU when a measurement starts.
                  </Typography>
                  {daqListPreview.length > 0 && (
                    <Box>
                      <Typography variant="subtitle2" sx={{ mb: 0.5 }}>
                        DAQ list CAN identifiers
                      </Typography>
                      <DaqCanIdSection
                        daqLists={daqListPreview}
                        overrides={canIdOverrides}
                        onOverridesChange={setCanIdOverrides}
                      />
                      {activeProtocol === 'xcp' && (
                        <Typography variant="caption" color="text.secondary">
                          Assigned ids above are sent via SET_DAQ_ID as part of the init sequence, and only take effect
                          once &quot;Start DAQ on the ECU&quot; is enabled.
                        </Typography>
                      )}
                    </Box>
                  )}
                  <Box sx={{ display: 'flex', gap: 2 }}>
                    <Button variant="contained" disabled={!canApply} onClick={() => void onApply()}>
                      Apply to data processor
                    </Button>
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
                  {warnings.length > 0 && (
                    <Alert severity="warning">
                      {warnings.map((w, i) => (
                        <div key={i}>{w}</div>
                      ))}
                    </Alert>
                  )}
                </Box>
              </SectionControl>
              {stepNavigation}
            </>
          )}
        </Box>
      </Box>
    </SizedBox>
  );
}

export default WorkspaceView;
