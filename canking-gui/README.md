# DAQ Configurator (CanKing GUI extension)

A CanKing workspace view for setting up XCP/CCP DAQ measurements from an A2L file:

1. **Load A2L** - load an A2L, review the parsed signal/event counts, and choose the measurement protocol
2. **Select signals** - pick MEASUREMENT signals from a searchable list.
3. **Assign events** - map each selected signal to an event channel, or keep the event declared in the A2L.
4. **Configure** - the generated t-script's settings: an optional manual CAN bus override, optional signal trigger conditions, and t-script options.
5. **Apply & export** - set the data processor / `start_daq` option and push the generated configuration into the data
   processor node via `addDataProcessor`/`updateDataProcessor`. Three artefacts can be downloaded: the DBC, the raw config JSON, and a
   Memorator t-script (which uses the trigger conditions as its start-logger expression).

## How it works

All A2L parsing, DAQ list packing, and DBC/t-script generation happens in the browser through the [`a2l-daq`](../a2l-daq/README.md) Rust crate, compiled to
a WASI P2 component and transpiled to JS with `jco` into `src/a2l-daq-wasm/` (generated with `npm run build:wasm`, or `npm run transpile:wasm` to only re-run the
`jco` step on an already built `target/wasm32-wasip2/release/a2l_daq.wasm`).

State handling:

- **Project data**: signal/event selections, trigger conditions, t-script options, protocol, `startDaq`, DAQ list CAN id
  overrides (`daqCanIdOverrides`), processor name and node id.
- **Session data**: the A2L contents, wizard step, last warnings/DBC/config/t-script. 

## Development

```sh
npm install          # once
npm run start        # vite dev server + CanKing in debug mode
npm run startpreview # vite dev server + CanKing in production mode
npm run build:wasm   # rebuild the a2l-daq wasm component + jco transpile
npm run package      # npm pack (runs build via prepack) -> .tgz
```

`build:wasm` requires the Rust toolchain with the `wasm32-wasip2` target (see the [root README](../README.md#building)).
The compiled component is picked up from the workspace-level `target/` directory.

The installable `.ckext` is not produced here; it is made from that `.tgz` by `./run.sh stage pack` from the repo root.

`start`/`startpreview` launch CanKing (via the `canking` script) from `C:/Program Files/Kvaser CanKing/Kvaser CanKing.exe`,
so they expect a Windows machine with CanKing installed at that path.

The `@kvaser/canking-api` dependency is published on the public npm registry, but under Kvaser's EULA, not this
repository's MIT license.

## Info
- CanKing validates the configuration against a schema inferred from the data processor's default config. 
See the [root README](../README.md#the-config-json-contract) for the JSON contract rules this imposes.
