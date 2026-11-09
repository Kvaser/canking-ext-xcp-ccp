# tests/ck-cli

End-to-end tests of both extensions inside a running CanKing, driven through the CanKing CLI (`ck`). 
They build a config with the same wasm the GUI runs, apply it, decode logs offline and live, and optionally talk to a real ECU.
`ck` is documented with CanKing, by default at `C:\Program Files\Kvaser CanKing\docs\user-guide\html\ck\overview.html`.

```bash
./run.sh ck-cli                                                       # from the repository root
bash tests/ck-cli/run-all.sh                                          # every case
bash tests/ck-cli/run-all.sh tests/ck-cli/cases/ccp-sample.case.json  # one case
```

## Prerequisites

- The a2l-daq wasm transpiled into `canking-gui/src/a2l-daq-wasm/` (`npm run build:wasm`) and `canking-gui/node_modules`installed. 
  Without them `10-config` skips and every other check is blocked.
- Node.js. Every check and assert runs through it.
- A running CanKing service reachable by `ck`, with an empty measurement setup. `lib.sh` finds `ck` at `$CK`, else the
  first of `C:/Program Files/Kvaser CanKing/service/ck.exe`, `/opt/canking/service/ck`, and `ck` on `PATH`.
- Two Kvaser Virtual CAN Driver channels, for `40-live` and `45-live-pid-remap`.
- Optional: a built decoder, `canking-service/dist/daq_decoder-*.ckext` or `target/wasm32-wasip2/release/daq_decoder.wasm`,
  installed before the run, otherwise an already installed version will be used.
- Optional: a Kvaser interface wired to an ECU and a private `live-physical` case, for `90-live-physical`.

## Cases

A case (`cases/*.case.json`) stands in for the GUI's selection steps:

```json
{
  "name": "xcp-sample",
  "a2l": "a2l-daq/data/XCP/SampleECU.a2l",
  "encoding": "latin1",
  "protocol": "xcp",
  "startDaq": false,
  "signals": ["EngineSpeed_rpm", "ThrottlePosition_pct"],
  "assignments": { "10ms_Cyclic": ["EngineSpeed_rpm"], "100ms_Cyclic": ["ThrottlePosition_pct"] }
}
```

`"a2l"` may be an absolute path. Optional `daqCanIdOverrides` and `daqPrescalers` take `[DAQ list id, value]` pairs, as the GUI's DAQ list table sends them.

- **Generated** (both shipped cases): `util/make-fixture.mjs` invents DTO frames from the config.
  Cheap and deterministic, but it derives the frame layout with the decoder's own rules. It refuses floats, partial bit masks and payloads over 8 bytes.
- **Recorded**: adds a `recorded` block naming a log captured from a real ECU and the config the GUI wrote with it, under `logs/`. 
  `10-config` rebuilds the config from the A2L and asserts it equals the recorded one, which pins the A2L-to-config chain to a real GUI output.
  `recorded.expect` holds human-checked values, and `minDaqFrames` stops a log that decoded nothing from passing.

Real ECUs and their A2Ls stay out of the repository.

## Checks

`run-all.sh` runs every `checks/*.sh` in order for each case. A check limited to some cases says so in a `# only-for-cases:` header.

| Check | Does | Passes with |
|---|---|---|
| `10-config` | Builds the config with `util/build-config.mjs --strict` (any component warning fails), then the fixture (generated) or stages the log and compares configs (recorded) | the config summary |
| `20-apply` | Adds a log replay node and the decoder with the config, as Apply does, and reads the config back | `config identical after round-trip (...)` |
| `30-analysis` | Decodes the log offline with `ck analysis`; checks against the fixture (`assert/decode-synthetic.mjs`) or recomputes every value from the config (`assert/decode-log.mjs`) | `8 signal value(s) across 4 frame(s) decoded correctly` for either shipped case |
| `40-live` | Over two virtual channels: replays the log and checks every payload arrives on the DTO id, then sets `start_daq` and checks the init goes out on the command id, starting with CONNECT | all payloads seen, CONNECT sent |
| `45-live-pid-remap` | `ccp-sample` only: answers GET_DAQ_SIZE with first PID `0x12` and checks the decoder re-keys the list | `decoder re-keyed daq list <n> from 0x<old> to 0x12` |
| `90-live-physical` | `live-physical` only: runs the decoder against a real ECU (below) | every step passes or explains its skip |

Decoded values arrive as `signalValues[].qualifiedName` = `daq_decoder.DAQ.<signal>`. The decoder attaches its whole
cached snapshot to every frame, so a frame also reports other ODTs' signals with their last values; the asserts allow
for that, and `decode-log.mjs` checks those values are stale rather than invented.

Recorded logs replay at a fixed rate (`REPLAY_MS`) in `40-live`, since a capture rarely fits in `SETTLE_S`, and only
their Rx frames are injected.

### A real ECU

Write a `live-physical` case for the connected ECU and keep it outside the repository. A generated case is blocked by
the fixture generator when the ECU has floats or CAN FD frames, so run the two steps directly:

```bash
bash tests/ck-cli/checks/10-config.sh /path/to/live-physical.case.json
PHYS_CHANNEL=0 bash tests/ck-cli/checks/90-live-physical.sh /path/to/live-physical.case.json
```

It opens the channel with the config's bus settings (`PHYS_BITRATE`, `PHYS_DATA_BITRATE` override them), runs the
decoder with `start_daq` for `SETTLE_S`, then checks that the channel opened, the init and stop sequences went out,
a node acknowledged CONNECT, DAQ frames came back, nothing unexpected was logged, and the frames decode as the config
predicts. PIDs the decoder corrected live are carried into the config the offline decode uses.

## Outputs

Everything in `out/` is derived, so a check never needs to know where a case came from:

| File | Written by | Contents |
|---|---|---|
| `<name>.config.json`, `<name>.dbc` | `10-config` | the config and DBC built from the A2L |
| `<name>.asc` / `.txt` | `10-config` | the generated fixture or the staged recording |
| `<name>.expected.json` | `10-config` (generated), `30-analysis` (recorded) | the values the decode must produce |
| `<name>.frames.json` | `30-analysis` | raw `ck analysis frames-between` output |
| `<name>.startdaq.json` | `40-live`, `45`, `90` | the config with `start_daq` on |
| `<name>.pid-remap-*` | `45-live-pid-remap` | the crafted response log, its expected and actual decode |
| `<name>.physical.config.json`, `.physical.frames.json` | `90-live-physical` | the config with corrected PIDs, the decoded ECU log |

## Writing a check

A new `checks/NN-name.sh`, numbered after what it depends on (`10-config`'s output is always there; anything needing a
running measurement goes after `40`):

- `source ../lib.sh` and use its helpers: `ck_`, `pass`/`fail`/`skip`, `detail`/`detail_block`, `add_node` (adds a
  node, checks the id, registers it for cleanup), `run_for_settle`, `with_start_daq`, `add_virtual_pair`, `case_kind`,
  `case_log`, `track_node`/`cleanup_set`.
- Put the assertions in a small `assert/*.mjs`. It prints plain facts; the check prints the verdict and passes the facts
  to `detail_block`.
- Read and write `.asc` only through `util/asc.mjs`.
- Use `# only-for-cases:` when a check only applies to known cases; otherwise detect missing prerequisites and `skip`.

Exit codes: `0` pass, `77` (`$SKIP_EXIT`) skipped for a missing prerequisite, anything else fails; a check that can't
set up exits `1`. `run-all.sh` exits non-zero only on failures, and ends with a greppable count:

```
result: ok=13 skipped=2 blocked=0 failed=0
```

## Environment

| Variable | Default | Effect |
|---|---|---|
| `CK` | auto-detected | `ck.exe` / `ck` |
| `EXT_NAME` | `daq_decoder` | the extension under test |
| `EXT_CKEXT` | newest `canking-service/dist/$EXT_NAME-*.ckext` | installed before the run, if present |
| `EXT_WASM` | `target/wasm32-wasip2/release/$EXT_NAME.wasm` | installed when there is no `.ckext` |
| `EXT_DISPLAY_NAME` | `DAQ Decoder` | the display name the log assertions match |
| `SETTLE_S` | `3` (`5` in `90-live-physical`) | how long a live measurement runs |
| `REPLAY_MS` | `1` | replay rate for recorded logs |
| `OUT_DIR` | `tests/ck-cli/out` | where derived files go |
| `PHYS_CHANNEL` | the only physical channel, if exactly one | the channel wired to the ECU |
| `PHYS_BITRATE` | the config's | bench bit rate, when the ECU runs differently from its A2L |
| `PHYS_DATA_BITRATE` | the config's | bench CAN FD data rate; setting it opens the channel as CAN FD |

## One run at a time

The checks change CanKing's shared measurement setup, so two runs on one machine corrupt each other. The checks that
add nodes refuse a non-empty setup, which turns most collisions into skips.
