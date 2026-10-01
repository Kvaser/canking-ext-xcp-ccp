# CanKing ck-cli tests

Tests that check the CanKing extensions (GUI & service)

```bash
./run.sh ck-cli                   # from the repo root
bash tests/ck-cli/run-all.sh      # or directly
bash tests/ck-cli/run-all.sh tests/ck-cli/cases/ccp-sample.case.json
```

Everything is driven through the CanKing CLI (`ck`). On Windows its documentation is installed with CanKing, at
`C:\Program Files\Kvaser CanKing\docs\user-guide\html\ck\overview.html` (default install location).

`lib.sh` locates `ck` as follows: `$CK` if set, otherwise the first executable of
`/c/Program Files/Kvaser CanKing/service/ck.exe`, `C:/Program Files/Kvaser CanKing/service/ck.exe`,
`/opt/canking/service/ck`, and `ck` on `PATH`.

### Prerequisites

- The a2l-daq wasm transpiled into `canking-gui/src/a2l-daq-wasm/` (`npm run build:wasm` in `canking-gui`) and
  `canking-gui/node_modules` installed (`npm install`). Without them `checks/10-config.sh` skips and every other check is blocked.
- Node.js (every check and assert runs through `node`).
- A running CanKing service, reachable through the `ck` CLI (see above). Without it every per-case check is blocked.
- An empty CanKing measurement setup; the checks that add nodes skip otherwise.
- Two channels from the Kvaser Virtual CAN Driver for `checks/40-live.sh` and `checks/45-live-pid-remap.sh` (skipped otherwise).
- Optionally, a physical Kvaser interface wired to an ECU for `checks/90-live-physical.sh` (skipped otherwise).
- Optionally, a built service extension (`canking-service/dist/daq_decoder-*.ckext` or the raw
  `target/wasm32-wasip2/release/daq_decoder.wasm`), which `run-all.sh` installs before the run. Without one, whatever
  `daq_decoder` is already installed is tested.

### Layout

```
tests/ck-cli/
├── run-all.sh   entry point: discovers and runs every checks/*.sh
├── lib.sh       shared helpers (ck discovery, node cleanup, pass/fail/skip, ...)
├── cases/       one *.case.json per A2L/protocol combination under test
├── checks/      one script per procedure being checked, numbered for run order
├── assert/      the actual assertions each check delegates to
├── util/        helpers for checks or asserts
├── logs/        (not shipped) recorded log + config pairs for any "recorded"-kind case (see below)
└── out/         generated artifacts during testing
```

### Two kinds of case

**Generated** cases name an A2L and a signal/event selection. `util/make-fixture.mjs` then invents
DTO frames from the resulting config. Cheap, deterministic, and regenerable for any A2L in the repo,
but the generator derives the frame layout with the same rules the decoder uses, so the two
agree even if that shared idea of the layout is wrong. Both cases shipped today (`xcp-sample`,
`ccp-sample`) are this kind.

**Recorded** cases add a `recorded` block naming a log captured off a real ECU and the config the GUI
wrote during that session. The frames are ground truth and not synthetic. They also reach code the
generator refuses to emit: `make-fixture.mjs` bails on floats, on partial bit masks, and on payloads
over 8 bytes. Both kinds run every check the same way; the machinery for this kind (`case_kind`,
the `recorded` staging in `checks/10-config.sh`, the replay-rate handling in `checks/40-live.sh`)
stays in place for whenever a case needs it, but no recorded pair ships in this repo -- add one under
`logs/` with a `recorded` block pointing at it, the same way the generated cases point at an A2L.

A recorded case still names its A2L, and `checks/10-config.sh` still rebuilds the config from it,
then asserts the rebuild equals `recorded.config`. That keeps one path to a config instead of two,
and pins the whole A2L -> config chain against a real GUI output at the same time. If the rebuild
ever stops matching, either the generator regressed or the change was deliberate and the pair needs re-recording.

`recorded.expect` holds values checked once by a human for physical plausibility. `minDaqFrames`
stops a log that silently decoded nothing from passing.

### `checks/10-config.sh`: A2L -> extension config JSON

Runs the wasm component that the GUI runs and builds a config. For a generated case it then produces
the DTO frame fixture from that config; for a recorded case it stages the recorded log and asserts
the config it just built against the one recorded with that log.

Inputs are case files (`cases/*.case.json`), the stand-in for the GUI's selection screens:

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

**Expected result.** Exit 0 and print the config

Fails on: an unknown signal or event name in the case file, a protocol the A2L does not declare, code failures, `configJson` not parsing, 
or any component warning (the check always runs `util/build-config.mjs` with `--strict`).

Skips when needed files are missing

---

### `checks/20-apply.sh`: Apply

Attaches the extension with the generated config exactly as the GUI's Apply button does, then reads the configuration back out and asserts it.

```bash
ck add logreplay '{"0":"<guid>"}' --path <fixture.asc> --name "ApplySrc"
ck add extension daq_decoder --name "DAQ Decoder" --previous-node <src> --config "$(cat config.json)"
ck get nodes --dataprocessors --json --node <dp>
```

A Log Replay node is used as the upstream source.

**Expected result.** Exit 0 and `config identical after round-trip (<size> B, <n> signal(s), <n> DAQ list(s))`
(2 signal(s), 2 DAQ list(s) for either shipped case).

---

### `checks/30-analysis.sh`: offline decode

This is the test of the extension's decoder. `ck analysis` runs a service extension over a log file with no external factors.

```bash
ck analysis open <fixture.asc> --json
ck analysis set-processing <session> --step "ext:name=daq_decoder;config=$(cat config.json)"
ck analysis frames-between <session> --start-s 0 --end-s 3600 --json
ck analysis close <session>
```

Decoded values arrive as `signalValues[].qualifiedName` = `daq_decoder.DAQ.<signal>` with `doubleValue` and `unit`.

Note that the extension attaches its **whole cached snapshot** to every frame (`replay_cached_signals`
in `canking-service/src/lib.rs`), so a frame reports signals other ODTs own, carrying their last
decoded value. Both asserts below allow for that; `decode-log.mjs` checks those replayed values are
stale rather than invented.

Which assert runs depends on the case kind:

- **generated** -> `assert/decode-synthetic.mjs`, against `<name>.expected.json` from the fixture
  generator. **Expected result:** exit 0 and `8 signal value(s) across 4 frame(s) decoded correctly`
  (true of both shipped cases).
- **recorded** -> `assert/decode-log.mjs`, which recomputes every value from the config in
  JavaScript, compares against the extension, checks the pinned expectations, and checks the latched
  values. **Expected result:** exit 0 and a summary line of the form

  ```
  <n> DAQ frame(s) on id <dto>, <n> value(s) recomputed, <n> pinned, <n> latched, <n> non-DAQ frame(s): all correct
  ```

---

### `checks/40-live.sh`: live measurement over two virtual channels

Two virtual CAN channels connected, one with the extension attached and the other that is used to send out frames

Channels are added to match the config's framing: a config declaring `can_fd` gets the pair created
with `--bit-rate-fd`. Needs two virtual channels from the Kvaser Virtual CAN Driver.

Two phases:

- **Phase A: transport.** Replays the log and asserts every payload arrived as Rx on the DTO id.
Recorded logs are replayed with `--samplerate` (`REPLAY_MS`, default 1) instead of their own
timestamps, since a real capture's actual timespan rarely fits inside `SETTLE_S`. Replay defaults to
`--transceiver-mode Rx`, so the master's Tx commands in a recorded log are not injected onto the bus.
- **Phase B: init sequence.** Re-applies the same config with `start_daq: true` and asserts the extension transmits its
command sequence on the CRO id, leading with CONNECT (`ff 00` for XCP, `01 ..` for CCP). The exact frame
count depends on the config's signal/DAQ-list layout.

---

### `checks/45-live-pid-remap.sh`: PID calibration against an ECU report

Runs only for `ccp-sample` (`# only-for-cases: ccp-sample`), and skips unless the config is CCP with at least two DAQ lists.
Uses the same virtual channel pair as `40-live`.

- **Pass 1: discovery.** Runs a measurement with `start_daq: true` and finds the extension's `GET_DAQ_SIZE` command for the
  last DAQ list in the logged frames, to learn its command counter.
- **Pass 2: re-keying.** Builds a responder log (`util/make-responder.mjs`) that answers that command with a different first PID
  (`0x12`) followed by a DAQ frame keyed to it, runs it through `ck analysis`, and asserts with `assert/decode-synthetic.mjs`
  that the frame decoded to that list's own signals.

**Expected result.** Exit 0 and `decoder re-keyed daq list <n> from 0x<configured> to 0x12: ...`.

---

### `checks/90-live-physical.sh`: real ECU on the bench

A sanity check against a real ECU connected through a physical Kvaser interface. It uses the case's config, so it
is only meaningful when the case matches the connected ECU (the shipped cases describe sample A2Ls, not real hardware).

Opens the physical channel (`PHYS_CHANNEL`, or the only non-virtual channel if exactly one exists; skips otherwise) with
the config's bit rate/CAN FD settings, attaches the extension with `start_daq: true` and runs for `SETTLE_S` (default `5`
here). It then asserts, from the service event log and the message log, that:

1. the channel opened (skips if canlib could not open it),
2. the extension queued the init sequence and sent the stop sequence,
3. CONNECT was sent on the command id (only a note if no node ACKed it),
4. DAQ frames came back on the DTO id (skips if none did),
5. the extension logged no unexpected warnings, and the received frames decode consistently with the config (`assert/decode-log.mjs`).

---

### What a check can rely on

Everything in `out/` is derived, so a check never has to know where a case came from:

| file | written by | contents |
|---|---|---|
| `<name>.config.json` | `10-config` | the extension config, from the A2L |
| `<name>.dbc` | `10-config` | the DBC the same build produced |
| `<name>.asc` / `<name>.txt` | `10-config` | the log to replay, generated fixture, or staged recording |
| `<name>.expected.json` | `10-config` (generated), `30-analysis` (recorded) | see below |
| `<name>.frames.json` | `30-analysis` | raw `ck analysis frames-between` output |
| `<name>.startdaq.json` | `40-live` / `45-live-pid-remap` | the config with `start_daq` forced on |
| `<name>.pid-remap-responder.asc` | `45-live-pid-remap` | the crafted GET_DAQ_SIZE response + synthetic re-keyed DAQ frame |
| `<name>.pid-remap.expected.json` / `.pid-remap.frames.json` | `45-live-pid-remap` | what pass 2's synthetic frame must decode to, and what `ck analysis` actually decoded |
| `<name>.physical.frames.json` | `90-live-physical` | raw `ck analysis frames-between` output of the ECU log |

### Adding a case

A generated case is one `cases/*.case.json` naming an A2L and a selection, nothing else. A recorded
case additionally commits its log and config under `logs/` and points a `recorded` block at them.
Either way `run-all.sh` picks it up by glob and every check applies.

### Adding a check

A new `checks/NN-name.sh`:

- Numbered to run after whatever it depends on. `10-config.sh`'s output is always available;
anything depending on a running measurement fits after `40-live.sh`'s setup, e.g. `45-`.
- `source`s `../lib.sh`, uses `ck_`, `pass`/`fail`/`skip`, and the `track_node`/`cleanup_set` node-tracking helpers.
- Delegates real assertions to a small script under `assert/`
- Picked up by `run-all.sh` automatically
- Checks may restrict execution to specific cases using # only-for-cases: when their applicability is known statically. Checks that are generally applicable but encounter unmet prerequisites should detect the condition and call skip with a reason.

`lib.sh` carries the pieces a check should not re-invent: `add_node` (add a node, verify ck returned an
id, register it for cleanup), `run_for_settle` (start the measurement, optionally start a replay, wait
`SETTLE_S`, stop, wait for the logger to flush), `with_start_daq`, `add_virtual_pair`, `case_kind` and
`case_log`. The `.asc` format lives only in `util/asc.mjs`. Read and write it through that.

---

### Output convention and exit codes

Every status line from `lib.sh` is a status word padded to a fixed column, then the message:

`pass`/`fail`/`skip` are the status lines. `detail`/`detail_block` nest a supporting line under the message column with no status word of its own. Called after the line they support, so the verdict always leads and the data trails it. `assert/*.mjs` scripts follow the same rule: they print plain, unindented facts and leave the verdict and its indentation to the caller, which captures their stdout and passes it to `detail_block`.

**Exit codes**

`0` = pass, `77` (`$SKIP_EXIT`) = skipped (a missing prerequisite, not a defect), anything else = fail.
A check that cannot even get set up (e.g. its config JSON is missing) exits `1` directly.

`run-all.sh` aggregates one exit-coded run per check into a single process, so it cannot pass all three
states through its own exit code the same way -- only "0 = nothing failed" survives. Skips and blocks
do not fail the run, only failures do. To tell "13 ok" apart from "11 ok + 2 skipped" without scraping
the whole summary, the final line is a greppable count of every outcome:

```
result: ok=13 skipped=2 blocked=0 failed=0
```

### Environment

| variable | default | effect |
|---|---|---|
| `CK` | auto-detected | path to `ck.exe` / `ck` |
| `EXT_NAME` | `daq_decoder` | service extension under test |
| `EXT_CKEXT` | newest `canking-service/dist/$EXT_NAME-*.ckext` | packaged extension installed before the run, if present |
| `EXT_WASM` | `target/wasm32-wasip2/release/$EXT_NAME.wasm` | wasm installed before the run when no `.ckext` is packaged |
| `EXT_DISPLAY_NAME` | `DAQ Decoder` | manifest display name the log assertions match on |
| `SETTLE_S` | `3` (`5` in `90-live-physical`) | seconds a live measurement runs |
| `REPLAY_MS` | `1` | fixed replay rate for recorded logs |
| `OUT_DIR` | `tests/ck-cli/out` | where derived artifacts go |
| `PHYS_CHANNEL` | auto-detected if exactly one physical channel exists | CAN channel number wired to the ECU, for `90-live-physical` |

---

### One worker at a time

`run-all.sh` mutates the shared CanKing measurement setup, so two concurrent runs on one machine corrupt each other.
Run the suite on a machine with no other CanKing sessions, one run at a time (or serialise runs with a lock). `checks/20-apply.sh`,
`checks/40-live.sh`, `checks/45-live-pid-remap.sh` and `checks/90-live-physical.sh` do refuse to run against a non-empty setup, which turns some collisions into skips rather than corruption.
