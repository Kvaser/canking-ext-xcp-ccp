#!/usr/bin/env bash
#
# Offline decode. This is the test of the extension's decoder.
#
# Runs the daq_decoder service extension over a log file via `ck analysis` and asserts the decoded signal values. 
# What is asserted depends on where the frames came from:
#
#   generated cases  assert/decode-synthetic.mjs, against the values util/make-fixture.mjs said its invented frames must produce.
#                    Proves the pipeline; both sides share one idea of the layout.
#   recorded cases   assert/decode-log.mjs, against values recomputed from the config plus expectations pinned in the case file. 
#                    The frames are real, so this is what proves the layout itself.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/xcp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"
EXT_NAME="${EXT_NAME:-daq_decoder}"

require_ck || exit $SKIP_EXIT

CONFIG="$OUT_DIR/$NAME.config.json"
EXPECTED="$OUT_DIR/$NAME.expected.json"
FRAMES="$OUT_DIR/$NAME.frames.json"
KIND="$(case_kind "$CASE_FILE")"

[ -f "$CONFIG" ]  || { echo "  FAILED  missing $CONFIG (run checks/10-config.sh first)"; exit 1; }
FIXTURE="$(case_log "$NAME")" \
    || { echo "  FAILED  no log for '$NAME' in $OUT_DIR (run checks/10-config.sh first)"; exit 1; }

SESSION=""
cleanup() {
    [ -n "$SESSION" ] && ck_ analysis close "$SESSION" > /dev/null 2>&1 || true
}
trap cleanup EXIT

# ---- open the log ------------------------------------------------------------
SESSION=$(ck_capture analysis open "$(winpath "$FIXTURE")" --json \
    | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).sessionId)}catch{process.exit(1)}})')
if [ -z "$SESSION" ]; then
    fail "ck analysis open did not return a session id"
    exit 1
fi
pass "opened analysis session $SESSION"

# ---- attach the extension ---------------------------------------------------
STEP="ext:name=$EXT_NAME;config=$(cat "$CONFIG")"
SET_OUT=$(ck_capture analysis set-processing "$SESSION" --step "$STEP" --json)
if printf '%s' "$SET_OUT" | grep -q '"error"'; then
    fail "set-processing reported an error:"
    detail_block "$SET_OUT"
    exit 1
fi
# Note: set-processing always reports "frameDefinitions": [] even on success.
pass "attached extension '$EXT_NAME' with generated config"

# ---- read back the decoded frames -------------------------------------------
ck_ analysis frames-between "$SESSION" --start-s 0 --end-s 3600 --max-count 10000 --json \
    > "$FRAMES" 2>&1
if ! node -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$FRAMES" 2>/dev/null; then
    fail "frames-between did not return JSON:"
    detail_block "$(head -5 "$FRAMES")"
    exit 1
fi

if [ "$KIND" = "recorded" ]; then
    if OUT=$(node "$CLI_DIR/assert/decode-log.mjs" "$CONFIG" "$FRAMES" "$CASE_FILE"); then
        pass "recorded DAQ frames decoded to the expected physical values"
    else
        fail "recorded DAQ frames did not decode as expected"
    fi
    detail_block "$OUT"

    # checks/40-live.sh replays this log and needs to know which payloads to expect.
    # For a generated case util/make-fixture.mjs wrote that file, but for a recorded one
    # it comes from the frames just read, so it is produced here
    if OUT=$(node "$CLI_DIR/util/replay-expect.mjs" "$CONFIG" "$FRAMES" --out "$EXPECTED"); then
        pass "wrote the replay expectation for the live check"
    else
        fail "could not derive a replay expectation from the recorded log"
    fi
    detail_block "$OUT"
elif OUT=$(node "$CLI_DIR/assert/decode-synthetic.mjs" "$EXPECTED" "$FRAMES"); then
    pass "decoded signal values match the fixture"
    detail_block "$OUT"
else
    fail "decoded signal values do not match the fixture"
    detail_block "$OUT"
fi

exit $((FAILURES > 0))
