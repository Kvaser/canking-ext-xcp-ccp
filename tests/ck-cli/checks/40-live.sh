#!/usr/bin/env bash
#
# Live measurement over two virtual CAN channels.
#
# Complements checks/30-analysis.sh to test everything except decode correctness.
# Two virtual channels loop back to each other. Log replay sends to one of them and the other has the decoder attached.
# Skips with a warning when fewer than two virtual channels are available.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/xcp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"
DP_NAME="DAQ Decoder $$"

require_ck || exit $SKIP_EXIT
need_outputs "$NAME" log || exit 1

EXPECTED="$OUT_DIR/$NAME.expected.json"

# Which check produced the expectation depends on the case kind.
if [ ! -f "$EXPECTED" ]; then
    if [ "$(case_kind "$CASE_FILE")" = "recorded" ]; then
        skip "no replay expectation at $EXPECTED; run checks/30-analysis.sh for this case first"
    else
        skip "no replay expectation at $EXPECTED; run checks/10-config.sh for this case first"
    fi
    exit $SKIP_EXIT
fi

# Replay every thing at the same speed, ignoring the log timestamps
replayargs=()
[ "$(case_kind "$CASE_FILE")" = "recorded" ] && replayargs=(--samplerate "${REPLAY_MS:-1}")

assert_setup_empty || exit $SKIP_EXIT

declare -a CHANNELS=()   # kept across both phases
declare -a PHASE=()      # torn down and re-populated per phase

cleanup_all() {
    cleanup_set PHASE
    cleanup_set CHANNELS
}
trap cleanup_all EXIT

bus_params "$CONFIG"

fdargs=()
[ "$NEEDS_FD" = "1" ] && fdargs=(--bit-rate-fd "$DBITRATE")

add_virtual_pair CHANNELS "$BITRATE" "${fdargs[@]+"${fdargs[@]}"}" || exit $?
pass "added CAN channels ($([ "$NEEDS_FD" = 1 ] && echo "CAN FD ${BITRATE}/${DBITRATE}" || echo "CAN ${BITRATE}"))"

# ---- phase A: transport ------------------------------------------------------
run_phase() {  # run_phase <config-path> <log-basename> <start-replay:0|1>
    local cfg="$1" logname="$2" replay="$3"
    local key

    # Start from a clean slate so exactly one logger file can match afterwards.
    clear_logs "$OUT_DIR" "$logname"

    add_node PHASE dataproc "add extension" \
        extension "$EXT_NAME" --name "$DP_NAME" --previous-node "$VCAN_A" \
        --config "$(cat "$cfg")" || return 1

    add_node PHASE target "add messagelogger" \
        messagelogger --path "$(winpath "$OUT_DIR/$logname.asc")" --name "MsgLog" || return 1

    if [ "$replay" = "1" ]; then
        # The channel-map key is the LOG FILE's internal channel index.
        key=$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).logChannelKey)' "$EXPECTED")
        add_node PHASE source "add logreplay" \
            logreplay "{\"$key\":\"$VCAN_B\"}" --path "$(winpath "$FIXTURE")" \
            --name "Replay" -i 1 ${replayargs[@]+"${replayargs[@]}"} || return 1
        run_for_settle "$logname" "Replay"
    else
        run_for_settle "$logname"
    fi
}

if run_phase "$CONFIG" "live-transport" 1; then
    if OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --rx-payloads "$EXPECTED"); then
        pass "replayed DAQ frames crossed the virtual bus and reached the extension's channel"
    else
        fail "replayed DAQ frames did not arrive as expected"
    fi
    detail_block "$OUT"
fi
cleanup_set PHASE   # tear down phase A, keep the channels for phase B

# ---- phase B: DAQ init sequence ---------------------------------------------
# with start_daq the extension transmits the XCP/CCP command sequence on the CRO id at measurement start.
STARTDAQ_CFG="$(with_start_daq "$CONFIG" "$OUT_DIR/$NAME.startdaq.json")"

if [ "$CROID" = "0" ]; then
    skip "init sequence: config declares no command CAN id"
elif run_phase "$STARTDAQ_CFG" "live-init" 0; then
    if OUT=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" --tx-on-id "$CROID" --first-data-byte "$CONNECT_BYTE"); then
        pass "extension transmitted the DAQ init sequence on id 0x$CROID, leading with CONNECT"
    else
        fail "extension did not transmit the DAQ init sequence"
    fi
    detail_block "$OUT"
fi

exit $((FAILURES > 0))
