#!/usr/bin/env bash
#
# The "Apply" step.
#
# Attaches the extension to a source node with the generated config
# then reads the configuration back out of the measurement setup and asserts it
#
# Uses a Log Replay node as the upstream source.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
CASE_FILE="${1:-$CLI_DIR/cases/xcp-sample.case.json}"
NAME="$(basename "$CASE_FILE" .case.json)"

require_ck || exit $SKIP_EXIT
assert_setup_empty || exit $SKIP_EXIT

# The log is only there to give the data processor an upstream source.
need_outputs "$NAME" log || exit 1

declare -a NODES=()
trap 'cleanup_set NODES' EXIT

# ---- a source to hang the data processor off --------------------------------
add_node NODES source "could not add a Log Replay source" \
    logreplay '{"0":"00000000-0000-0000-0000-000000000000"}' \
    --path "$(winpath "$FIXTURE")" --name "ApplySrc" || exit 1
src="$NODE_ID"
pass "added Log Replay source node"

# ---- the Apply step ----------------------------------------------------------
add_node NODES dataproc "ck add extension" \
    extension "$EXT_NAME" --name "DAQ Decoder" --previous-node "$src" \
    --config "$(cat "$CONFIG")" || exit 1
dp="$NODE_ID"
pass "attached extension '$EXT_NAME' as a data processor"

# ---- read the config back out -----------------------------------------------
ck_ get nodes --dataprocessors --json --node "$dp" > "$OUT_DIR/$NAME.applied.json" 2>&1

if OUT=$(node "$CLI_DIR/assert/apply.mjs" "$CONFIG" "$OUT_DIR/$NAME.applied.json" "$EXT_NAME" "$dp"); then
    pass "configuration round-tripped through the measurement setup"
else
    fail "configuration did not round-trip"
fi
detail_block "$OUT"

exit $((FAILURES > 0))
