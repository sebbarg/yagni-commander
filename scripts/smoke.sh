#!/usr/bin/env bash
# Smoke test of the real app on a virtual X display (Linux only).
#
# Starts the debug build under Xvfb, drives it with xdotool through the main
# features, checks the results on disk and saves screenshots for a visual
# review. Config, state and trash live in a temporary folder, never in the
# user's real ones (F8 uses the real trash code, into that folder's trash).
#
# usage: scripts/smoke.sh [screenshot-dir]   (default: target/smoke)
#   Needs: Xvfb, xdotool, ImageMagick (import), a Vulkan driver (Mesa llvmpipe
#   is fine). Builds with cargo first. Exits non-zero if a check fails.
#   SMOKE_DELAY (seconds between keys, default 0.4) and SMOKE_DISPLAY
#   (default :99) can be set in the environment.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d /tmp/yagni-smoke.XXXXXX)
shots=${1:-$root/target/smoke}
delay=${SMOKE_DELAY:-0.4}
display=${SMOKE_DISPLAY:-:99}
cargo=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}

for tool in Xvfb xdotool import; do
    command -v "$tool" >/dev/null || { echo "smoke: $tool is not installed" >&2; exit 2; }
done

"$cargo" build --manifest-path "$root/Cargo.toml" -q
app=$root/target/debug/yagni-commander

mkdir -p "$shots" "$work/xdg" "$work/config/yagni-commander" "$work/state" "$work/data"
chmod 700 "$work/xdg"
export DISPLAY=$display XDG_RUNTIME_DIR=$work/xdg XDG_CONFIG_HOME=$work/config \
    XDG_STATE_HOME=$work/state XDG_DATA_HOME=$work/data
# `true` stands in for an editor: it starts and exits at once.
printf 'editor = "true"\nlog = true\n' >"$work/config/yagni-commander/config.toml"
# An old log file that startup must delete.
logs=$work/state/yagni-commander/logs
mkdir -p "$logs"
: >"$logs/operations-2020-01-01.log"

left=$work/left
right=$work/right
mkdir -p "$left/docs" "$right"
printf hello >"$left/notes.txt"
printf secret >"$left/.hidden"
printf old >"$right/notes.txt"
seq 1 200000 | sed 's/^/line /' >"$left/big.txt"
head -c 300000 /dev/zero | tr '\0' 'x' >"$left/oneline.txt"
mkdir -p "$left/many"
(cd "$left/many" && seq 1 150000 | xargs touch)

pids=()
cleanup() {
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
}
# The temporary folder is kept only when a check failed, for its app.log.
trap cleanup EXIT

Xvfb "$display" -screen 0 1280x860x24 >/dev/null 2>&1 &
pids+=($!)
sleep 1
"$app" "$left" "$right" >"$work/app.log" 2>&1 &
pids+=($!)

failures=0
pass() { echo "  ok    $1"; }
fail() {
    echo "  FAIL  $1" >&2
    failures=$((failures + 1))
}
# check "description" command...: passes once the command succeeds (5 s).
check() {
    local what=$1
    shift
    for _ in $(seq 50); do
        if "$@" 2>/dev/null; then
            pass "$what"
            return
        fi
        sleep 0.1
    done
    fail "$what"
}
keys() {
    for key in "$@"; do
        xdotool key "$key"
        sleep "$delay"
    done
}
typed() {
    xdotool type --delay 40 "$1"
    sleep "$delay"
}
shot() { import -window root "$shots/$1.png"; }
has() { [[ $(cat "$1") == "$2" ]]; }

for _ in $(seq 50); do
    window=$(xdotool search --name yagni-commander 2>/dev/null | head -1) && break
    sleep 0.2
done
[[ -n ${window:-} ]] || { echo "smoke: the app did not start; see $work/app.log" >&2; exit 1; }
xdotool windowfocus "$window"
sleep 1

echo "smoke: $work"
shot 01-start

echo "quick search"
typed no
shot 02-quick-search
keys Escape

echo "F7 new directory"
keys F7
typed made
keys Return
check "made/ created" test -d "$left/made"

echo "Shift-F4 new file"
keys shift+F4
typed new.txt
keys Return
check "new.txt created" test -f "$left/new.txt"

echo "F2 rename (the name is preselected up to the extension)"
keys F2
typed renamed
keys Return
check "new.txt renamed to renamed.txt" test -f "$left/renamed.txt"

echo "F5 copy onto an existing file: Enter overwrites (preselected)"
typed notes
keys F5 Return
sleep 1
shot 03-conflict
keys Return
check "right/notes.txt overwritten" has "$right/notes.txt" hello
check "left/notes.txt kept" has "$left/notes.txt" hello

echo "F6 move"
typed made
keys F6 Return
check "made/ moved to the right" test -d "$right/made"
check "made/ gone from the left" test ! -e "$left/made"

echo "F8 trash (into the temporary trash)"
typed renamed
keys F8
shot 04-trash-confirm
keys Return
check "renamed.txt gone" test ! -e "$left/renamed.txt"
check "renamed.txt in the trash" test -f "$work/data/Trash/files/renamed.txt"

echo "Shift-Del delete permanently"
printf bye >"$left/doomed.txt"
keys ctrl+r
typed doomed
keys shift+Delete
shot 05-delete-confirm
keys Return
check "doomed.txt deleted" test ! -e "$left/doomed.txt"
check "doomed.txt not in the trash" test ! -e "$work/data/Trash/files/doomed.txt"

echo "F12 button test dialog"
keys F12
shot 06-buttons
keys Escape

echo "Ctrl-. hidden files"
keys ctrl+period
shot 07-hidden-shown
keys ctrl+period

echo "a big folder loads in the background"
typed many
keys Return
shot 08a-loading
check "big folder opened" xdotool search --name "^$left/many - yagni-commander\$"
keys BackSpace
check "back in the left folder" xdotool search --name "^$left - yagni-commander\$"

echo "F3 viewer"
typed big
keys F3
check "viewer window open" xdotool search --name "big.txt - yagni-commander"
# No window manager here to focus the new window.
xdotool windowfocus "$(xdotool search --name "big.txt - yagni-commander" | head -1)"
sleep 0.5
shot 08-viewer
keys ctrl+End
shot 09-viewer-end
keys w Right Right
shot 10-viewer-nowrap
keys Escape
check "viewer closed" bash -c '! xdotool search --name "big.txt - yagni-commander"'
xdotool windowfocus "$window"
sleep 0.5

echo "F3 viewer on a giant single line"
typed oneline
keys F3
check "viewer window open" xdotool search --name "oneline.txt - yagni-commander"
# No window manager here to focus the new window.
xdotool windowfocus "$(xdotool search --name "oneline.txt - yagni-commander" | head -1)"
sleep 0.5
keys ctrl+End
shot 11-viewer-oneline-end
keys q
check "viewer closed" bash -c '! xdotool search --name "oneline.txt - yagni-commander"'
xdotool windowfocus "$window"
sleep 0.5

echo "quit (with the right panel active)"
keys Tab
keys alt+F4
check "state file written" test -f "$work/state/yagni-commander/state.toml"
check "hidden files hidden again in the state" grep -q "show_hidden = false" \
    "$work/state/yagni-commander/state.toml"
check "viewer geometry in the state" grep -q "^\[viewer\]" "$work/state/yagni-commander/state.toml"
check "panel folders in the state" grep -qF "right = \"$right\"" "$work/state/yagni-commander/state.toml"

echo "restart without arguments: same folders, right panel active"
"$app" >"$work/app2.log" 2>&1 &
pids+=($!)
check "reopened on the right folder" xdotool search --name "^$right - yagni-commander\$"
shot 12-restart
xdotool windowfocus "$(xdotool search --name "^$right - yagni-commander\$" | head -1)"
sleep 0.5
keys alt+F4
check "restarted app quit" bash -c '! xdotool search --name "yagni-commander"'

echo "operation log"
check "old log deleted at startup" test ! -e "$logs/operations-2020-01-01.log"
today_log=$logs/operations-$(date +%F).log
for line in "mkdir created directory $left/made" "new file created $left/new.txt" \
    "rename renamed $left/new.txt -> $left/renamed.txt" \
    "copy replacing $right/notes.txt" "move moved $left/made -> $right/made" \
    "trash trashed $left/renamed.txt" "delete deleted $left/doomed.txt"; do
    check "logged: $line" grep -qF "$line" "$today_log"
done

echo "screenshots: $shots"
if ((failures > 0)); then
    echo "smoke: $failures check(s) failed; app log: $work/app.log" >&2
    exit 1
fi
cleanup
rm -rf "$work"
echo "smoke: all checks passed"
