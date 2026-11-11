# protocol-core

Everything the configurator side ([a2l-daq](../a2l-daq/README.md), [canking-gui](../canking-gui/README.md)) and the decoder side ([canking-service](../canking-service/README.md)) must agree on: the parsed-A2L data model, the XCP and CCP command sequences, where each signal sits in a DAQ frame, and the config JSON passed between them.

## Protocols

`protocol::build_init_commands` and `build_stop_commands` dispatch to `xcp` or `ccp`. The data processor and the t-script drive either protocol with one state machine:

| Phase | XCP | CCP |
|---|---|---|
| Connect | CONNECT, GET_STATUS, GET_SEED + UNLOCK (optional) | CONNECT, GET_CCP_VERSION, GET_SEED + UNLOCK (optional) |
| Prepare | stop all; FREE_DAQ + ALLOC_* (dynamic) or CLEAR_DAQ_LIST (static); SET_DAQ_ID | stop all; GET_DAQ_SIZE per list |
| WriteEntries | SET_DAQ_PTR per ODT, WRITE_DAQ per entry | SET_DAQ_PTR and WRITE_DAQ per element |
| SetMode | SET_DAQ_LIST_MODE: event, prescaler, timestamp | START_STOP (prepare): raster, last ODT, prescaler |
| Start | START_STOP_DAQ_LIST (select) per list, START_STOP_SYNCH | START_STOP_ALL |
| Stop | stop all; FREE_DAQ or CLEAR_DAQ_LIST; DISCONNECT | stop all, DISCONNECT |

The byte layouts and the protocol rules behind them are in the module docs of [xcp.rs](src/xcp.rs) and [ccp.rs](src/ccp.rs).

`classify_response` reads a frame on the response id through the protocol's `ResponseFormat`: XCP answers `0xFF` (ok) or
`0xFE` (error), CCP answers `0xFF` with error code 0 (ok) or with another code.
A command marked optional may fail without aborting the sequence. The responses marked `ExpectedResponse::FirstPid` (XCP's START_STOP_DAQ_LIST select,CCP's GET_DAQ_SIZE)
carry the list's real first PID, which the data processor uses to correct the estimate in the config.

## Frame layout

`layout::daq_layout` says which bytes of which frame hold each signal, for the DBC, the standalone t-script and the
runtime decoder alike:

- Byte 0 of a DAQ frame is the PID (absolute identification field).
- With XCP, the first ODT of each list starts with the timestamp when the A2L declares `TIMESTAMP_SUPPORTED`.
- A signal sits at its address offset within its ODT entry. Entries can be wider than one signal, because they cover whole granules and neighbouring signals share them.

## Config JSON contract

`config::Config` (feature `serde`) is the JSON the DAQ Configurator writes and the DAQ Decoder reads: `{ project, daq_lists, start_daq, protocol }`. 
CanKing round-trips a data processor's config through a schema it infers from the extension's default config, `Config::sample()`. 

Hence:

1. `compu_methods` is serialized as an array, not a map.
2. `Config::sample()` keeps every optional sub-object (`can_fd`, `protocol_layer`, ...) populated, or CanKing strips those fields from real configs.
3. A new field gets `#[serde(default)]`, because configs saved by older versions lack it, and a value in `Config::sample()`.

## Layout

```
|-examples/
  |-dump_default_config.rs  Prints Config::sample(); run.sh puts it in the service's ck-ext-info.json
|-src/
  |-lib.rs       Crate docs
  |-types.rs     The data model: A2lProject, ParsedSignal, DaqList, ...
  |-protocol.rs  Protocol, Phase, Command, response classification, dispatch
  |-xcp.rs       XCP command sequences
  |-ccp.rs       CCP command sequences
  |-layout.rs    Signal positions and scaling in DAQ frames
  |-config.rs    Config, the JSON contract (feature serde)
```

## Build and test

```sh
cargo test -p protocol-core --features serde
```

The `serde` feature adds serde impls and `config`. The CLI build of a2l-daq leaves it off, but a workspace-wide
`cargo build` or `cargo test` turns it on anyway: `daq-decoder` depends on protocol-core with `serde`, and Cargo unifies features.
