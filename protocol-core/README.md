# protocol-core

The shared library. Everything that more than one crate needs lives here, so the configurator side (`a2l-daq` / `canking-gui`) and the decoder side (`canking-service`) always agree on the data model, the DAQ init bytes, and the JSON shape.

## Layout
```
|-examples/
  |- dump_default_config.rs  Prints Config::sample() as JSON (needs --features serde); run.sh uses it for the service's default config
|-src/
  |- lib.rs       Crate root and module declarations
  |- ccp.rs       CCP command generation
  |- config.rs    Config: the JSON the extension is built from (feature `serde`).
  |- layout.rs    Where each signal lands in its DAQ frame.
  |- protocol.rs  The shared abstraction for different protocols.
  |- types.rs     the shared data model
  |- xcp.rs       XCP command generation
```

## Features

- `serde`: serde impls on all types plus the `config` module. Off by default so the CLI build of `a2l-daq` does not pull in serde.
  Note that a workspace-wide `cargo build` / `cargo test` enables it anyway, because `daq-decoder` (canking-service) depends on
  `protocol-core` with `features = ["serde"]` and Cargo unifies features across the workspace build.

## Build & test

```sh
cargo test -p protocol-core --features serde
```
