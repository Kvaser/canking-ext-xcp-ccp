# a2l-daq

Turns an A2L file into a DAQ setup: parses its measurements, transport and events, packs the selected signals into DAQ
lists, and renders a DBC, the data processor's config JSON and a Memorator t-script. One codebase, three builds: the
interactive `a2l-daq-cli`, the wasm component the [DAQ Configurator](../canking-gui/README.md) runs, and a plain library.
The data model, the protocol commands and the frame layout come from [protocol-core](../protocol-core/README.md).

## Pipeline

```
A2L text ──a2l::parse_a2l──> A2lProject ──pipeline::build──> DaqList[] + DBC + Diagnostic[]
                                                       ├──> Config JSON          (feature serde)
                                                       └──> tscript::generate    (feature tscript)
```

`pipeline::build` applies the user's event assignments, packs the lists (`daq::build_daq_list`) and renders the DBC. Its
diagnostics name the signals that couldn't be placed, are unknown, can't be measured, collide once sanitised, or have a
conversion that can't be applied. `a2l::inspect` is the one-pass summary a selection UI needs, including the keywords
listed under [Flagged](#flagged-but-not-read).

## A2L coverage

### Read

| Area | Keywords |
|---|---|
| Measurements | `MEASUREMENT` with its datatype, `ECU_ADDRESS`, `ECU_ADDRESS_EXTENSION`, `BIT_MASK`, `BYTE_ORDER`, `ARRAY_SIZE`, `MATRIX_DIM`; `VIRTUAL` measurements with one input; `GROUP` |
| Conversions | `COMPU_METHOD` `IDENTICAL`, `LINEAR`, `RAT_FUNC` when linear, `FORM` when linear in X1, `TAB_VERB` (`COMPU_VTAB`, `COMPU_VTAB_RANGE`) |
| Module | `MOD_COMMON BYTE_ORDER` |
| XCP | `XCP_ON_CAN` (CAN ids with the bit-31 extended flag, `BAUDRATE`, `MAX_DLC_REQUIRED`, `CAN_FD`), `PROTOCOL_LAYER`, the `DAQ` header (mode, `MIN_DAQ`, `GRANULARITY_ODT_ENTRY_SIZE_DAQ_*` and max entry size, `PRESCALER_SUPPORTED`), `DAQ_LIST` (`EVENT_FIXED`, `FIRST_PID`, `DAQ_LIST_CAN_ID`), `EVENT`, `TIMESTAMP_SUPPORTED` |
| CCP | `ASAP1B_CCP`: `TP_BLOB` (CRO/DTO ids, baud rate), `RASTER`, `SOURCE`/`QP_BLOB`, `CAN_ID_FIXED`, `BYTES_ONLY`, `SEED_KEY` (the DAQ DLL name) |

A non-linear `RAT_FUNC` or `FORM`, and numeric `TAB_INTP`/`TAB_NOINTP`, decode as raw values with an
`UnsupportedConversion` diagnostic.

### Flagged but not read

[`a2l/unread.rs`](src/a2l/unread.rs) warns when these appear, because they change what the outputs mean: `/include`,
`INSTANCE`, several `MODULE`s, `IDENTIFICATION_FIELD_TYPE_RELATIVE_*`, `ADDRESS_GRANULARITY_WORD`/`DWORD`, an
`ADDRESS_TYPE` other than `DIRECT`, `OVERLOAD_INDICATION_PID`, `FIXED_EVENT_LIST`. Add a row there to flag another one.

Everything else is ignored, notably transports other than CAN.

## DAQ packing

- Signals are grouped by event, one event per list. XCP dynamic lists come from one shared pool, XCP static lists only
  from those the A2L binds to the event, CCP gets one list per raster.
- Each signal becomes entries that cover whole granules of `GRANULARITY_ODT_ENTRY_SIZE_DAQ`, address and size, and is
  split at the max entry size. Entries that touch or overlap in memory merge, never across address extensions.
- First fit decreasing fills the ODTs within the frame payload (7 bytes on CAN, up to 63 on CAN FD, minus the timestamp
  in a list's first ODT) and the entry limit.
- First PIDs come from the A2L where declared and are estimated otherwise. The data processor corrects them at runtime.

## Outputs

### DBC

One message per DAQ CAN id, multiplexed by the PID in byte 0. Scaling is the conversion's factor and offset, `TAB_VERB`
tables become `VAL_` entries, floats get `SIG_VALTYPE_`, and extended ids carry bit 31.

### t-script

A Memorator program (slot 0) that brings the ECU up and logs on a trigger, without a PC:

1. Sets up the CAN channel, then runs the DAQ init as a response-driven state machine, one `case` per command.
   Optional commands may fail. A command is resent after 1 s without a response; after 10 tries, or on an error
   response, the script sends the stop sequence and starts the init over 5 s later.
2. Latches the trigger signals per PID and compares the latched values. DAQ frames are multiplexed, so two signals in
   different ODTs never share a frame:

   ```c
   if (this.data[0] == 0x01) { val_Rpm = <read>; got_Rpm = 1; }
   if (this.data[0] == 0x02) { val_Temp = <read>; got_Temp = 1; }
   if (logging == 1) { return; }
   if (got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90) { ... }
   ```

3. Starts the logger when the condition holds. With a timer it stops the logger after that long, keeps DAQ running,
   and captures again the next time the condition turns from false to true, up to an optional number of captures.
4. With a watchdog time, runs the init again when no DAQ frame arrives for that long, e.g. after the ECU restarts.

The script reads signals in one of two ways:

| | DBC mode (default) | Standalone mode |
|---|---|---|
| Compile | `scc.exe -dbase=<name>.dbc <name>.t` | `scc.exe <name>.t` |
| Message hook | `on CanMessage<*> DAQ` | `on CanMessage<*> (0x<id>)`, one per DAQ CAN id |
| Signal read | `this.Sig.Phys`, from the structs `scc` generates | `sig_Sig(this.data)`, generated into the script |
| `.Raw` triggers | supported | only where the scaling is the identity |

The DBC is generated in both modes, since it decodes the log.

## Features

| Feature | Default | Adds | Used by |
|---|---|---|---|
| `tscript` | on | the `tscript` module | the CLI, the component |
| `serde` | off | serde impls and `Config` (through `protocol-core/serde`) | the component |
| `component` | off | `wit-bindgen` and `component.rs`; implies `serde` and `tscript` | the wasm component |

## Layout

```
|-examples/dump_init.rs     Prints the DAQ init commands for an A2L
|-src/
  |-lib.rs                  Crate docs and re-exports
  |-main.rs                 a2l-daq-cli
  |-pipeline.rs             assignments -> packing -> DBC, and the diagnostics
  |-daq.rs                  DAQ list, ODT and entry packing
  |-dbc.rs                  DBC generation
  |-traceability.rs         The "generated from" lines in each output
  |-component.rs            The wasm component, implementing wit/world.wit
  |-a2l/                    The parser, one file per block family
    |-mod.rs                  parse_a2l, list_*, inspect
    |-tokenizer.rs            Tokens, block extraction, quick scans
    |-measurement.rs, virtual_signal.rs, compu_method.rs, formula.rs, common.rs, group.rs
    |-xcp.rs, ccp.rs          IF_DATA XCP and ASAP1B_CCP
    |-datatype.rs, encoding.rs
    |-summary.rs              inspect(): the view a selection UI needs
    |-unread.rs               Keywords that are flagged but not read
  |-tscript/                The t-script generator (feature tscript)
    |-mod.rs                  TScriptConfig, DaqInit, signal hooks
    |-emit.rs                 Renders the .t source
    |-trigger.rs, trigger_expr.rs  Trigger latches, conditions, text expressions
    |-inline_signals.rs       Decoders for standalone mode
|-tests/                    End-to-end: integration_tests.rs, standalone_tscript.rs (compiles with scc when installed)
|-wit/world.wit             The component's interface
|-data/                     Sample A2Ls (XCP, CCP)
```

## Build and run

From the workspace root:

```sh
cargo run -p a2l-daq --release -- path/to/ecu.a2l     # the CLI; prompts for everything else
cargo build -p a2l-daq --lib --release --target wasm32-wasip2 --no-default-features --features component
cargo test -p a2l-daq                                    # default features, the CLI
cargo test -p a2l-daq --no-default-features --features serde
```

The CLI writes `<a2l>.dbc` and a `.t` next to the A2L, the `.t` named with at most 8 characters from `A-Z a-z 0-9 _ -`,
as the logger requires (`SampleECU.a2l` gives `SampleEC.t`). It compiles a `.txe` too when it finds `scc.exe` at
`C:\Program Files (x86)\Kvaser\Canlib\Bin\scc.exe`.

The component's functions, typed in [wit/world.wit](wit/world.wit):

```wit
inspect:       func(a2l) -> result<a2l-summary, string>;
build:         func(a2l, source-name, selected, assignments, protocol) -> result<core-output, string>;
build-config:  func(a2l, source-name, selected, assignments, start-daq, daq-can-id-overrides, daq-prescalers, protocol) -> result<config-output, string>;
build-tscript: func(a2l, source-name, selected, assignments, options, daq-can-id-overrides, daq-prescalers, protocol) -> result<tscript-output, string>;
```

`wasm-tools component wit target/wasm32-wasip2/release/a2l_daq.wasm` prints the interface of a build.
