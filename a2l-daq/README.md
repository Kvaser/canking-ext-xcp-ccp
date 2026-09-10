# a2l-daq

Turns A2L measurement metadata into everything needed to run DAQ logging:
packed DAQ lists, a CAN database (DBC) file, the data-processor config JSON, 
and a t-script that initialises the ECU for DAQ and starts logging on a trigger condition.

The shared data model, the DAQ command generation, and the `Config` JSON contract live in
[protocol-core](../protocol-core/README.md); this crate adds the A2L parser, the DAQ packing, and the DBC/t-script generators on top.

---

## What it does

1. **Parses an A2L file**: extracts `MEASUREMENT` signals, `IF_DATA` transport configuration, and `COMPU_METHOD` scaling definitions.
2. **Packs selected signals into ODTs** using a First-Fit-Decreasing bin-packing algorithm that respects the size constraints defined in the A2L.
3. **Generates a DBC file** to decode the raw DAQ frames.
4. **Generates a t-script** that configures the CAN bus, runs the full DAQ initialisation sequence, and starts/stops logging on a signal trigger.

---

## Architecture

```
|-examples/
  |- dump_init.rs         Short CLI tool for testing DAQ init command generation
|-src/
  |- a2l/                 Parser for A2L files, only parses sections needed for DAQ. One file per block family:
    |- mod.rs                Public API (parse_a2l, list_*, inspect) + orchestration
    |- tokenizer.rs          Tokenizing + block extraction + the without-a-full-parse scans
    |- datatype.rs           The A2L base datatype (size/signedness/float-ness in one place)
    |- measurement.rs        MEASUREMENT block parsing
    |- virtual_signal.rs     Resolving virtual measurements against real ones
    |- formula.rs            FORM formula -> linear(factor, offset) reduction
    |- xcp.rs / ccp.rs       IF_DATA XCP / ASAP1B_CCP transport blocks
    |- compu_method.rs       COMPU_METHOD / COMPU_VTAB(_RANGE)
    |- common.rs             MOD_COMMON (protocol-independent defaults)
    |- summary.rs            UI-facing SignalSummary/inspect() view over a parse result
    |- encoding.rs           raw file bytes -> UTF-8 (Windows-1252 fallback)
  |- tscript/             T-script generation. Everything behind the `tscript` feature lives here:
    |- mod.rs                Public API (TScriptConfig/DaqInit + the helpers deriving them from an A2L) + re-exports
    |- emit.rs               Generates the .t source
    |- trigger.rs            What the trigger reads + the PID-gated latch/condition fragments
    |- inline_signals.rs     Signal decoders for scripts that carry no DBC
    |- trigger_expr.rs       Text trigger expression -> terms -> resolved trigger
    |- test_util.rs          Utilities shared by the tscript tests.
  |- component.rs         WASI component / wit-bindgen glue.
  |- daq.rs               FFD bin-packing into ODTs / DAQ lists.
  |- dbc.rs               DBC file generator.
  |- main.rs              Interactive CLI.
  |- pipeline.rs          Orchestrates parse + event assignments -> { daq_lists, dbc, warnings }
  |- traceability.rs      Generates a traceability comment for the generated artefacts so that the source is more easily found.
|-tests/
  |-integration_test.rs   End to end testing for the a2l-daq crate
  |-standalone_tscript    End to end testing for building a t-script without dbc-file
|-wit/
  |-world.wit             WASM-WASIP2 outward interface 
```

### Data flow

```
A2L file
  │
  V  a2l/          (select signals and assign to events -> ParsedSignal[]  +  CompuMethod{} + ...)
  │
  V  pipeline::build  -->  daq.rs (DaqList[], ODT-packed, PID-assigned) + dbc.rs
  │
  ├─>  <name>.dbc                     (always)
  │
  ├─>  Config JSON                    (feature "serde"; used by the service ext.)
  │                                    
  └─>  DaqList[]  -->  protocol::build_init_commands  ->  DAQ init command bytes
                   │
                   └>  tscript::generate  ->  <name>.t  [-> scc.exe -> .txe]
```

---

## Generated outputs

### DBC file

- One multiplexed message named `DAQ` on the slave CAN ID; the first byte of each frame is the PID, remaining bytes are signals from the ECU.
- Signal definitions include bit position, length, byte order, and scaling from `COMPU_METHOD`; TAB_VERB text tables become `VAL_` entries.
- `FLOAT32_IEEE` / `FLOAT64_IEEE` signals get a `SIG_VALTYPE_` attribute.
- If `TIMESTAMP_FIXED` is declared in the A2L, the first ODT prepends a timestamp signal and all other signal offsets are shifted accordingly.

### T-script

The generated `.t` file does the following at runtime:

1. Configures the CAN channel (bitrate, CAN-FD data-phase rate if needed).
2. Runs the full DAQ initialisation sequence as a response-driven state machine.
   `sendDaqCommand` is a `switch` over the state, one `case` per command, each in its own block. 
3. Optionally configures a pretrigger buffer.
4. Optionally starts a countdown timer that stops logging (and stops DAQ / disconnects) after N ms.
5. Installs the signal trigger hook (start/stop logging when the condition over the sampled signal values becomes true).

The CLI compiles the `.t` to a `.txe` automatically when `scc.exe`  (from Canlib) is found.

#### The trigger and the PID multiplexer

DAQ data is multiplexed: every ODT arrives as a separate frame on the same CAN id, distinguished by the PID in byte 0.
So a signal's bytes are only valid in frames carrying ODT, and two signals in different ODTs are never present in the same frame.

The generated trigger therefore latches rather than reading in place. Each signal gets `val_<ident>`/`got_<ident>` globals,
updated behind the PID gate for its ODT, and the comparisons run over the latched values:

```c
if (this.data[0] == 0x01) { val_Rpm = <read>; got_Rpm = 1; }
if (this.data[0] == 0x02) { val_Temp = <read>; got_Temp = 1; }
if (logging == 1) { return; }
if (got_Rpm == 1 && got_Temp == 1 && val_Rpm > 3000 && val_Temp < 90) { ... }
```

This applies to both signal-access modes; only `<read>` differs.

#### How the script reaches signal values

Two modes, selected per generation DBC mode is the default.

| | DBC mode | Standalone mode |
|---|---|---|
| Compile | `scc.exe -dbase=<name>.dbc <name>.t` | `scc.exe <name>.t` |
| Message hook | `on CanMessage<*> DAQ` | `on CanMessage<*> (0x<slave id>)` |
| Signal read | `this.Sig.Phys`, from the struct `scc` generates | `sig_Sig(this.data)`, generated into the script |
| `.Raw` triggers | supported; latched separately from `.Phys` | only where the scaling is the identity |

Standalone mode exists so the `.t` can be used on its own. The DBC is still generated, since it is what decodes the resulting log.
---

## Building and running

One codebase compiles into three shapes, selected by Cargo features:

| Feature | Default | Adds | Used by |
|---------|---------|------|---------|
| `tscript` | on | the `tscript/` module (generator, trigger, inline decoders) | the CLI and the component |
| `serde` | off | serde impls + the `Config` JSON contract (via `protocol-core/serde`) | the component, JSON consumers |
| `component` | off | `wit-bindgen` + `component.rs` (implies `serde` + `tscript`) | the wasm component |

All commands work from the workspace root (or from this directory). Build output goes to the workspace-level `target/`.

### 1. CLI (default)

Interactive native binary. A2L in, DBC + t-script out:

```sh
cargo run -p a2l-daq --release -- path/to/file.a2l
cargo build -p a2l-daq --release        # -> target/release/a2l-daq-cli
```

The tool prompts you to:
1. Select which signals to include (by number or name; empty = all).
2. Assign selected signals to event channels.
3. Configure CAN channel index, bitrate, pretrigger, timer, and trigger conditions.

Outputs are written next to the input A2L file: `file.dbc` and a `.t` script (plus `.txe` if `scc.exe` is installed).

### 2. WASM component

The core exposed as a WASI 0.2 component, consumed by
[canking-gui](../canking-gui/README.md):

```sh
rustup target add wasm32-wasip2   # once
cargo build -p a2l-daq --lib --release --target wasm32-wasip2 --no-default-features --features component # -> target/wasm32-wasip2/release/a2l_daq.wasm
```

The interface is defined in [`wit/world.wit`](wit/world.wit) and implemented by the wit-bindgen glue in `src/component.rs`.

```wit
inspect:       func(a2l) -> result<a2l-summary, string>;
build:         func(a2l, source-name, selected, assignments, protocol) -> result<core-output, string>;
build-config:  func(a2l, source-name, selected, assignments, start-daq, protocol) -> result<config-output, string>;
build-tscript: func(a2l, source-name, selected, assignments, options, protocol) -> result<tscript-output, string>;
```

(Parameter names only; see [`wit/world.wit`](wit/world.wit) for the types.)

A host first calls `inspect` to get the selectable signals and event channels in a single pass, populates its selection/assignment UI, then calls
`build-config` (data-processor JSON + DBC) and/or `build-tscript` (`.t` file) with the user's choices.

To inspect/validate the built component (requires `cargo install wasm-tools`):

```sh
wasm-tools component wit target/wasm32-wasip2/release/a2l_daq.wasm
wasm-tools validate target/wasm32-wasip2/release/a2l_daq.wasm
```

### 3. Core library only

```sh
cargo build -p a2l-daq --lib --no-default-features
```

### Tests

```sh
cargo test -p a2l-daq                                       # core + CLI
cargo test -p a2l-daq --no-default-features --features serde  # JSON contract
```