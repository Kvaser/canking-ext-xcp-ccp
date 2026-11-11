# canking-gui (DAQ Configurator)

A CanKing workspace view, React and TypeScript: a five-step wizard from an A2L file to a configured[DAQ Decoder](../canking-service/README.md) node, plus DBC, config and t-script downloads.

## Behaviour

The view holds no A2L or protocol logic. It runs [a2l-daq](../a2l-daq/README.md) in the browser: the wasm component, transpiled by `jco` into `src/a2l-daq-wasm/`. 

The steps call it like this:

| Step | Calls |
|---|---|
| Load A2L | `inspect`: signals, events, protocols, bus settings, unread keywords |
| Select signals, Assign events | `build`, for the live DAQ list preview |
| Apply & export | `build-config` and `build-tscript`, with the CAN id overrides and prescalers from the DAQ list table |

Apply then adds a DAQ Decoder node behind the chosen source with `addDataProcessor`, or updates the node it added before with `updateDataProcessor`.

State is kept with the CanKing API's slices:

- **Project data**, saved with the CanKing project: `signalEvents` (selection and events), `startDaq`,
  `triggerConditions`, `tscriptOptions`, `protocol`, `processorName`, `processorNodeId`, `daqCanIdOverrides`, `daqPrescalers`. 
  Options added in a later version are absent from older projects, so read them with a default.
- **Session data**, gone when CanKing closes: `a2lFile`, `activeStep`, and the last `warnings`, `dbc`, `configJson`, `tscript`.

CanKing validates a data processor's config against the schema of its default config, which constrains the JSON; see [the contract](../protocol-core/README.md#config-json-contract).

## Layout

```
|-src/
  |-main.tsx, App.tsx      Entry point; App provides the CanKing context and the view's id
  |-WorkspaceView/
    |-index.tsx            The wizard: state, steps, Apply
    |-SignalTable.tsx      Step 2, virtualised signal list with search, groups and sorting
    |-EventAssignTable.tsx Step 3
    |-BusSection.tsx, TriggerSection.tsx, TScriptSection.tsx   Step 4
    |-DaqListSection.tsx   Step 5, CAN id and prescaler per DAQ list
    |-util.ts              Option types and defaults, parsing and formatting helpers
  |-a2l-daq-wasm/          jco output (generated)
|-user-guide.md            Shipped in the .ckext
```

## Develop

```sh
npm ci
npm run build:wasm     # build the a2l-daq component and transpile it; needs the Rust wasm target
npm run start          # vite dev server + CanKing in debug mode, loading this view live
npm run lint           # eslint and Prettier (npm run format fixes the formatting)
npm run package        # typecheck, vite build, npm pack -> daq-gui-<version>.tgz
```

`start` and `startpreview` launch `C:/Program Files/Kvaser CanKing/Kvaser CanKing.exe`, so they need Windows with
CanKing installed there. `npm run transpile:wasm` re-runs only the `jco` step on an existing build. The installable
`.ckext` is made from the `.tgz` by `./run.sh stage pack` at the repository root.
