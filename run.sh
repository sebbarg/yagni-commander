#!/usr/bin/env bash
# Builds and runs yagni-commander: a release build, or a debug build with
# --debug. Other arguments go to the app (the left and right folders).
#
#   ./run.sh [--debug] [left-folder] [right-folder]
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
profile=--release
[[ ${1:-} == --debug ]] && { profile=; shift; }
exec "$cargo" run ${profile:+"$profile"} --manifest-path "$root/Cargo.toml" -p yagni-commander -- "$@"
