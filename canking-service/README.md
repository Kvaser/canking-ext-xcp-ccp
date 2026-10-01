# DAQ CanKing data-processor extension

A CanKing data-processor service extension that decodes CAN DAQ frames into signal values in real time, and can optionally drive DAQ startup on the ECU.

The extension does not parse A2L files or pack DAQ lists itself, that happens in the [DAQ Configurator](../canking-gui/README.md),
which compiles the user's selection into a JSON config. This extension uses that pre-compiled config, builds a runtime decoder from it and, 
when enabled, sends the init/start-DAQ command sequence to the ECU.

## Layout

The pieces shared with the configurator side live in [protocol-core](../protocol-core/README.md).

```
|- calibration.rs   Adjusts the decoder if the reported PID is not the same as the Config
|- channels.rs      Picking/validating the output CAN channel to send init frames on
|- config.rs        Re-exports protocol_core::config::Config and turns it into runtime state (AppliedConfig:
                    decoder, signal metadata, CAN FD flags, init/stop command frames; ConfigError on failure)
|- decode.rs        Frame decoding (DAQ frame -> Signal values), PID layout, re-keying
|- definitions.rs   Building the FrameDefinitions reported to CanKing
|- lib.rs           Data-processor world implementation (CanKing functions)
|- logging.rs       Logging logic
|- state.rs         Global STATE lock + the State struct (config-derived data + measurement progress)
```

The extension implements the `data-processor` world from [`wit/canking-data-processor@1.2.0.wit`](wit/canking-data-processor@1.2.0.wit):

- `on-config-change` parses and applies the JSON config.
- `get-subscription` / `get-frame-definitions` subscribe to the DAQ CAN ids and declare the decoded signals.
- `on-start-measurement` queues the DAQ init sequence (when `start_daq` is set).
- `on-frames` decodes DAQ frames into signal values and tracks the slave's responses to the init sequence.
- `on-stop-measurement` sends the DAQ stop sequence.

### DAQ startup (optional)

DAQ startup is off by default. Enable it with `start_daq: true`:

- `start_daq` - whether the extension queues the DAQ init sequence at measurement start. 
If `true` but the config has no command CAN id (`CAN_ID_MASTER` for XCP, the CRO id for CCP), it warns and only decodes.

### PID calibration

The `first_pid` in the config is only an estimate from the a2l. The extension therefore corrects the assignment if needed while a measurement runs, from what the slave reports: `GET_DAQ_SIZE` (CCP) and `START_STOP_DAQ_LIST` in select mode (XCP) answer with the DAQ list's real first PID.

A correction is logged, re-declares the DAQ frame so the host's multiplex values follow, and is kept across measurements. A new config discards it. 

#### When responses go missing

Counting XCP responses in order is only sound while every command is answered. A dropped response shifts every later one by a slot, and a report attributed to the wrong command re-keys the wrong list with a plausible-looking PID.

- Calibration only runs with `start_daq: true`
- A reported PID range running into the protocol's reserved identifiers (`0xFC` and up for XCP, `0xFF` for CCP) is rejected and the configured PIDs kept.
- Once the sequence is fully answered, DAQ lists left sharing PIDs are reported.

If the sequence ends with commands unanswered, that is logged at stop with the warning that any calibration from the run may be misattributed.

## Configuration

The [JSON config](../protocol-core/src/config.rs) contains the available fields that the extension uses. 

Top-level fields:
- `project`: the `A2lProject`: signals, transport config, DAQ mode, events, and COMPU_METHOD scaling. Used for decoding and for building the init sequence.
- `daq_lists`: the compiled ODT-packed DAQ lists.
- `start_daq`: whether to initiate DAQ. Defaults to false.
- `protocol`: What protocol is used to perform DAQ.

## Build

Install the wasm target (if not already present):

```sh
rustup target add wasm32-wasip2
```

Build the component (from the workspace root or this directory):

```sh
cargo build -p daq-decoder --release --target wasm32-wasip2
# -> target/wasm32-wasip2/release/daq_decoder.wasm
```

Run the unit tests:

```sh
cargo test -p daq-decoder
```

## Install

Package the component into an installable `.ckext` from the repo root:

```sh
./run.sh wasm stage pack
# -> canking-service/dist/daq_decoder-<version>.ckext
```

(`wasm` runs the `cargo build ... --target wasm32-wasip2` above; `stage` and `pack` bundle it with its `ck-ext-info.json` and `license.txt`.
Those two groups also handle the GUI bundle; its steps report a failure if no GUI `.tgz` was built, which does not affect the service `.ckext`.)

Installed CanKing extensions live in `C:\ProgramData\Kvaser\CanKing\extensions`.
