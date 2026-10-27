#!/usr/bin/env bash
# Whether the newest release on crates.io of a gpui crate fixes one of the
# gpui problems we work around or live with (ARCHITECTURE). Run weekly by
# .github/workflows/upstream-watch.yml.
#
#   scripts/check-upstream.sh <check> [crate-file]
#
# Checks:
#   menu-keys     gpui-pre-macos `key_to_native` maps Enter and Tab, so the
#                 native macOS menu can show them (Gotchas, menu shortcut
#                 labels; actions::hide_native_menu_keys).
#   x11-position  gpui-pre-linux sets a position in the X11 window's
#                 WM_NORMAL_HINTS, so KWin restores and centers the window
#                 (Known issues).
#
# Prints the version checked. Exit 0: fixed; 1: not yet; 2: the check
# failed. A local .crate file skips the download (for trying the script).
set -euo pipefail

check=${1:-}
case $check in
menu-keys) name=gpui-pre-macos file='*/src/events.rs' ;;
x11-position) name=gpui-pre-linux file='*/src/linux/x11/window.rs' ;;
*)
    echo "usage: $0 menu-keys|x11-position [crate-file]" >&2
    exit 2
    ;;
esac

agent="yagni-commander upstream check (github.com/sebbarg/yagni-commander)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if [[ $# -gt 1 ]]; then
    crate=$2
    version=$(basename "$crate" .crate)
    version=${version#"$name"-}
else
    version=$(curl -fsS -A "$agent" "https://crates.io/api/v1/crates/$name" |
        python3 -c 'import json, sys; print(json.load(sys.stdin)["crate"]["max_stable_version"])') ||
        { echo "cannot ask crates.io for $name" >&2; exit 2; }
    crate=$work/$name.crate
    curl -fsSL -A "$agent" -o "$crate" \
        "https://static.crates.io/crates/$name/$name-$version.crate" ||
        { echo "cannot download $name $version" >&2; exit 2; }
fi

tar -xzf "$crate" -C "$work" --wildcards "$file" ||
    { echo "no ${file#*/} in $name $version" >&2; exit 2; }
# The glob matches the one file extracted.
# shellcheck disable=SC2206
files=("$work"/$file)
source_file=${files[0]}

case $check in
menu-keys)
    # The match arms of key_to_native, up to its catch-all.
    table=$(sed -n '/fn key_to_native/,/_ =>/p' "$source_file")
    [[ -n $table ]] || { echo "no key_to_native in $name $version" >&2; exit 2; }
    echo "$version"
    grep -q '"enter" =>' <<<"$table" && grep -q '"tab" =>' <<<"$table" && exit 0
    ;;
x11-position)
    grep -q 'WmSizeHints::new' "$source_file" ||
        { echo "no WmSizeHints in $name $version" >&2; exit 2; }
    echo "$version"
    grep -q 'size_hints\.position' "$source_file" && exit 0
    ;;
esac
exit 1
