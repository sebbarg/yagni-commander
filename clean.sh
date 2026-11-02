#!/usr/bin/env bash
# Removes the build output: all of target/, with the release files in
# target/dist/ and the smoke screenshots in target/smoke/. Other arguments
# go to cargo clean (--release, -p yagni-commander, ...).
#
#   ./clean.sh [cargo clean arguments]
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
exec "$cargo" clean --manifest-path "$root/Cargo.toml" "$@"
