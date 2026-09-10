# DAQ Configurator

This repository contains components for setting up and logging DAQ measurements from A2L files, targeting Kvaser CanKing and tscript generation.

Currently supports CAN with XCP/CCP

## Repository layout

The Rust crates form a single Cargo workspace (shared `./target` and a single
root `Cargo.lock`):

```
|- a2l-daq/          A2L parsing, DAQ list packing, DBC + t-scriptgeneration. Written in Rust.
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
       |- t-script    -> download -> scc.exe -> Memorator standalone logging
```

The t-script reads its trigger signals either through the DBC (`scc -dbase=`) or by decoding the DAQ frames itself, 
in which case it compiles standalone.

The same DBC/t-script artefacts can also be produced offline with the
`a2l-daq-cli`. See [a2l-daq/README.md](a2l-daq/README.md).

## Building

Prerequisites: a stable Rust toolchain; for the wasm components the
`wasm32-wasip2` target (`rustup target add wasm32-wasip2`); for the GUI
Node.js + npm.

To build and test everything in one go:

```sh
./run.sh          # all groups
./run.sh usage    # available groups
```

The individual commands, from the repository root:

```sh
cargo test                                 # all crates, default features
cargo test -p protocol-core --features serde    # + the shared JSON contract tests
cargo test -p a2l-daq --no-default-features --features serde # CLI (A2L -> DBC + t-script from the command line)
cargo build -p a2l-daq --release           # -> target/release/a2l-daq-cli

# Wasm components
cargo build -p a2l-daq --lib --release --target wasm32-wasip2 --no-default-features --features component 
cargo build -p daq-decoder --release --target wasm32-wasip2 # -> target/wasm32-wasip2/release/daq_decoder.wasm
```

The GUI consumes the `a2l-daq` component via `jco transpile`; run
`npm run build:wasm` in [canking-gui/](canking-gui/README.md) to rebuild and
re-transpile in one step (`npm run transpile:wasm` transpiles without rebuilding the wasm first).

Installed CanKing extensions live in`C:\ProgramData\Kvaser\CanKing\extensions`.

### Building in Docker

One toolchain image with both Rust and Node.

```sh
#Build dockerfile
docker build -t daq-build .
# Run the dockerfile with the correct privileges
docker run --rm -v "$PWD:/workspace" -u "$(id -u):$(id -g)" -e CI=1 daq-build
```

The default command is `./run.sh all` which runs cargo tests, then the wasm components,
then jco transpile, then the GUI build and `.tgz`.

## JSON info

`protocol-core::config::Config` is the single source of truth for the JSON handed from the configurator to the data processor:
`{ project, daq_lists, start_daq, protocol }`. Both sides use the same type so they cannot drift apart. Defaults to XCP when absent.

Two representation rules exist because CanKing round-trips extension configurations through a schema inferred from the extension's default config.

1. `compu_methods` is serialized as an array representation
2. The data processor's `Config::sample()` (its default config) must keep every optional sub-object (`can_fd`, `protocol_layer`, ...) populated,
   otherwise those fields would be stripped from every real configuration.

See [protocol-core/src/config.rs](protocol-core/src/config.rs) for the full rules.
