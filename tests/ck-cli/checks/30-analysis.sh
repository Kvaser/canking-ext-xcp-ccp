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
#                    This is what proves the layout itself.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/xcp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"

require_ck || exit $SKIP_EXIT
need_outputs "$NAME" log || exit 1

EXPECTED="$OUT_DIR/$NAME.expected.json"
FRAMES="$OUT_DIR/$NAME.frames.json"

analysis_frames "$FIXTURE" "$CONFIG" "$FRAMES" report || exit 1

if [ "$(case_kind "$CASE_FILE")" = "recorded" ]; then
    if OUT=$(node "$CLI_DIR/assert/decode-log.mjs" "$CONFIG" "$FRAMES" "$CASE_FILE" 2>&1); then
        pass "recorded DAQ frames decoded to the expected physical values"
    else
        fail "recorded DAQ frames did not decode as expected"
    fi
    detail_block "$OUT"

    # checks/40-live.sh replays this log and needs to know which payloads to expect.
    # For a generated case util/make-fixture.mjs wrote that file, but for a recorded one it comes from the frames just read.
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
