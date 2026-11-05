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
EXT_NAME="${EXT_NAME:-daq_decoder}"
SETTLE_S="${SETTLE_S:-5}"
DP_NAME="DAQ Decoder $$"
EXT_DISPLAY_NAME="${EXT_DISPLAY_NAME:-DAQ Decoder}"
LOGNAME="live-physical"

require_ck || exit $SKIP_EXIT

CONFIG="$OUT_DIR/$NAME.config.json"
[ -f "$CONFIG" ] || { echo "  FAILED  missing $CONFIG (run checks/10-config.sh first)"; exit 1; }

chan=$(physical_channel); status=$?
case $status in
    0) ;;
    "$SKIP_EXIT") skip "$chan"; exit "$SKIP_EXIT" ;;
    *) fail "$chan"; exit 1 ;;
esac

# The extension picks an output channel matching the config's framing.
# CONNECT is the first command in both protocols' init sequence: XCP `ff 00`, CCP `01 ..`.
read -r _ NEEDS_FD BITRATE DBITRATE CROID DTO_ID CONNECT_BYTE < <(node -e '
const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));
const x=c.project.xcp_data?.xcp_info, k=c.project.ccp_data?.info;
const xcp=String(c.protocol).toLowerCase()==="xcp";
const fd=xcp?x?.can_fd:null;
process.stdout.write([xcp?"xcp":"ccp", fd?1:0, (xcp?x?.baudrate:k?.baudrate)||500000,
  fd?.data_baudrate||2000000, ((xcp?x?.can_id_master:k?.cro_id)||0).toString(16),
  ((xcp?x?.can_id_slave:k?.dto_id)||0).toString(16),
  xcp?"ff":"01"].join(" "));
' "$CONFIG")

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
if node "$CLI_DIR/assert/log-contains.mjs" "$EV" "$DP_NAME" \
        "Failed to open input channel" "Failed to open output channel" "Canlib error: [canOpenChannel]"; then
    skip "the local CAN channel could not be opened -- check it isn't held by another program" \
         "and that the adapter is still plugged in." \
         "See $EV for the canlib error. Not evidence of a decoder bug."
    exit $SKIP_EXIT
fi

# ---- step 2: init sequence out, DAQ data back --------------------------------
#
# Whether the extension *tried* to send the init sequence is checked in the service log.
if node "$CLI_DIR/assert/log-contains.mjs" "$EV" "$DP_NAME" "no channel available to send DAQ commands"; then
    fail "extension found no usable channel to send DAQ commands on"
    exit 1
fi
if ! node "$CLI_DIR/assert/log-contains.mjs" "$EV" "$DP_NAME" "queueing"; then
    fail "extension never queued the DAQ init sequence (expected 'queueing N DAQ init frames' in the service log)"
    exit 1
fi
pass "extension queued the DAQ init sequence"

if ! node "$CLI_DIR/assert/log-contains.mjs" "$EV" "$DP_NAME" "sending"; then
    fail "extension never sent the DAQ stop sequence on measurement stop (expected 'sending N DAQ stop frames' in the service log)"
    exit 1
fi
pass "extension sent the DAQ stop sequence"

# Captured rather than let print straight through: an empty result here isn't a failure.
if TX_OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --tx-on-id "$CROID" --min 1 --first-data-byte "$CONNECT_BYTE" 2>&1); then
    pass "...and it succesfully established connection on id 0x$CROID"
    detail_block "$TX_OUT"
else
    detail "note: no completed Tx frame on id 0x$CROID -- CAN needs another live node to ACK a" \
           "frame before it counts as sent, so this is expected with the ECU off/unwired."
fi

if ! RX_OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --rx-on-id "$DTO_ID" --min 1 2>&1); then
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
SESSION=""
close_session() { [ -n "$SESSION" ] && ck_ analysis close "$SESSION" > /dev/null 2>&1 || true; }
trap 'close_session; cleanup_set PHASE; cleanup_set CHANNELS' EXIT

SESSION=$(ck_capture analysis open "$(winpath "$LOG_OUT")" --json \
    | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).sessionId)}catch{process.exit(1)}})')
if [ -z "$SESSION" ]; then
    fail "ck analysis open did not return a session id"
    exit 1
fi

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

STEP="ext:name=$EXT_NAME;config=$(cat "$ANALYSIS_CONFIG")"
SET_OUT=$(ck_capture analysis set-processing "$SESSION" --step "$STEP" --json)
if printf '%s' "$SET_OUT" | grep -q '"error"'; then
    fail "set-processing reported an error:"
    detail_block "$SET_OUT"
    exit 1
fi

FRAMES="$OUT_DIR/$NAME.physical.frames.json"
ck_ analysis frames-between "$SESSION" --start-s 0 --end-s 3600 --max-count 10000 --json \
    > "$FRAMES" 2>&1
if ! node -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$FRAMES" 2>/dev/null; then
    fail "frames-between did not return JSON:"
    detail_block "$(head -5 "$FRAMES")"
elif DECODE_OUT=$(node "$CLI_DIR/assert/decode-log.mjs" "$ANALYSIS_CONFIG" "$FRAMES" "$CASE_FILE" --no-pin --min-daq-frames 1); then
    pass "DAQ frames from the ECU decoded consistently with the config's layout"
    detail_block "$DECODE_OUT"
else
    fail "DAQ frames from the ECU did not decode as the config's layout predicts"
    detail_block "$DECODE_OUT"
fi

exit $((FAILURES > 0))
