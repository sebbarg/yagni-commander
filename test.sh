#!/usr/bin/env bash
# Runs every test of the workspace. Other arguments go to cargo test. The
# remote-folder tests need OpenSSH's sftp-server (see the README).
#
#   ./test.sh [cargo test arguments]
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
exec "$cargo" test --manifest-path "$root/Cargo.toml" --workspace "$@"
