#!/usr/bin/env bash
#
# Retrieve the correct version from the gui and service extension.

set -eu
cd "$(dirname "$0")"

usage() {
    echo "Usage: $0 gui|service" >&2
    exit 1
}

[ $# -eq 1 ] || usage

case "$1" in
    gui)
        source="canking-gui/package.json"
        version=$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$source" | head -n 1)
        ;;
    service)
        source="canking-service/Cargo.toml"
        version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$source" | head -n 1)
        ;;
    *)
        usage
        ;;
esac

if [ -z "$version" ]; then
    echo "Could not read a version from $source" >&2
    exit 1
fi

printf '%s' "$version"