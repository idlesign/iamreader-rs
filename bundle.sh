#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

command -v zip > /dev/null || { echo "zip is required" >&2; exit 1; }

VERSION="$(awk -F '"' '
    /^\[package\]/ { package = 1; next }
    /^\[/ { package = 0 }
    package && /^[[:space:]]*version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"
if [[ -z "$VERSION" ]]; then
    echo "Cannot read package version from Cargo.toml" >&2
    exit 1
fi

cargo build --release --bin iamreader --target-dir "$SCRIPT_DIR/target"

ARCHIVE="$SCRIPT_DIR/iamreader_v${VERSION}.zip"
STAGING_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING_DIR"' EXIT

zip -j "$STAGING_DIR/bundle.zip" target/release/iamreader LICENSE README.md get_models.sh
mv -f "$STAGING_DIR/bundle.zip" "$ARCHIVE"
echo "Created $ARCHIVE"
