#!/usr/bin/env bash
# only-for-cases: ccp-sample
#
# CCP first-PID re-keying against a ECU report.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/ccp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"
DP_NAME="DAQ Decoder $$"
REPORTED_PID=$((0x12))   # arbitrary, just needs to differ from the configured first PID

require_ck || exit $SKIP_EXIT
need_outputs "$NAME" || exit 1

# ---- pick the DAQ list to mismatch, from the config itself ------------------
read -r TARGET_ID CONFIGURED_PID REASON < <(node -e '
const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const bad = (reason) => { console.log(["-", "-", reason].join(" ")); process.exit(0); };
if (String(c.protocol).toLowerCase() !== "ccp") bad("not-ccp");
const lists = c.daq_lists;
if (lists.length < 2) bad("too-few-daq-lists");
const t = lists[lists.length - 1];
console.log([t.id, t.first_pid, "ok"].join(" "));
' "$CONFIG")

if [ "$REASON" != "ok" ]; then
    skip "PID-remap check: case '$NAME' is $REASON (needs a CCP config with 2+ DAQ lists)"
    exit $SKIP_EXIT
fi
if [ "$CONFIGURED_PID" -eq "$REPORTED_PID" ]; then
    skip "PID-remap check: case '$NAME' already configures list $TARGET_ID at 0x$(printf '%02X' "$REPORTED_PID")"
    exit $SKIP_EXIT
fi

bus_params "$CONFIG"
STARTDAQ_CFG="$(with_start_daq "$CONFIG" "$OUT_DIR/$NAME.startdaq.json")"

assert_setup_empty || exit $SKIP_EXIT

declare -a CHANNELS=()
declare -a PHASE=()
trap 'cleanup_set PHASE; cleanup_set CHANNELS' EXIT

add_virtual_pair CHANNELS "$BITRATE" || exit $?
pass "added CAN channels (CAN ${BITRATE})"

# ---- pass 1: discover the GET_DAQ_SIZE command counter -----------------
clear_logs "$OUT_DIR" "pidremap-discover"

add_node PHASE dataproc "add extension" \
    extension "$EXT_NAME" --name "$DP_NAME" --previous-node "$VCAN_A" \
    --config "$(cat "$STARTDAQ_CFG")" || exit 1

add_node PHASE target "add messagelogger" \
    messagelogger --path "$(winpath "$OUT_DIR/pidremap-discover.asc")" --name "MsgLog" || exit 1

run_for_settle "pidremap-discover" || exit 1

# GET_DAQ_SIZE is 0x14, with the DAQ list id in byte 2 and the counter in byte 1.
CRO_CTR=$(node "$CLI_DIR/assert/asc.mjs" "$LOG_OUT" \
    --tx-on-id "$CROID" --match-byte 0=0x14 --match-byte "2=$TARGET_ID" --print-byte 1)

if [ -z "$CRO_CTR" ]; then
    fail "could not find GET_DAQ_SIZE for daq list $TARGET_ID in the discovery pass ($LOG_OUT)"
    exit 1
fi
pass "found GET_DAQ_SIZE(daq=$TARGET_ID) using command counter $CRO_CTR"

# The bus and this pass's nodes were only needed to observe that counter.
cleanup_set PHASE
cleanup_set CHANNELS

# ---- pass 2: answer it with a different first PID, then decode against it ---
CONFIGURED_HEX=$(printf '%02X' "$CONFIGURED_PID")
REPORTED_HEX=$(printf '%02X' "$REPORTED_PID")

RESPONDER_ASC="$OUT_DIR/$NAME.pid-remap-responder.asc"
EXPECTED="$OUT_DIR/$NAME.pid-remap.expected.json"
RESPONDER_OUT=$(node "$CLI_DIR/util/make-responder.mjs" --config "$STARTDAQ_CFG" --daq-id "$TARGET_ID" \
    --counter "$CRO_CTR" --first-pid "$REPORTED_PID" --out "$RESPONDER_ASC" --expected-out "$EXPECTED")
detail_block "$RESPONDER_OUT"

FRAMES="$OUT_DIR/$NAME.pid-remap.frames.json"
analysis_frames "$RESPONDER_ASC" "$STARTDAQ_CFG" "$FRAMES" || exit 1

if OUT=$(node "$CLI_DIR/assert/decode-synthetic.mjs" "$EXPECTED" "$FRAMES"); then
    pass "decoder re-keyed daq list $TARGET_ID from 0x$CONFIGURED_HEX to 0x$REPORTED_HEX: the ECU's claimed PID decoded to that list's own signals"
else
    fail "extension did not re-key to the ECU-reported first PID"
fi
detail_block "$OUT"

exit $((FAILURES > 0))
