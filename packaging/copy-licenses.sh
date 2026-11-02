#!/usr/bin/env bash
# Copies the license files a binary release must carry into $1: ours, the
# third-party notices and the bundled fonts' licenses.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
fonts=$root/crates/yagni-commander/assets/fonts
mkdir -p "$1/licenses"
cp "$root/LICENSE" "$root/THIRD-PARTY-NOTICES.md" "$1/"
cp "$fonts/LICENSE" "$1/licenses/SymbolsNerdFont-LICENSE"
cp "$fonts/JetBrainsMono-OFL.txt" "$1/licenses/"
