#!/usr/bin/env bash
# Builds yagni-commander: a release build, on macOS also bundled as
# target/release/yagni-commander.app (packaging/bundle-mac.sh), which
# install/install-<os>.sh installs; or a debug build with --debug. Other
# arguments go to cargo.
#
#   ./build.sh [--debug] [cargo build arguments]
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
profile=--release
[[ ${1:-} == --debug ]] && { profile=; shift; }
"$cargo" build ${profile:+"$profile"} --manifest-path "$root/Cargo.toml" -p yagni-commander "$@"
if [[ $profile && $(uname) == Darwin ]]; then
    "$root/packaging/bundle-mac.sh"
fi
