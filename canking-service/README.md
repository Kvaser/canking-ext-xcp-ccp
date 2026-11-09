# canking-service (DAQ Decoder)

A CanKing data processor, Rust compiled to a wasm component. It turns the DAQ Configurator's config into a runtime
decoder, optionally sends the DAQ init sequence at measurement start, and decodes DAQ frames into signals.
Parsing A2Ls and packing lists happen in the [DAQ Configurator](../canking-gui/README.md); the config format is[protocol-core's](../protocol-core/README.md#config-json-contract).

## Behaviour

It implements the `data-processor` world of [canking-data-processor@1.2.0.wit](wit/canking-data-processor@1.2.0.wit), the version that needs CanKing 7.6:

| Call | Does |
|---|---|
| `on-config-change` | Builds the decoder, the signal definitions and the init/stop frames from the config |
| `get-subscription`, `get-frame-definitions` | Subscribes to the DAQ CAN ids and declares a `DAQ` frame per id, its signals multiplexed by `PID` |
| `on-start-measurement` | With `start_daq`, queues the init frames on the output channel |
| `on-frames` | Decodes DAQ frames and tracks the ECU's responses to the init sequence |
| `on-stop-measurement` | With `start_daq`, sends the stop frames |

- **DAQ start-up** is off unless the config sets `start_daq`. Without a command CAN id (XCP `CAN_ID_MASTER`, CCP CRO)
  it warns and only decodes. The init goes out on the first non-silent channel that matches the A2L's transport.
- **PID calibration.** The config's first PIDs are estimates. GET_DAQ_SIZE (CCP) and START_STOP_DAQ_LIST select (XCP)
  answer with a list's real first PID; the decoder then re-keys the list, logs the correction and re-declares the frame.
  A correction lasts until the config changes.
- **Responses are matched by order** Calibration therefore only runs with `start_daq`, a reported PID range that reaches the reserved identifiers (`0xFC` and up for XCP, `0xFF` for CCP) is
  rejected, lists left sharing PIDs are reported, and a sequence that ends with unanswered commands is logged at stop with a warning that the run's calibration may be misattributed.
- **Every decoded frame carries the latest value of every signal**, not only those in its own ODT (`replay_cached_signals` in [lib.rs](src/lib.rs)).

## Layout

```
|-src/
  |-lib.rs           The data-processor world: CanKing's calls
  |-config.rs        Config -> AppliedConfig: decoder, signal metadata, CAN FD flags, init/stop frames
  |-state.rs         The global state: applied config and measurement progress
  |-decode.rs        DAQ frame -> signal values, by PID
  |-calibration.rs   Re-keying a list from the PID the ECU reports
  |-definitions.rs   The frame and signal definitions declared to CanKing
  |-channels.rs      Which channel the init commands go out on
  |-logging.rs       Log messages to CanKing
|-wit/               CanKing's data-processor interface
```

## Build and test

From the workspace root:

```sh
cargo test -p daq-decoder
cargo build -p daq-decoder --release --target wasm32-wasip2   # target/wasm32-wasip2/release/daq_decoder.wasm
./run.sh wasm stage pack                                      # canking-service/dist/daq_decoder-<version>.ckext
```

`stage` and `pack` also handle the GUI and report its steps as failed when no GUI `.tgz` was built; the service `.ckext`
is unaffected. [tests/ck-cli](../tests/ck-cli/README.md) runs the built decoder inside CanKing.
