#!/usr/bin/env bash
#
# Runs every CanKing ck-cli check, keeping going after a failure, then prints a
# summary. Mirrors the style of the repo-root run.sh.
#
# Usage:
#   ./run-all.sh [case.json ...]      (default: every cases/*.case.json)
#
# Environment:
#   CK          path to ck.exe / ck            (auto-detected)
#   EXT_NAME    service extension name         (default: daq_decoder)
#   SETTLE_S    seconds to let a measurement run (default: 3)
#   OUT_DIR     directory for generated artifacts (default: tests/ck-cli/out)

set -u
INVOKED_FROM="$PWD"
cd "$(dirname "$0")"
source ./lib.sh

# Case paths on the command line are relative to where this was run from
cases=()
for c in "$@"; do
    case "$c" in
        /* | [A-Za-z]:[/\\]*) cases+=("$c") ;;
        *)                    cases+=("$INVOKED_FROM/$c") ;;
    esac
done
if [ ${#cases[@]} -eq 0 ]; then
    for r in ./cases/*.case.json; do cases+=("$r"); done
fi

RESULTS=()
run() {  # run <label> <script> [args...]
    local label="$1" status
    shift
    echo
    echo "=== ${label} ==="
    bash "$@"
    status=$?
    case "$status" in
        0)           RESULTS+=("ok      $label") ;;
        "$SKIP_EXIT") RESULTS+=("skipped $label") ;;
        *)           RESULTS+=("FAILED  $label") ;;
    esac
    return "$status"
}

echo "ck:        ${CK_BIN:-<not found>}"
echo "extension: ${EXT_NAME:-daq_decoder}"
echo "out dir:   $OUT_DIR"

# Every per-case check shells out to ck, so nothing after setup can do anything useful without it.
CK_OK=0
if [ -n "${CK_BIN:-}" ] && ck_ get about > /dev/null 2>&1; then
    CK_OK=1
    # Make sure the extension under test is the one that was just built, not whatever an earlier run left installed.
    install_extension
    export EXT_NAME="${EXT_NAME:-daq_decoder}"
else
    echo
    echo "ck not found or unreachable; every per-case check needs it, so all are blocked"
    echo "(set CK=/path/to/ck.exe, or start the CanKing service, then re-run)"
fi

# Every check in checks/ is discovered by glob and ordered by its numeric prefix
# (keep prefixes two digits, spaced by 10, so lexical sort matches numeric order
# and there's room to insert one later without renumbering).
#
# The lowest-numbered check builds the config + fixture every other check consumes, so it alone runs once up front for all cases
# and everything after it runs once per case.
checks=(./checks/*.sh)
setup_check="${checks[0]}"
percase_checks=("${checks[@]:1}")

label_of() { basename "${1%.sh}"; }   # ./checks/10-config.sh -> 10-config

run "$(label_of "$setup_check"): A2L -> config JSON + fixture" "$setup_check" "${cases[@]}"
setup_status=$?

if [ "$setup_status" -eq "$SKIP_EXIT" ]; then
    # A blanket prerequisite gap (wasm not built, node_modules missing): setup
    # bailed before touching a single case, so nothing below can run.
    echo
    echo "setup skipped; every per-case check is blocked"
    for case_file in "${cases[@]}"; do
        name="$(basename "$case_file" .case.json)"
        for check in "${percase_checks[@]}"; do
            RESULTS+=("blocked $(label_of "$check") [$name] (setup skipped)")
        done
    done
elif [ "$CK_OK" -eq 0 ]; then
    for case_file in "${cases[@]}"; do
        name="$(basename "$case_file" .case.json)"
        for check in "${percase_checks[@]}"; do
            RESULTS+=("blocked $(label_of "$check") [$name] (ck unavailable)")
        done
    done
else
    for case_file in "${cases[@]}"; do
        name="$(basename "$case_file" .case.json)"

        # Every per-case check consumes the setup check's output.
        # A recorded case has its log staged instead of a generated fixture, and its expected values live in the case file.
        required=("$name.config.json")
        if [ "$(case_kind "$case_file")" = "recorded" ]; then
            required+=("$name.txt")
        else
            required+=("$name.asc" "$name.expected.json")
        fi
        missing=""
        for f in ${required[@]+"${required[@]}"}; do
            [ -f "$OUT_DIR/$f" ] || missing="$missing $f"
        done
        if [ -n "$missing" ]; then
            echo
            echo "=== [$name] ==="
            echo "  blocked per-case checks: setup produced no$missing"
            RESULTS+=("blocked checks [$name] (setup failed for this case)")
            continue
        fi

        for check in "${percase_checks[@]}"; do
            label="$(label_of "$check")"

            # A check may restrict itself to specific cases with a header comment:
            #   # only-for-cases: name-one name-two
            # No such comment means every case
            restriction=$(grep -m1 '^# only-for-cases:' "$check" || true)
            if [ -n "$restriction" ]; then
                names=${restriction#'# only-for-cases:'}
                match=0
                for allowed in $names; do
                    [ "$allowed" = "$name" ] && { match=1; break; }
                done
                [ "$match" -eq 1 ] || continue
            fi
            run "$label [$name]" "$check" "$case_file"
        done
    done
fi

echo
echo "==================== summary ===================="
for r in ${RESULTS[@]+"${RESULTS[@]}"}; do echo "  $r"; done
echo "================================================="

ok_n=0; skip_n=0; block_n=0; fail_n=0
for r in ${RESULTS[@]+"${RESULTS[@]}"}; do
    case "$r" in
        ok\ *)      ok_n=$((ok_n + 1)) ;;
        skipped\ *) skip_n=$((skip_n + 1)) ;;
        blocked\ *) block_n=$((block_n + 1)) ;;
        FAILED\ *)  fail_n=$((fail_n + 1)) ;;
    esac
done
echo "result: ok=$ok_n skipped=$skip_n blocked=$block_n failed=$fail_n"

[ "$fail_n" -eq 0 ]
