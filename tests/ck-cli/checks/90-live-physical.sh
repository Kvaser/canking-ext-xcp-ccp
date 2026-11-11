#!/usr/bin/env bash
# only-for-cases: live-physical
#
# Sanity check against a real ECU on the bench, and an example for writing your own.
#
# Runs only for a case named `live-physical`, which is not shipped.
# Write one for the connected ECU, keep it (and its A2L) outside the repo its "a2l" may be an absolute path, and pass it to run-all.sh along with the shipped cases:
#
#   PHYS_CHANNEL=0 tests/ck-cli/run-all.sh tests/ck-cli/cases/*.case.json /path/to/live-physical.case.json
#
# Bus settings (bit rate, CAN FD data rate) come from the config, i.e. from the A2L. PHYS_BITRATE and PHYS_DATA_BITRATE override them when the bench runs differently.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:?usage: live-physical.sh <case.json describing the connected ECU>}"
NAME="$(basename "$CASE_FILE" .case.json)"
SETTLE_S="${SETTLE_S:-5}"
DP_NAME="DAQ Decoder $$"
EXT_DISPLAY_NAME="${EXT_DISPLAY_NAME:-DAQ Decoder}"
LOGNAME="live-physical"

require_ck || exit $SKIP_EXIT
need_outputs "$NAME" || exit 1

chan=$(physical_channel); status=$?
case $status in
    0) ;;
    "$SKIP_EXIT") skip "$chan"; exit "$SKIP_EXIT" ;;
    *) fail "$chan"; exit 1 ;;
esac

bus_params "$CONFIG"

# An A2L's bus settings don't always match the bench; PHYS_BITRATE / PHYS_DATA_BITRATE (implies CAN FD) override them.
BITRATE="${PHYS_BITRATE:-$BITRATE}"
if [ -n "${PHYS_DATA_BITRATE:-}" ]; then NEEDS_FD=1; DBITRATE="$PHYS_DATA_BITRATE"; fi

fdargs=()
[ "$NEEDS_FD" = "1" ] && fdargs=(--bit-rate-fd "$DBITRATE")

assert_setup_empty || exit $SKIP_EXIT

declare -a CHANNELS=()
declare -a PHASE=()
trap 'cleanup_set PHASE; cleanup_set CHANNELS' EXIT

add_node CHANNELS source "add canchannel" \
    canchannel --number "$chan" --name "PHYS_ECU" -r "$BITRATE" "${fdargs[@]+"${fdargs[@]}"}" || exit 1
chan_id="$NODE_ID"
pass "opened physical channel $chan ($([ "$NEEDS_FD" = 1 ] && echo "CAN FD ${BITRATE}/${DBITRATE}" || echo "CAN ${BITRATE}"))"

# with start_daq the extension transmits the XCP/CCP command sequence on the CRO id at measurement start.
STARTDAQ_CFG="$(with_start_daq "$CONFIG" "$OUT_DIR/$NAME.startdaq.json")"

clear_logs "$OUT_DIR" "$LOGNAME"

add_node PHASE dataproc "add extension" \
    extension "$EXT_NAME" --name "$DP_NAME" --previous-node "$chan_id" \
    --config "$(cat "$STARTDAQ_CFG")" || exit 1

add_node PHASE target "add messagelogger" \
    messagelogger --path "$(winpath "$OUT_DIR/$LOGNAME.asc")" --name "MsgLog" || exit 1

run_for_settle "$LOGNAME" || exit 1

# ---- step 1: the local channel actually opened -------------------------------
EV=$(capture_event_log "$OUT_DIR" "$LOGNAME")
if [ -z "$EV" ]; then
    fail "could not capture the service event log"
    exit 1
fi
if node "$CLI_DIR/assert/eventlog.mjs" "$EV" "$DP_NAME" --contains \
        "Failed to open input channel" "Failed to open output channel" "Canlib error: [canOpenChannel]"; then
    skip "the local CAN channel could not be opened -- check it isn't held by another program" \
         "and that the adapter is still plugged in." \
         "See $EV for the canlib error. Not evidence of a decoder bug."
    exit $SKIP_EXIT
fi

# ---- step 2: init sequence out, DAQ data back --------------------------------
#
# Whether the extension *tried* to send the init sequence is checked in the service log.
if node "$CLI_DIR/assert/eventlog.mjs" "$EV" "$DP_NAME" --contains "no channel available to send DAQ commands"; then
    fail "extension found no usable channel to send DAQ commands on"
    exit 1
fi
if ! node "$CLI_DIR/assert/eventlog.mjs" "$EV" "$DP_NAME" --contains "queueing"; then
    fail "extension never queued the DAQ init sequence (expected 'queueing N DAQ init frames' in the service log)"
    exit 1
fi
pass "extension queued the DAQ init sequence"

if ! node "$CLI_DIR/assert/eventlog.mjs" "$EV" "$DP_NAME" --contains "sending"; then
    fail "extension never sent the DAQ stop sequence on measurement stop (expected 'sending N DAQ stop frames' in the service log)"
    exit 1
fi
pass "extension sent the DAQ stop sequence"

# Captured rather than let print straight through: an empty result here isn't a failure.
if TX_OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --tx-on-id "$CROID" --first-data-byte "$CONNECT_BYTE" 2>&1); then
    pass "...and it succesfully established connection on id 0x$CROID"
    detail_block "$TX_OUT"
else
    detail "note: no completed Tx frame on id 0x$CROID -- CAN needs another live node to ACK a" \
           "frame before it counts as sent, so this is expected with the ECU off/unwired."
fi

if ! RX_OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --rx-on-id "$DTO_ID" 2>&1); then
    skip "no DAQ frames came back from the ECU on id 0x$DTO_ID -- check it is powered, wired to" \
         "channel $chan, and that the config's bitrate/CAN-FD settings match."
    exit $SKIP_EXIT
fi
pass "ECU answered and streamed DAQ frames back on id 0x$DTO_ID"
detail_block "$RX_OUT"

# Check the log for unexpected warnings
EVLOG_OUT=$(node "$CLI_DIR/assert/eventlog.mjs" "$EV" "$DP_NAME" --prefix "$EXT_DISPLAY_NAME")
case $? in
    0)  pass "extension logged no unexpected warnings" ;;
    77) skip "extension log assertions (service log does not cover this run)" ;;
    *)  fail "extension log shows a problem" ;;
esac
detail_block "$EVLOG_OUT"

# ---- step 3: what came back decodes correctly --------------------------------
# The live decoder re-keys a list whose first PID the ECU reports differently (logged above), but the offline  analysis sends no commands and can't learn that.
# Carry this run's logged corrections into the config it decodes with.
ANALYSIS_CONFIG="$OUT_DIR/$NAME.physical.config.json"
node -e '
const fs = require("fs");
const [cfgPath, logPath, node, out] = process.argv.slice(1);
const cfg = JSON.parse(fs.readFileSync(cfgPath, "utf8"));
const log = fs.readFileSync(logPath, "latin1");
const run = log.slice(log.lastIndexOf(`Added new data processor '"'"'${node}'"'"'`));
for (const m of run.matchAll(/DAQ list (\d+) starts at PID 0x([0-9A-Fa-f]+)/g)) {
  const list = cfg.daq_lists.find((d) => d.id === Number(m[1]));
  if (list) list.first_pid = parseInt(m[2], 16);
}
fs.writeFileSync(out, JSON.stringify(cfg));
' "$CONFIG" "$EV" "$DP_NAME" "$ANALYSIS_CONFIG"

FRAMES="$OUT_DIR/$NAME.physical.frames.json"
analysis_frames "$LOG_OUT" "$ANALYSIS_CONFIG" "$FRAMES" || exit 1

if DECODE_OUT=$(node "$CLI_DIR/assert/decode-log.mjs" "$ANALYSIS_CONFIG" "$FRAMES" "$CASE_FILE" --no-pin --min-daq-frames 1 2>&1); then
    pass "DAQ frames from the ECU decoded consistently with the config's layout"
    detail_block "$DECODE_OUT"
else
    fail "DAQ frames from the ECU did not decode as the config's layout predicts"
    detail_block "$DECODE_OUT"
fi

exit $((FAILURES > 0))
