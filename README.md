# DAQ Configurator

Sets up XCP and CCP DAQ measurements on CAN from an A2L file. The repository builds two Kvaser CanKing extensions, which need **Kvaser CanKing 7.6 or later**, and a command-line tool:

- **DAQ Configurator** ([canking-gui](canking-gui/README.md)): a CanKing workspace view. Load an A2L, pick signals and
events, and apply the result to a data processor; export a DBC and a Memorator t-script.
- **DAQ Decoder** ([canking-service](canking-service/README.md)): a CanKing data processor. Starts DAQ on the ECU and decodes the DAQ frames into signals.
- **a2l-daq-cli** ([a2l-daq](a2l-daq/README.md)): the DBC and t-script export without CanKing.

The extensions work with any CanKing-compatible Kvaser interface. The t-scripts run on interfaces such as the Memorator.

## Components

| Directory | Contents |
|---|---|
| [a2l-daq](a2l-daq/README.md) | A2L parser, DAQ list packing, DBC and t-script generation. Builds the CLI and the wasm component the GUI runs |
| [protocol-core](protocol-core/README.md) | What both sides must agree on: data model, XCP/CCP command sequences, frame layout, the config JSON |
| [canking-gui](canking-gui/README.md) | The DAQ Configurator, TypeScript/React ([user guide](canking-gui/user-guide.md)) |
| [canking-service](canking-service/README.md) | The DAQ Decoder, Rust compiled to wasm ([user guide](canking-service/user-guide.md)) |
| [tests/ck-cli](tests/ck-cli/README.md) | End-to-end tests against a running CanKing |

```
A2L ──> DAQ Configurator (a2l-daq as wasm)
          ├─ config JSON ──> DAQ Decoder ──> decoded signals in CanKing
          ├─ DBC ──────────> decodes DAQ logs in any DBC-aware tool
          └─ t-script ─────> scc ──> Memorator, standalone logging
```

Both extensions take the protocol commands and the frame layout from `protocol-core`, so they can't disagree. 
The config JSON is the only interface between them; adding a field to it follows [these rules](protocol-core/README.md#config-json-contract).

## Build and test

Prerequisites:

- Rust (stable) with the wasm target: `rustup target add wasm32-wasip2`
- Node.js 22 ([.nvmrc](canking-gui/.nvmrc)) and npm, for the GUI
- [cargo-about](https://github.com/EmbarkStudios/cargo-about) for `stage`: `cargo install --locked --features cli cargo-about`
- Optional: [Kvaser CanKing](https://kvaser.com) 7.6 or later to use the extensions i CanKing.
- Optional: `scc.exe` from the Kvaser CANlib SDK (Windows). The tests that compile generated t-scripts skip without it.

`run.sh` runs the groups you name, keeps going past a failure, and prints a summary:

| Group | Steps |
|---|---|
| `lint` | rustfmt, clippy with `-D warnings` in every build configuration, rustdoc, eslint and Prettier |
| `rust` | cargo tests in each feature combination, the CLI release build |
| `wasm` | the a2l-daq component and the DAQ Decoder, for `wasm32-wasip2` |
| `gui` | npm ci, jco transpile, typecheck and vite build, npm pack |
| `stage` | license files and `ck-ext-info.json` next to each build in `canking-gui/dist` and `canking-service/dist`, ready to sign |
| `pack` | `canking-gui/dist/daq-gui-<version>.ckext` and `canking-service/dist/daq_decoder-<version>.ckext` |
| `ck-cli` | the end-to-end tests |
| `all` (default) | every group except `ck-cli` |

```sh
./run.sh             # all
./run.sh lint rust   # before a commit
```

The same toolchain as CI, in Docker:

```sh
docker build -t daq-build .
docker run --rm -v "$PWD:/workspace" -u "$(id -u):$(id -g)" -e CI=1 daq-build   # runs ./run.sh all
```

CanKing keeps installed extensions in `C:\ProgramData\Kvaser\CanKing\extensions`.

## License

[MIT](LICENSE). The GUI depends on [`@kvaser/canking-api`](https://www.npmjs.com/package/@kvaser/canking-api), which Kvaser distributes under its own EULA, outside this license.

Each `.ckext` lists its third-party licenses: `THIRD_PARTY_LICENSES.html` for the Rust crates (cargo-about, configured in
[about.toml](about.toml)) and, for the GUI, `dist/THIRD_PARTY_LICENSES.md` inside the `.tgz` (npm packages, from Vite's`build.license`) plus `THIRD_PARTY_LICENSES_FONTS.txt`.

Kvaser, CanKing and the Kvaser and CanKing logos (including `canking-gui/src/assets/icon.png`) are trademarks of Kvaser AB and not licensed under the MIT License.
