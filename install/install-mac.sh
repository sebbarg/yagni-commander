#!/usr/bin/env bash
# Installs the bundle ./build.sh built into ~/Applications, so
# it starts from Finder, Launchpad and Spotlight with its icon. APP_DIR
# moves it.
#
#   install/install-mac.sh
set -euo pipefail

app=yagni-commander
root=$(cd "$(dirname "$0")/.." && pwd)
bundle=$root/target/release/$app.app
dest=${APP_DIR:-$HOME/Applications}

[[ $(uname) == Darwin ]] || { echo "$0: macOS only" >&2; exit 1; }
[[ -d $bundle ]] || { echo "$0: no $bundle; run ./build.sh first" >&2; exit 1; }

mkdir -p "$dest"
# Only ever our own bundle at this path; a running copy keeps working
# until it quits.
rm -rf "${dest:?}/$app.app"
cp -R "$bundle" "$dest/"
echo "installed $dest/$app.app"
