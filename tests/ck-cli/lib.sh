#!/usr/bin/env bash
#
# Shared helpers for the CanKing ck-cli tests.
#

set -u

# The extension under test. run-all.sh may change it to the name the freshly built one installs under.
export EXT_NAME="${EXT_NAME:-daq_decoder}"

# ---- locating ck -------------------------------------------------------------

find_ck() {
    if [ -n "${CK:-}" ]; then printf '%s' "$CK"; return 0; fi
    local candidates=(
        "/c/Program Files/Kvaser CanKing/service/ck.exe"
        "C:/Program Files/Kvaser CanKing/service/ck.exe"
        "/opt/canking/service/ck"
        "$(command -v ck 2>/dev/null || true)"
    )
    for c in "${candidates[@]}"; do
        [ -n "$c" ] && [ -x "$c" ] && printf '%s' "$c" && return 0
    done
    return 1
}

CK_BIN="$(find_ck || true)"

ck_() {
    "$CK_BIN" "$@" < /dev/null
}

ck_capture() { ck_ "$@" 2>&1; }

require_ck() {
    if [ -z "$CK_BIN" ]; then
        echo "SKIP: ck not found (set CK=/path/to/ck.exe)" >&2
        return 1
    fi
    if ! ck_ get about > /dev/null 2>&1; then
        echo "SKIP: CanKing service not reachable" >&2
        return 1
    fi
    return 0
}

# ---- paths -------------------------------------------------------------------

CLI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$CLI_DIR/../.." && pwd)"
OUT_DIR="${OUT_DIR:-$CLI_DIR/out}"
mkdir -p "$OUT_DIR"

# Always hands back an absolute path
winpath() {
    local p="$1"
    case "$p" in
        /*|?:*|?:\\*) ;;
        *) p="$PWD/$p" ;;
    esac
    if command -v cygpath > /dev/null 2>&1; then
        cygpath -w "$p"
    else
        printf '%s' "$p"
    fi
}

# ---- case files --------------------------------------------------------------
#
# Two kinds of case live in cases/:
#
#   generated  an A2L plus a signal/event selection. checks/10-config.sh builds the config and util/make-fixture.mjs invents DTO frames from it.
#
#   recorded   the same, plus a "recorded" block naming a log captured off a real ECU and the config the GUI wrote during that session.
#
# Both end up with a config at $OUT_DIR/<name>.config.json and a replayable log, which is all the checks after 10-config need.

case_kind() {  # case_kind <case-file>  -> generated | recorded
    node -e 'const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));
             process.stdout.write(c.recorded ? "recorded" : "generated")' "$1"
}

# The log a case replays: the generated fixture, or the recorded log staged by
# checks/10-config.sh. Prints nothing and returns 1 when neither is there.
case_log() {  # case_log <name>
    local name="$1" f
    for f in "$OUT_DIR/$name.asc" "$OUT_DIR/$name.txt"; do
        [ -f "$f" ] && printf '%s' "$f" && return 0
    done
    return 1
}

# What checks/10-config.sh left for case <name>: sets CONFIG and, given "log", FIXTURE (the log case_log finds).
# Prints a failure and returns 1 when one is missing.
need_outputs() {  # need_outputs <name> [log]
    CONFIG="$OUT_DIR/$1.config.json"
    if [ ! -f "$CONFIG" ]; then
        fail "missing $CONFIG (run checks/10-config.sh first)"
        return 1
    fi
    if [ -n "${2:-}" ] && ! FIXTURE="$(case_log "$1")"; then
        fail "no log for '$1' in $OUT_DIR (run checks/10-config.sh first)"
        return 1
    fi
}

# ---- CAN channel discovery ---------------------------------------------------

# The canlib channel numbers `ck get devices` lists, one per line. The JSON is kept in <out-file>.
device_channels() {  # device_channels <out-file> [ck get devices args...]
    local out="$1"
    shift
    ck_ get devices --json "$@" > "$out" 2>/dev/null || return 1
    node -e '
        const devs = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
        for (const d of devs) for (const c of d.channels ?? []) console.log(c.channelNumber);
    ' "$out"
}

virtual_channels() {  # the virtual channel numbers, ascending
    local phys
    phys=$(device_channels "$OUT_DIR/dev-phys.json" --no-virtuals) || return 1
    device_channels "$OUT_DIR/dev-all.json" | grep -vxF "$phys" | sort -n
}

# ---- measurement setup cleanup ----------------------------------------------
#
# A check registers ids it creates into an array it owns, then tears that arraydown with cleanup_set.
# Separate sets let a multi-phase check tear down one phase's nodes while keeping another's
# (For example, drop a data processor between phases, keep the CAN channels).
#
#   declare -a MY_SET=()
#   track_node MY_SET dataproc "$dp"
#   cleanup_set MY_SET
#
# <kind> is whatever `ck delete <kind> <id>` expects: target, dataproc, source.

track_node() {  # track_node <set-var-name> <kind> <id>
    local -n _set="$1"
    _set+=("$2:$3")
}

# Add a node and register it for cleanup in one step.
#
#   add_node PHASE dataproc "add extension" extension "$EXT_NAME" --name ... --config ...
#
# Runs `ck add <args...>`, checks the result really is a node id, and tracks it.
#
# Prints a message and returns 1 if ck did not hand back an id.
add_node() {  # add_node <set-var-name> <kind> <label> <ck add args...>
    local set_name="$1" kind="$2" label="$3"
    shift 3
    NODE_ID=$(ck_capture add "$@" | tr -d '\r')
    if ! printf '%s' "$NODE_ID" | grep -qE '^[0-9a-fA-F-]{36}$'; then
        fail "$label failed: $NODE_ID"
        NODE_ID=""
        return 1
    fi
    track_node "$set_name" "$kind" "$NODE_ID"
    return 0
}

cleanup_set() {  # cleanup_set <set-var-name>
    local -n _set="$1"
    local entry
    for entry in ${_set[@]+"${_set[@]}"}; do
        ck_ delete "${entry%%:*}" "${entry#*:}" > /dev/null 2>&1 || true
    done
    _set=()
}

# ---- physical CAN channel -----------------------------------------------------
#
# For tests against a real ECU connected to the computer through usb. PHYS_CHANNEL picks which one.
# Auto-detection only works if the box happens to expose exactly one non-virtual channel  or more, so in practice PHYS_CHANNEL needs to be set.
#
#   chan=$(physical_channel); status=$?
#   case $status in
#       0)  track_node CHANNELS source "$(ck_ add canchannel --number "$chan" ...)" ;;
#       "$SKIP_EXIT") skip "$chan"; exit "$SKIP_EXIT" ;;
#   esac
physical_channel() {
    if [ -n "${PHYS_CHANNEL:-}" ]; then printf '%s' "$PHYS_CHANNEL"; return 0; fi
    local list
    list=$(device_channels "$OUT_DIR/dev-phys.json" --no-virtuals) \
        || { printf 'could not query physical CAN devices from the service\n'; return 1; }
    local chans=($list)
    case "${#chans[@]}" in
        0)
            printf 'no physical CAN channel found. Connect the adapter wired to the ECU.\n'
            return "$SKIP_EXIT" ;;
        1)
            printf '%s' "${chans[0]}" ;;
        *)
            printf 'found %d physical CAN channels (%s); set PHYS_CHANNEL=<n> to pick the one wired to the ECU\n' \
                "${#chans[@]}" "${chans[*]}"
            return "$SKIP_EXIT" ;;
    esac
}

# ---- paired virtual CAN channels ----------------------------------------------
#
# Adds two virtual channels, "VCAN_A" and "VCAN_B", tracked in <set-var-name>, and sets VCAN_A / VCAN_B to their node ids.
# Otherwise prints why and returns $SKIP_EXIT (fewer than two virtual channels found) or 1 (`ck add canchannel` failed):
#
#   add_virtual_pair CHANNELS "$BITRATE" || exit $?
add_virtual_pair() {  # add_virtual_pair <set-var-name> <bitrate> [canchannel args...]
    local set_name="$1" bitrate="$2" vchans
    shift 2
    mapfile -t vchans < <(virtual_channels)
    if [ "${#vchans[@]}" -lt 2 ]; then
        skip "found ${#vchans[@]} virtual CAN channel(s), need 2. Install/enable the Kvaser Virtual CAN Driver to run this test."
        return "$SKIP_EXIT"
    fi
    add_node "$set_name" source "add canchannel" \
        canchannel --number "${vchans[0]}" --name "VCAN_A" -r "$bitrate" "$@" || return 1
    VCAN_A="$NODE_ID"
    add_node "$set_name" source "add canchannel" \
        canchannel --number "${vchans[1]}" --name "VCAN_B" -r "$bitrate" "$@" || return 1
    VCAN_B="$NODE_ID"
}

# Refuse to run against a setup that already has work in it
assert_setup_empty() {
    local n
    n=$(ck_capture get nodes --sources --dataprocessors 2>/dev/null | grep -c -- '-- Identifier:' || true)
    if [ "${n:-0}" -ne 0 ]; then
        echo "SKIP: CanKing measurement setup is not empty ($n node(s));" >&2
        echo "      clear it first so the test does not disturb your work." >&2
        return 1
    fi
    return 0
}

# ---- the extension under test ------------------------------------------------
# Install the freshly built extension so what runs is what was just built.
# Prefers the packaged .ckext, falls back to the raw wasm when no .ckext has been packaged.

install_extension() {
    local ext="$EXT_NAME"
    local wasm="${EXT_WASM:-$REPO_ROOT/target/wasm32-wasip2/release/$ext.wasm}"
    local ckext="${EXT_CKEXT:-$(ls -t "$REPO_ROOT/canking-service/dist/${ext}"-*.ckext 2>/dev/null | head -n 1)}"
    local artifact out registered

    if [ -n "$ckext" ] && [ -f "$ckext" ]; then
        artifact="$ckext"
        # A .ckext installs under its manifest's declared name (== $ext), regardless of filename.
        registered="$ext"
    elif [ -f "$wasm" ]; then
        artifact="$wasm"
        # A bare .wasm installs under its FILENAME stem
        registered="$(basename "$artifact")"
        registered="${registered%.wasm}"
    else
        echo "  note: no packaged .ckext and $wasm not built (run './run.sh wasm' or './run.sh pack');"
        echo "        testing whatever '$ext' is currently installed."
        return 0
    fi

    if out=$(ck_capture install extension "$(winpath "$artifact")" --replace); then
        EXT_NAME="$registered"
        echo "  installed '$EXT_NAME' from ${artifact#"$REPO_ROOT/"}"
    else
        echo "  note: could not install '$ext' ($out);"
        echo "        testing whatever version is currently installed."
    fi
    return 0
}

# ---- the config --------------------------------------------------------------

# The same config with start_daq turned on, written beside the original.
with_start_daq() {  # with_start_daq <config-path> <out-path>
    node -e '
        const fs = require("fs");
        const c = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
        c.start_daq = true;
        fs.writeFileSync(process.argv[2], JSON.stringify(c));
    ' "$1" "$2" || return 1
    printf '%s' "$2"
}

# The config's bus settings: sets NEEDS_FD (0|1), BITRATE, DBITRATE, CROID and DTO_ID (hex), and CONNECT_BYTE.
# The extension picks an output channel matching the config's framing.
# CONNECT is the first command in both protocols' init sequence: XCP `ff 00`, CCP `01 ..`.
bus_params() {  # bus_params <config>
    read -r NEEDS_FD BITRATE DBITRATE CROID DTO_ID CONNECT_BYTE < <(node -e '
const c=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"));
const x=c.project.xcp_data?.xcp_info, k=c.project.ccp_data?.info;
const xcp=String(c.protocol).toLowerCase()==="xcp";
const fd=xcp?x?.can_fd:null;
process.stdout.write([fd?1:0, (xcp?x?.baudrate:k?.baudrate)||500000,
  fd?.data_baudrate||2000000, ((xcp?x?.can_id_master:k?.cro_id)||0).toString(16),
  ((xcp?x?.can_id_slave:k?.dto_id)||0).toString(16),
  xcp?"ff":"01"].join(" "));
' "$1")
}

# ---- logger output files -----------------------------------------------------

clear_logs() {  # clear_logs <dir> <prefix>
    rm -f "$1/$2"_*.asc 2>/dev/null || true
}

newest_log() {  # newest_log <dir> <prefix>  -> highest-numbered <prefix>_N.asc
    ls -v "$1/$2"_*.asc 2>/dev/null | grep -E '_[0-9]+\.asc$' | tail -n 1
}

# Run the measurement that is already set up for SETTLE_S, then hand back the log the message logger wrote.
#
#   run_for_settle live-transport Replay
#
# Starts the measurement, starts the named log replay node if given, waits, stops, and waits for the logger to finish flushing.
# The path lands in LOG_OUT. Prints a message and returns 1 if no usable log appeared.
#
# A third argument names a function to call right before the stop, e.g. to send a protocol-level shutdown while the session is still live.
run_for_settle() {  # run_for_settle <logname> [replay-node-name] [pre-stop-hook]
    local logname="$1" replay="${2:-}" pre_stop="${3:-}"

    ck_ start meas > /dev/null 2>&1
    [ -n "$replay" ] && ck_ start logreplay "$replay" > /dev/null 2>&1
    sleep "${SETTLE_S:-3}"
    [ -n "$pre_stop" ] && "$pre_stop"
    ck_ stop meas > /dev/null 2>&1

    # The logger finishes writing asynchronously after stop, so wait for its end-of-file marker
    if LOG_OUT=$(wait_for_log "$OUT_DIR" "$logname"); then
        return 0
    fi
    if [ -n "$LOG_OUT" ]; then
        fail "message logger never finished writing ${LOG_OUT##*/} (no 'End Triggerblock')"
    else
        fail "message logger produced no file for $logname"
    fi
    return 1
}

wait_for_log() {  # wait_for_log <dir> <prefix> [timeout-seconds]
    local dir="$1" prefix="$2" timeout="${3:-20}" tries=0 max f
    max=$((timeout * 4))
    while [ "$tries" -lt "$max" ]; do
        f="$(newest_log "$dir" "$prefix")"
        if [ -n "$f" ] && grep -q "End Triggerblock" "$f" 2>/dev/null; then
            printf '%s' "$f"
            return 0
        fi
        sleep 0.25
        tries=$((tries + 1))
    done
    f="$(newest_log "$dir" "$prefix")"
    [ -n "$f" ] && printf '%s' "$f"
    return 1
}

# ---- offline decode ----------------------------------------------------------
#
# Decode a log offline with the extension under test: opens a `ck analysis` session on <log>, attaches $EXT_NAME with <config>, and writes `frames-between --json` to <frames-out>.
#
# Prints a failure and returns 1 if a step fails; "report" also passes each step.
analysis_frames() (  # analysis_frames <log> <config> <frames-out> [report]
    log="$1" config="$2" frames="$3" report="${4:-}" session=""
    trap '[ -n "$session" ] && ck_ analysis close "$session" > /dev/null 2>&1' EXIT

    session=$(ck_capture analysis open "$(winpath "$log")" --json \
        | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).sessionId)}catch{process.exit(1)}})')
    if [ -z "$session" ]; then
        fail "ck analysis open did not return a session id"
        exit 1
    fi
    [ -n "$report" ] && pass "opened analysis session $session"

    out=$(ck_capture analysis set-processing "$session" --step "ext:name=$EXT_NAME;config=$(cat "$config")" --json)
    if printf '%s' "$out" | grep -q '"error"'; then
        fail "set-processing reported an error:"
        detail_block "$out"
        exit 1
    fi
    # Note: set-processing always reports "frameDefinitions": [] even on success.
    [ -n "$report" ] && pass "attached extension '$EXT_NAME' with generated config"

    ck_ analysis frames-between "$session" --start-s 0 --end-s 3600 --max-count 10000 --json > "$frames" 2>&1
    if ! node -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$frames" 2>/dev/null; then
        fail "frames-between did not return JSON:"
        detail_block "$(head -5 "$frames")"
        exit 1
    fi
    exit 0
)

# ---- service event log -------------------------------------------------------
#
# `ck get event-log` writes two files - a service event log (.txt) and the full service log (.log)
# into the working directory of the process that launched ck. Echoes the destination path of the .log file.
capture_event_log() {  # capture_event_log <dest-dir> <prefix>
    local dest="$1" prefix="$2" out line src moved=""
    mkdir -p "$dest"
    out=$(ck_capture get event-log) || return 1
    while IFS= read -r line; do
        case "$line" in
            *"Extracted"*" to "*)
                src="${line#*Extracted*entries to }"
                src="$(printf '%s' "$src" | tr -d '\r')"
                [ -f "$src" ] || src="$(cygpath -u "$src" 2>/dev/null || printf '%s' "$src")"
                [ -f "$src" ] || continue
                case "$src" in
                    *.log) mv -f "$src" "$dest/$prefix.log" && moved="$dest/$prefix.log" ;;
                    *)     mv -f "$src" "$dest/$prefix.event-log.txt" ;;
                esac
                ;;
        esac
    done <<< "$out"
    [ -n "$moved" ] && printf '%s' "$moved"
}

# ---- reporting ---------------------------------------------------------------
#
# Every status line is a status word padded to STATUS_COL, followed by its message.
#
#   ok      opened analysis session ...
#   FAILED  frames-between did not return JSON
#
# detail() nests a supporting line (a frame dump, a decoded-value list) under the message column, with no status word of its own, so it reads as "belongs to the status line above".
# Call it *after* the pass/fail/skip it supports, so the verdict always leads. detail_block() does the same for a whole (possibly multi-line) string.
#
# Scripts exit with SKIP_EXIT when a prerequisite is missing, and run-all.sh reports that as 'skipped'.
SKIP_EXIT=77
STATUS_COL=8

pass()   { printf '  %-*s%s\n' "$STATUS_COL" "ok"      "$*"; }
fail()   { printf '  %-*s%s\n' "$STATUS_COL" "FAILED"  "$*"; FAILURES=$((${FAILURES:-0} + 1)); }
skip()   { printf '  %-*s%s\n' "$STATUS_COL" "skipped" "$*"; }
detail() { printf '  %*s%s\n' "$STATUS_COL" "" "$*"; }

detail_block() {  # detail_block <text>   (may be multi-line; no-op on empty text)
    [ -n "$1" ] || return 0
    local line
    while IFS= read -r line; do
        detail "$line"
    done <<< "$1"
}
