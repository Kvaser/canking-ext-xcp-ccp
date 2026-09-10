#!/usr/bin/env bash
# only-for-cases: ccp-sample
#
# CCP first-PID re-keying against a ECU report.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/ccp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"
EXT_NAME="${EXT_NAME:-daq_decoder}"
SETTLE_S="${SETTLE_S:-3}"
DP_NAME="DAQ Decoder $$"
REPORTED_PID=$((0x12))   # arbitrary, just needs to differ from the configured first PID

require_ck || exit $SKIP_EXIT

CONFIG="$OUT_DIR/$NAME.config.json"
[ -f "$CONFIG" ] || { echo "  FAILED  missing $CONFIG (run checks/10-config.sh first)"; exit 1; }

# ---- pick the DAQ list to mismatch, from the config itself ------------------
read -r PROTOCOL DTO_ID CRO_ID BITRATE TARGET_ID CONFIGURED_PID ODT_COUNT REASON < <(node -e '
const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
const bad = (reason) => { console.log(["-","-","-","-","-","-","-", reason].join(" ")); process.exit(0); };
if (String(c.protocol).toLowerCase() !== "ccp") bad("not-ccp");
const lists = c.daq_lists;
if (lists.length < 2) bad("too-few-daq-lists");
const info = c.project.ccp_data.info;
const t = lists[lists.length - 1];
console.log([c.protocol, info.dto_id, info.cro_id, info.baudrate, t.id, t.first_pid, t.odts.length, "ok"].join(" "));
' "$CONFIG")

if [ "$REASON" != "ok" ]; then
    skip "PID-remap check: case '$NAME' is $REASON (needs a CCP config with 2+ DAQ lists)"
    exit $SKIP_EXIT
fi
if [ "$CONFIGURED_PID" -eq "$REPORTED_PID" ]; then
    skip "PID-remap check: case '$NAME' already configures list $TARGET_ID at 0x$(printf '%02X' "$REPORTED_PID")"
    exit $SKIP_EXIT
fi

STARTDAQ_CFG="$(with_start_daq "$CONFIG" "$OUT_DIR/$NAME.startdaq.json")"

assert_setup_empty || exit $SKIP_EXIT

declare -a CHANNELS=()
declare -a PHASE=()
SESSION=""
cleanup_session() { [ -n "$SESSION" ] && ck_ analysis close "$SESSION" > /dev/null 2>&1 || true; }
trap 'cleanup_session; cleanup_set PHASE; cleanup_set CHANNELS' EXIT

pair=$(add_virtual_pair "$BITRATE")
case $? in
    0) ;;
    "$SKIP_EXIT") skip "$pair"; exit "$SKIP_EXIT" ;;
    *) fail "$pair"; exit 1 ;;
esac
read -r id_a id_b <<< "$pair"
track_node CHANNELS source "$id_a"
track_node CHANNELS source "$id_b"
pass "added CAN channels (CAN ${BITRATE})"

# ---- pass 1: discover the GET_DAQ_SIZE command counter -----------------
clear_logs "$OUT_DIR" "pidremap-discover"

add_node PHASE dataproc "add extension" \
    extension "$EXT_NAME" --name "$DP_NAME" --previous-node "$id_a" \
    --config "$(cat "$STARTDAQ_CFG")" || exit 1

add_node PHASE target "add messagelogger" \
    messagelogger --path "$(winpath "$OUT_DIR/pidremap-discover.asc")" --name "MsgLog" || exit 1

run_for_settle "pidremap-discover" || exit 1

# GET_DAQ_SIZE is 0x14, with the DAQ list id in byte 2 and the counter in byte 1.
CRO_CTR=$(node "$CLI_DIR/util/find-command.mjs" "$LOG_OUT" \
    --tx-on-id "$CRO_ID" --code 14 --match-byte "2=$TARGET_ID" --print-byte 1)

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
RESPONDER_OUT=$(node "$CLI_DIR/util/make-responder.mjs" --out "$RESPONDER_ASC" \
    --dto-id "$DTO_ID" --counter "$CRO_CTR" --odts "$ODT_COUNT" --first-pid "$REPORTED_PID" \
    --config "$STARTDAQ_CFG" --daq-id "$TARGET_ID" --expected-out "$EXPECTED")
detail_block "$RESPONDER_OUT"

SESSION=$(ck_capture analysis open "$(winpath "$RESPONDER_ASC")" --json \
    | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).sessionId)}catch{process.exit(1)}})')
if [ -z "$SESSION" ]; then
    fail "ck analysis open did not return a session id"
    exit 1
fi

STEP="ext:name=$EXT_NAME;config=$(cat "$STARTDAQ_CFG")"
SET_OUT=$(ck_capture analysis set-processing "$SESSION" --step "$STEP" --json)
if printf '%s' "$SET_OUT" | grep -q '"error"'; then
    fail "set-processing reported an error:"
    detail_block "$SET_OUT"
    exit 1
fi

FRAMES="$OUT_DIR/$NAME.pid-remap.frames.json"
ck_ analysis frames-between "$SESSION" --start-s 0 --end-s 3600 --max-count 10000 --json \
    > "$FRAMES" 2>&1
if ! node -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$FRAMES" 2>/dev/null; then
    fail "frames-between did not return JSON:"
    detail_block "$(head -5 "$FRAMES")"
    exit 1
fi

if OUT=$(node "$CLI_DIR/assert/decode-synthetic.mjs" "$EXPECTED" "$FRAMES"); then
    pass "decoder re-keyed daq list $TARGET_ID from 0x$CONFIGURED_HEX to 0x$REPORTED_HEX: the ECU's claimed PID decoded to that list's own signals"
else
    fail "extension did not re-key to the ECU-reported first PID"
fi
detail_block "$OUT"

exit $((FAILURES > 0))
