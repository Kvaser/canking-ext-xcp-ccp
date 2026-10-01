#!/usr/bin/env bash
#
# Build + test runner for the DAQ Configurator repo.
# Runs all requested steps even if one fails, then prints a summary.
#
# Usage:
#   ./run.sh [group ...]
#
# Groups:
#   rust     cargo tests (workspace defaults, protocol-core+serde, a2l-daq serde without tscript)
#            + a2l-daq-cli release build
#   wasm     wasm32-wasip2 component builds (a2l-daq, daq-decoder)
#   gui      canking-gui: npm install, jco transpile, typecheck + vite build, npm pack (.tgz)
#   stage    generate license.txt + THIRD_PARTY_LICENSES.html (cargo-about), then stage the raw
#            .tgz/.wasm, license/doc files and ck-ext-info.json into canking-gui/dist and canking-service/dist,
#            ready to be signed. Needs `gui` and `wasm` (daq-decoder) run first.
#   pack     package the staged bundles into .ckext files. Needs `stage` run first;
#            picks up dist/bundle.p7s if a signing step dropped one in there.
#   ck-cli   end-to-end tests against a running CanKing service via the ck CLI (see tests/ck-cli/README.md).
#            Needs the wasm transpiled first. Individual tests skip when their prerequisites are absent.
#   all      every build task above (default)

set -u
cd "$(dirname "$0")"

PASS=()
FAIL=()

# step <name> <command...>. Run in a subshell, record result, keep going
step() {
    local name="$1"; shift
    echo
    echo "=== ${name} ==="
    local start=$SECONDS
    if ("$@"); then
        PASS+=("$(printf '%-50s %4ds' "$name" $((SECONDS - start)))")
    else
        FAIL+=("$(printf '%-50s %4ds' "$name" $((SECONDS - start)))")
    fi
}

gui_install() {
    cd canking-gui
    npm ci
}

gui_transpile() { cd canking-gui && npm run build:wasm; }

gui_build() { cd canking-gui && npm run build; }

gui_package() { cd canking-gui && npm run package; }

# write_license <file> <product name>: the project LICENSE, headed by the product name like other CanKing extensions.
write_license() {
    { printf 'Software Product: %s\n\n\n' "$2"; cat LICENSE; } > "$1"
}

# third_party_licenses <dir> <cargo-about args...>: <dir>/THIRD_PARTY_LICENSES.html for the crates in its wasm.
third_party_licenses() {
    local dir="$1"; shift
    cargo about generate --fail -c about.toml -o "$dir/THIRD_PARTY_LICENSES.html" "$@" about.hbs
}

# The GUI's npm dependencies are listed in dist/THIRD_PARTY_LICENSES.md inside its .tgz (vite build.license).
# The CJK fonts are only referenced from CSS url(), which vite does not track, so their licenses are copied here.
gui_license() {
    write_license canking-gui/license.txt "DAQ Configurator"
    third_party_licenses canking-gui -m a2l-daq/Cargo.toml --no-default-features --features component || exit 1

    local pkg
    for pkg in @fontsource/noto-sans-jp @fontsource/noto-sans-sc; do
        printf '%s %s (%s)\n\n' "$pkg" \
            "$(node -p "require('./canking-gui/node_modules/$pkg/package.json').version")" \
            "$(node -p "require('./canking-gui/node_modules/$pkg/package.json').license")"
        cat "canking-gui/node_modules/$pkg/LICENSE" || exit 1
        printf '\n\n'
    done > canking-gui/THIRD_PARTY_LICENSES_FONTS.txt
}

service_license() {
    write_license canking-service/license.txt "DAQ Decoder"
    third_party_licenses canking-service -m canking-service/Cargo.toml
}

# Read name/version/description/author + the canking.* fields out of package.json and write dist/ck-ext-info.json
gui_extract_info() {
    cd canking-gui
    mkdir -p dist
    info_file="dist/ck-ext-info.json"
    rm -f "$info_file"

    name=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).name")
    version=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).version")
    description=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).description")
    author=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).author")
    workspaceViewName=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).canking.workspaceViewName")
    extensionType=$(node -p "JSON.parse(require('fs').readFileSync('package.json','utf8')).canking.extensionType")
    cat > "$info_file" <<EOL
{
    "createdDate": "$(node -p "new Date().toISOString()")",
    "name": "$name",
    "displayName": "$workspaceViewName",
    "description": "$description",
    "version": "$version",
    "author": "$author",
    "extensionType": "$extensionType",
    "data": ""
}
EOL
}

# Stage one built artifact (found at <src>, relative to <dir>) plus whichever of the given license/doc files exist, into <dir>/dist/, ready to be signed.
#
#   stage_bundle <dir> <src> <name-in-dist> <missing-hint> [extra files...]
stage_bundle() {
    local dir="$1" src="$2" name="$3" hint="$4"; shift 4
    cd "$dir"
    mkdir -p dist
    if [ -z "$src" ] || [ ! -f "$src" ]; then
        echo "$hint" >&2
        exit 1
    fi
    cp "$src" "dist/$name"

    for f in "$@"; do
        [ -f "$f" ] && cp "$f" "dist/$f"
    done
    return 0
}

# Stage the .tgz produced by `npm run package`
gui_stage() {
    local tgz_file
    tgz_file=$(cd canking-gui && ls ./*.tgz 2>/dev/null | head -n 1)
    stage_bundle canking-gui "$tgz_file" "${tgz_file##*/}" \
        "No .tgz found in canking-gui/; run 'npm run package' (or ./run.sh gui) first" \
        THIRD_PARTY_LICENSES.html THIRD_PARTY_LICENSES_FONTS.txt license.txt ReleaseNotes.txt user-guide.md UserGuide.html
}

# Bundle a staged artifact  + ck-ext-info.json + ev. signature into a .ckext file under <dir>/dist/,
# then clean up the staged inputs so only the .ckext remains.
#
#   pack_ckext <dir> <dist-glob> <missing-hint> [extra files...]
pack_ckext() {
    local dir="$1" glob="$2" hint="$3"; shift 3
    cd "$dir"
    local info_file="dist/ck-ext-info.json"
    local sign_file="dist/bundle.p7s"

    local artifact_file
    artifact_file=$(ls $glob 2>/dev/null | head -n 1)
    if [ -z "$artifact_file" ]; then
        echo "$hint" >&2
        exit 1
    fi
    local artifact_name="${artifact_file##*/}"
    local ckext_file="dist/${artifact_name%.*}.ckext"
    rm -f "$ckext_file"

    local extras=""
    for f in "$@"; do
        [ -f "dist/$f" ] && extras="$extras $f"
    done

    echo "Creating $ckext_file"
    if [ -f "$sign_file" ]; then
        tar -czvf "$ckext_file" -C dist "$artifact_name" $extras "$(basename "$info_file")" "$(basename "$sign_file")"
    else
        tar -czvf "$ckext_file" -C dist "$artifact_name" $extras "$(basename "$info_file")"
    fi

    # Clean up staged inputs, keep only the .ckext in dist/
    rm -f "$info_file" "$sign_file" "dist/$artifact_name"
    for f in $extras; do rm -f "dist/$f"; done
}

# Bundle the staged .tgz + ck-ext-info.json into a .ckext file under dist/.
gui_pack() {
    pack_ckext canking-gui "dist/*.tgz" \
        "No staged .tgz in canking-gui/dist/; run './run.sh stage' first" \
        THIRD_PARTY_LICENSES.html THIRD_PARTY_LICENSES_FONTS.txt license.txt ReleaseNotes.txt user-guide.md UserGuide.html
}

# Read name/version/description from Cargo.toml, author from the workspace Cargo.toml and display_name
# from lib.rs, then write dist/ck-ext-info.json.
service_extract_info() {
    cd canking-service
    mkdir -p dist
    info_file="dist/ck-ext-info.json"
    rm -f "$info_file"

    name=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1 | tr '-' '_')
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
    description=$(sed -n 's/^description = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
    author=$(sed -n 's/^authors = \["\(.*\)"\]$/\1/p' ../Cargo.toml | head -n 1)

    display_name=$(sed -n 's/.*display_name: "\(.*\)"\.to_string(),.*/\1/p' src/lib.rs | head -n 1)

    default_config=$(cargo run -q -p protocol-core --features serde --example dump_default_config)
    data_escaped=$(printf '%s' "$default_config" \
        | sed ':a;N;$!ba;s/\\/\\\\/g;s/"/\\u0022/g;s/\n/\\n/g')

    cat > "$info_file" <<EOL
{
    "createdDate": "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)",
    "name": "$name",
    "displayName": "$display_name",
    "description": "$description",
    "version": "$version",
    "author": "$author",
    "extensionType": "DataProcessor",
    "data": "$data_escaped"
}
EOL
}

# Stage the built wasm component
service_stage() {
    local version wasm_src="../target/wasm32-wasip2/release/daq_decoder.wasm"
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' canking-service/Cargo.toml | head -n 1)
    stage_bundle canking-service "$wasm_src" "daq_decoder-${version}.wasm" \
        "No daq-decoder wasm found at $wasm_src; run './run.sh wasm' first" \
        THIRD_PARTY_LICENSES.html license.txt ReleaseNotes.txt user-guide.md UserGuide.html
}

# Bundle the staged wasm + ck-ext-info.json into a .ckext file under dist/.
service_pack() {
    pack_ckext canking-service "dist/*.wasm" \
        "No staged wasm in canking-service/dist/; run './run.sh stage' first" \
        THIRD_PARTY_LICENSES.html license.txt ReleaseNotes.txt user-guide.md UserGuide.html
}

rust_group() {
    step "cargo test (workspace, default features)" \
        cargo test --workspace
    step "cargo test protocol-core (serde)" \
        cargo test -p protocol-core --features serde
    step "cargo test a2l-daq (cli: serde, no default features)" \
        cargo test -p a2l-daq --no-default-features --features serde
    step "cargo build a2l-daq-cli (release)" \
        cargo build -p a2l-daq --release
}

wasm_group() {
    step "wasm build a2l-daq (component)" \
        cargo build -p a2l-daq --lib --release --target wasm32-wasip2 \
            --no-default-features --features component
    step "wasm build daq-decoder" \
        cargo build -p daq-decoder --release --target wasm32-wasip2
}

gui_group() {
    step "npm install (canking-gui)" gui_install
    step "jco transpile (a2l-daq wasm -> GUI)" gui_transpile
    step "GUI typecheck + vite build" gui_build
    step "GUI package (installable .tgz)" gui_package
}

stage_group() {
    step "license files (GUI)" gui_license
    step "license files (service)" service_license
    step "stage bundle for signing (GUI)" gui_stage
    step "extract extension info (GUI)" gui_extract_info
    step "stage bundle for signing (service)" service_stage
    step "extract extension info (service)" service_extract_info
}

pack_group() {
    step "package .ckext (GUI)" gui_pack
    step "package .ckext (service)" service_pack
}

ck_cli_group() {
    step "CanKing CLI tests" bash ./tests/ck-cli/run-all.sh
}

usage() {
    sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

[ $# -eq 0 ] && set -- all
for group in "$@"; do
    case "$group" in
        rust) rust_group ;;
        wasm) wasm_group ;;
        gui)  gui_group ;;
        stage) stage_group ;;
        pack) pack_group ;;
        ck-cli) ck_cli_group ;;
        all)  rust_group; wasm_group; gui_group; stage_group; pack_group ;;
        -h|--help) usage 0 ;;
        *) echo "Unknown group: $group"; usage 1 ;;
    esac
done

echo
echo "==================== summary ===================="
for s in ${PASS[@]+"${PASS[@]}"}; do echo "  ok      $s"; done
for s in ${FAIL[@]+"${FAIL[@]}"}; do echo "  FAILED  $s"; done
echo "================================================="

[ ${#FAIL[@]} -eq 0 ]
