# DAQ Configurator

This repository contains components for setting up and logging DAQ measurements from A2L files, targeting Kvaser CanKing and tscript generation.

Currently supports CAN with XCP/CCP. The CanKing extensions can be used with any Kvaser Canking compatible hardware, but the generated t-scripts can only be used with the related hardware such as the Memorator.

The CanKing extensions require Kvaser CanKing 7.6 or later.

## Repository layout

The Rust crates form a single Cargo workspace (shared `./target` and a single
root `Cargo.lock`):

```
|- a2l-daq/          A2L parsing, DAQ list packing, DBC + t-script generation. Written in Rust.
|
|- canking-gui/      "DAQ Configurator" CanKing GUI extension, uses the rust logic through generated jco artefacts.
|
|- canking-service/  "daq-decoder" - CanKing service extension. Decodes DAQ frames into signal values.
|
|- protocol-core/    Shared library written in Rust: parsed-A2L data model, DAQ signal layout, protocol command generation       
```

## Data flow

```
A2L file
  |- DAQ Configurator (canking-gui)
       |- Config JSON -> daq data processor -> decoded signals in CanKing             
       |- DBC         -> download (decode DAQ frames anywhere)
       |- t-script    -> download -> compile -> Memorator standalone logging
```

The t-script reads its trigger signals either through the DBC (`scc -dbase=`) or by decoding the DAQ frames itself, 
in which case it compiles standalone.

The same DBC/t-script artefacts can also be produced offline with the
`a2l-daq-cli`. See [a2l-daq/README.md](a2l-daq/README.md).

## Building

Prerequisites:

- a stable Rust toolchain; for the wasm components the `wasm32-wasip2` target
  (`rustup target add wasm32-wasip2`)
- for the GUI: Node.js 22 (see [canking-gui/.nvmrc](canking-gui/.nvmrc)) + npm
- to package the extensions (`./run.sh stage`): [cargo-about](https://github.com/EmbarkStudios/cargo-about)
  (`cargo install --locked --features cli cargo-about`)
- to use the extensions: [Kvaser CanKing](https://kvaser.com) 7.6 or later
- to compile generated t-scripts: `scc.exe` from Kvaser CANlib SDK (Windows). TRX, also from CANLIB SDK can be used to work on and compile t-scripts

Or build everything in Docker, see [Building in Docker](#building-in-docker).

To build and test everything in one go:

```sh
./run.sh          # all groups
./run.sh --help   # available groups
```

The individual commands, from the repository root:

```sh
cargo test                                 # all crates, default features
cargo test -p protocol-core --features serde    # the shared JSON contract tests on their own
cargo test -p a2l-daq --no-default-features --features serde # serde/JSON contract, without tscript
cargo build -p a2l-daq --release           # CLI (A2L -> DBC + t-script) -> target/release/a2l-daq-cli

# Wasm components
cargo build -p a2l-daq --lib --release --target wasm32-wasip2 --no-default-features --features component 
cargo build -p daq-decoder --release --target wasm32-wasip2 # -> target/wasm32-wasip2/release/daq_decoder.wasm
```

The GUI consumes the `a2l-daq` component via `jco transpile`; run
`npm run build:wasm` in [canking-gui/](canking-gui/README.md) to rebuild and
re-transpile in one step (`npm run transpile:wasm` transpiles without rebuilding the wasm first).

`./run.sh stage pack` bundles the builds into installable `.ckext` files
(`canking-gui/dist/daq-gui-<version>.ckext` and
`canking-service/dist/daq_decoder-<version>.ckext`). Installed CanKing
extensions live in `C:\ProgramData\Kvaser\CanKing\extensions`.

Component docs: [a2l-daq](a2l-daq/README.md), [canking-gui](canking-gui/README.md)
([user guide](canking-gui/user-guide.md)), [canking-service](canking-service/README.md),
[protocol-core](protocol-core/README.md), and the end-to-end tests in
[tests/ck-cli](tests/ck-cli/README.md).

### Building in Docker

One toolchain image with both Rust and Node.

```sh
#Build dockerfile
docker build -t daq-build .
# Run the dockerfile with the correct privileges
docker run --rm -v "$PWD:/workspace" -u "$(id -u):$(id -g)" -e CI=1 daq-build
```

The default command is `./run.sh all` which runs cargo tests, then the wasm components,
then jco transpile, the GUI build and `.tgz`, and finally stages and packs the
(unsigned) `.ckext` files into `canking-gui/dist` and `canking-service/dist`.

## The Config JSON contract

`protocol_core::config::Config` is the single source of truth for the JSON handed from the configurator to the data processor:
`{ project, daq_lists, start_daq, protocol }`. Both sides use the same type so they cannot drift apart. Defaults to XCP when absent.

Two representation rules exist because CanKing round-trips extension configurations through a schema inferred from the extension's default config.

1. `compu_methods` is serialized as an array representation
2. The data processor's `Config::sample()` (its default config) must keep every optional sub-object (`can_fd`, `protocol_layer`, ...) populated,
   otherwise those fields would be stripped from every real configuration.

See [protocol-core/src/config.rs](protocol-core/src/config.rs) for the full rules.

## License

This project is licensed under the [MIT License](LICENSE).

The GUI extension depends on [`@kvaser/canking-api`](https://www.npmjs.com/package/@kvaser/canking-api),
which is distributed by Kvaser under its own EULA and is not covered by this
project's license.

The packaged extensions list their third-party licenses: `THIRD_PARTY_LICENSES.html` (Rust crates, generated by [cargo-about](https://github.com/EmbarkStudios/cargo-about)
from [about.toml](about.toml)) and, for the GUI, `dist/THIRD_PARTY_LICENSES.md` inside the `.tgz` (npm packages, generated by Vite's `build.license`) plus `THIRD_PARTY_LICENSES_FONTS.txt`.

Kvaser, CanKing and the Kvaser and CanKing logos (including `canking-gui/src/assets/icon.png`) are trademarks of Kvaser AB and are not licensed under the MIT License.
