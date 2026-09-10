#!/usr/bin/env bash
#
# A2L to extension config JSON.
#
# Runs the same wasm component (a2l-daq.wasm) through generator.inspect + buildConfig, and
# asserts the returned configJson parses. Then produces the log every later check replays:
#
#   generated cases  util/make-fixture.mjs invents DTO frames from the config
#   recorded cases   the committed log is staged, and the one the config just built is
#                    asserted against the one the GUI wrote alongside it
#
# Runs once for all cases; every other check consumes its output.

set -u
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FAILURES=0
WASM_JS="$REPO_ROOT/canking-gui/src/a2l-daq-wasm/a2l_daq.js"

if [ ! -f "$WASM_JS" ]; then
    skip "a2l-daq wasm not transpiled; run 'npm run build:wasm' in canking-gui"
    exit $SKIP_EXIT
fi
if [ ! -d "$REPO_ROOT/canking-gui/node_modules" ]; then
    skip "canking-gui/node_modules missing; run 'npm install' in canking-gui"
    exit $SKIP_EXIT
fi

cases=("$@")
if [ ${#cases[@]} -eq 0 ]; then
    for r in "$CLI_DIR"/cases/*.case.json; do cases+=("$r"); done
fi

for case_file in "${cases[@]}"; do
    name="$(basename "$case_file" .case.json)"

    if node "$CLI_DIR/util/build-config.mjs" "$case_file" --out-dir "$OUT_DIR" --strict; then
        pass "$name: config JSON built from A2L"
    else
        fail "$name: config JSON build failed"
        continue
    fi

    if [ "$(case_kind "$case_file")" = "recorded" ]; then
        # The log and the config the GUI wrote with it, relative to tests/ck-cli.
        read -r log golden < <(node -e '
            const c = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
            console.log([c.recorded.log, c.recorded.config].join(" "))' "$case_file")
        log="$CLI_DIR/$log"
        golden="$CLI_DIR/$golden"

        if [ ! -f "$log" ]; then
            fail "$name: recorded log $log is missing"
            continue
        fi
        # Staged under the case name so case_log finds it
        cp "$log" "$OUT_DIR/$name.txt"
        pass "$name: staged recorded log ($(wc -l < "$OUT_DIR/$name.txt") lines)"

        if [ ! -f "$golden" ]; then
            fail "$name: recorded config $golden is missing"
        elif OUT=$(node "$CLI_DIR/assert/config-golden.mjs" "$OUT_DIR/$name.config.json" "$golden"); then
            pass "$name: config matches the one recorded with the log"
        else
            fail "$name: config differs from the one recorded with the log"
            detail_block "$OUT"
        fi
        continue
    fi

    if node "$CLI_DIR/util/make-fixture.mjs" "$OUT_DIR/$name.config.json" \
            --out-dir "$OUT_DIR" --name "$name"; then
        pass "$name: DTO fixture generated from config layout"
    else
        fail "$name: fixture generation failed"
    fi
done

exit $((FAILURES > 0))
