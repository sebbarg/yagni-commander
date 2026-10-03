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

for tool in Xvfb xdotool xprop import xclip; do
    command -v "$tool" >/dev/null || { echo "smoke: $tool is not installed" >&2; exit 2; }
done

"$cargo" build --manifest-path "$root/Cargo.toml" -q
app=$root/target/debug/yagni-commander

mkdir -p "$shots" "$work/xdg" "$work/config/yagni-commander" "$work/state" "$work/data"
chmod 700 "$work/xdg"
export DISPLAY=$display XDG_RUNTIME_DIR=$work/xdg XDG_CONFIG_HOME=$work/config \
    XDG_STATE_HOME=$work/state XDG_DATA_HOME=$work/data
# A stand-in for xdg-open (Enter on a file): records its argument, opens
# nothing.
mkdir -p "$work/bin"
printf '#!/bin/sh\necho "$1" >>"%s/opened"\n' "$work" >"$work/bin/xdg-open"
chmod +x "$work/bin/xdg-open"
export PATH="$work/bin:$PATH"
# `true` stands in for an editor: it starts and exits at once.
printf '# smoke config\neditor = "true"\nlog = true\n' >"$work/config/yagni-commander/config.toml"
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
printf move >"$left/movable.txt"
printf '#!/bin/sh\necho ran >"%s/ran"\n' "$work" >"$left/run.sh"
chmod +x "$left/run.sh"
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
# The app id: WM_CLASS on X11 (the Wayland app_id is the same string).
app_id() { xprop -id "$1" WM_CLASS | grep -q '= "yagni-commander", "yagni-commander"$'; }

for _ in $(seq 50); do
    window=$(xdotool search --name yagni-commander 2>/dev/null | head -1) && break
    sleep 0.2
done
[[ -n ${window:-} ]] || { echo "smoke: the app did not start; see $work/app.log" >&2; exit 1; }
xdotool windowfocus "$window"
sleep 1

echo "smoke: $work"
shot 01-start
check "main window app id" app_id "$window"

echo "quick search"
typed no
shot 02-quick-search
keys Escape

echo "Enter on a file hands it to xdg-open"
typed notes
keys Escape Return
check "xdg-open got notes.txt" has "$work/opened" "$left/notes.txt"
echo "Enter on an executable script runs it"
typed run
keys Escape Return
check "run.sh ran" has "$work/ran" ran
check "run.sh not handed to xdg-open" has "$work/opened" "$left/notes.txt"

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

echo "F5 one file under a new name (the name is preselected)"
typed notes
keys F5
typed notes2
shot 03b-copy-rename
keys Return
check "copied as right/notes2.txt" has "$right/notes2.txt" hello

echo "F6 one file to a full path with new folders"
typed movable
keys F6 ctrl+a
typed "$right/new/sub/moved.txt"
keys Return
check "moved into new folders" has "$right/new/sub/moved.txt" move
check "movable.txt gone from the left" test ! -e "$left/movable.txt"

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

echo "Alt-F5 pack a folder with a link (stored), Alt-F6 extract it"
mkdir -p "$left/bundle"
printf hello >"$left/bundle/hi.txt"
ln -s hi.txt "$left/bundle/link"
keys ctrl+r
typed bundle
keys Escape alt+F5
shot 08a-pack-prompt
keys Return            # into the right panel as bundle.zip
sleep 1
shot 08b-link-prompt
keys Right Return      # Store as link
check "bundle.zip written" test -f "$right/bundle.zip"
check "zip lists the files" python3 -c 'import sys, zipfile; n = sorted(zipfile.ZipFile(sys.argv[1]).namelist()); sys.exit(0 if n == ["bundle/", "bundle/hi.txt", "bundle/link"] else 1)' "$right/bundle.zip"
keys Tab               # the right panel
typed bundle.z
keys Escape alt+F6
keys ctrl+a
typed "$work/unpacked"
keys Return
check "extracted the file" has "$work/unpacked/bundle/hi.txt" hello
check "extracted the link" test -L "$work/unpacked/bundle/link"

echo "Alt-F9 extracts a .tar.gz made by tar"
tar -C "$left" -czf "$right/docs.tar.gz" docs
keys ctrl+r
typed docs.tar
keys Escape alt+F9
keys ctrl+a
typed "$work/untarred"
keys Return
check "tar.gz extracted" test -d "$work/untarred/docs"
keys Tab               # back to the left panel

echo "Alt-F6 asks for a zip's password"
if command -v zip >/dev/null; then
    mkdir -p "$work/locked/secret"
    printf hidden >"$work/locked/secret/s.txt"
    (cd "$work/locked" && zip -q -r -P pw "$left/secret.zip" secret)
    keys ctrl+r
    typed secret.z
    keys Escape alt+F6 ctrl+a
    typed "$work/unlocked"
    keys Return
    sleep 1
    shot 08c-password-prompt
    typed pw
    keys Return
    check "password zip extracted" has "$work/unlocked/secret/s.txt" hidden
else
    echo "  skip  no zip command: the password prompt is not checked"
fi

echo "Ctrl-C and Ctrl-Ins copy the full path under the cursor"
clipboard() { [[ $(xclip -o -selection clipboard) == "$1" ]]; }
typed notes
keys Escape ctrl+c
check "Ctrl-C copied the path" clipboard "$left/notes.txt"
typed docs
keys Escape ctrl+Insert
check "Ctrl-Ins copied the path" clipboard "$left/docs"
echo "Ctrl-C in a text field copies its text"
keys F7
typed typed-text
keys ctrl+a ctrl+c
check "the field's text copied" clipboard typed-text
keys Escape
echo "Ctrl-C in an error box copies its text"
keys F7
typed docs
keys Return            # "docs" exists: an error box
keys ctrl+c
check "the error copied" sh -c 'xclip -o -selection clipboard | head -1 | grep -qx "Cannot create directory"'
shot 01b-error-box
keys Return Escape     # dismiss, then cancel the prompt

echo "menu: Files > Compare by content on two selected folders"
mkdir -p "$left/cmp1" "$left/cmp2"
printf a >"$left/cmp1/x"
printf b >"$left/cmp2/x"
printf o >"$left/cmp1/only"
keys ctrl+r
typed cmp1
keys Escape space space
# Compare by content is the 12th Files item.
keys F10 Down Down Down Down Down Down Down Down Down Down Down Down
shot 08d-menu-compare
keys Return
sleep 1
shot 08e-compare-result
keys ctrl+c
check "compare listed the differences" clipboard "$(printf 'Compare\nThe folders differ:\nonly in cmp1: only\ndifferent: x')"
keys Return
typed cmp1
keys Escape space space   # deselect

echo "Ctrl-. hidden files"
keys ctrl+period
shot 07-hidden-shown
keys ctrl+period

echo "menu: a lone Alt opens it, Escape closes it"
keys alt
shot 07a-menu-files
keys Escape

echo "menu: Alt-F opens Files, Escape closes it"
keys alt+f
shot 07a2-menu-alt-f
keys Escape

echo "menu: F10, Show > Hidden files"
keys F10 Right Right
shot 07b-menu-show
keys Down Return
shot 07c-hidden-from-menu
keys ctrl+period

echo "menu: Help > About"
keys F10 Left Down Return
shot 07d-about
keys Return

echo "settings: Ctrl-, opens it; Space toggles the sort switch; Escape closes"
cfg=$work/config/yagni-commander/config.toml
keys ctrl+comma
shot 07e-settings
keys Tab space
shot 07f-settings-toggled
keys Escape
check "setting saved" grep -q "^case_sensitive_sort = true" "$cfg"
check "config comment kept" grep -q "^# smoke config" "$cfg"
check "other keys kept" grep -q '^editor = "true"' "$cfg"

echo "Ctrl-D hotlist: add the current folder, leave, come back by its letter"
keys ctrl+d
shot 07g-hotlist-popup
keys Return            # empty hotlist: the first row is "Add current folder"
keys ctrl+a
typed "&Left"
keys Return
check "hotlist entry saved" grep -q '^name = "&Left"' "$cfg"
check "hotlist path saved" grep -q "^path = \"$left\"" "$cfg"
check "config comment kept after the hotlist save" grep -q "^# smoke config" "$cfg"
typed docs
keys Escape Return     # into docs
title_is() { [[ $(xdotool getwindowname "$window") == "$1 - yagni-commander" ]]; }
check "in docs" title_is "$left/docs"
keys ctrl+d
shot 07h-hotlist-entry
keys l
check "back in left by the hotlist letter" title_is "$left"
shot 07i-hotlist-back
keys ctrl+d Up Return  # Configure...
shot 07j-hotlist-configure
keys Escape
keys ctrl+comma Tab space Escape
check "setting saved back" grep -q "^case_sensitive_sort = false" "$cfg"

echo "settings: the Owner column switch hides it (and it stays hidden after the restart)"
keys ctrl+comma Tab Tab Tab Tab Tab Tab space
shot 07g-owner-hidden
keys Escape
check "show_owner saved" grep -q "^show_owner = false" "$cfg"

echo "settings: the Icons switch turns icons off and on"
keys ctrl+comma Tab Tab Tab Tab space Escape
check "icons = false saved" grep -q "^icons = false" "$cfg"
shot 07h-icons-off
keys ctrl+comma Tab Tab Tab Tab space Escape
check "icons = true saved" grep -q "^icons = true" "$cfg"

echo "a big folder loads in the background"
typed many
keys Return
shot 08a-loading
check "big folder opened" xdotool search --name "^$left/many - yagni-commander\$"
keys BackSpace
check "back in the left folder" xdotool search --name "^$left - yagni-commander\$"

echo "the watcher lists a file created outside the app"
printf watched >"$left/watched.txt"
sleep 1.5
typed watched
keys F2
typed seen
keys Return
check "the new file was listed (renamed through the panel)" test -f "$left/seen.txt"
check "watched.txt is gone" test ! -e "$left/watched.txt"
shot 08b-watched

echo "the watcher leaves a folder deleted outside the app"
mkdir -p "$left/vanish/inner"
sleep 1.5
typed vanish
keys Return
check "in the folder" xdotool search --name "^$left/vanish - yagni-commander\$"
rm -rf "$left/vanish"
check "back in the parent" xdotool search --name "^$left - yagni-commander\$"
shot 08c-vanished

echo "tabs: Ctrl-T, Ctrl-Tab, Ctrl-Shift-Tab, Ctrl-W"
keys ctrl+t
typed docs
keys Return
check "second tab in docs" xdotool search --name "^$left/docs - yagni-commander\$"
keys ctrl+shift+Tab
check "first tab on the left folder" xdotool search --name "^$left - yagni-commander\$"
printf bg >"$left/docs/background.txt"
keys ctrl+Tab
check "back in docs" xdotool search --name "^$left/docs - yagni-commander\$"
typed background
keys F2
typed seen-in-tab
keys Return
check "a background tab caught up when shown" test -f "$left/docs/seen-in-tab.txt"
keys ctrl+t
shot 08d-tabs
keys ctrl+w
check "closed the third tab, docs in front" xdotool search --name "^$left/docs - yagni-commander\$"
keys ctrl+shift+Tab
check "first tab again" xdotool search --name "^$left - yagni-commander\$"

echo "F3 viewer"
typed big
keys F3
check "viewer window open" xdotool search --name "big.txt - yagni-commander"
check "viewer window app id" app_id "$(xdotool search --name "big.txt - yagni-commander" | head -1)"
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

echo "Enter opens a zip; F3 views an entry; F5 copies entries out"
mkdir -p "$work/browse/docs"
printf one >"$work/browse/docs/readme.txt"
printf two >"$work/browse/docs/guide.txt"
(cd "$work/browse" && python3 -m zipfile -c "$left/browse.zip" docs)
keys ctrl+r
typed browse.z
keys Escape Return
check "inside the zip" title_is "$left/browse.zip"
typed docs
keys Escape Return
check "inside docs in the zip" title_is "$left/browse.zip/docs"
shot 08f-archive-inside
typed readme
keys Escape F3
check "viewer on the zip entry" xdotool search --name "^readme.txt - yagni-commander\$"
xdotool windowfocus "$(xdotool search --name "^readme.txt - yagni-commander\$" | head -1)"
sleep 0.5
shot 08g-archive-viewer
keys Escape
check "entry viewer closed" bash -c '! xdotool search --name "^readme.txt - yagni-commander\$"'
check "its private copy deleted" bash -c '[[ -z $(find "$1" -mindepth 2) ]]' _ "$work/state/yagni-commander/viewer-tmp"
xdotool windowfocus "$window"
sleep 0.5
keys ctrl+a F5
sleep 0.5
keys Return
check "F5 copied readme.txt out" has "$right/readme.txt" one
check "F5 copied guide.txt out" has "$right/guide.txt" two
check "the zip is untouched" python3 -c 'import sys, zipfile; sys.exit(0 if len(zipfile.ZipFile(sys.argv[1]).namelist()) == 3 else 1)' "$left/browse.zip"
keys BackSpace BackSpace
check "out of the zip" title_is "$left"
tar -C "$work/browse" -czf "$left/browse2.tar.gz" docs
keys ctrl+r
typed browse2
keys Escape Return
check "inside the tar.gz" title_is "$left/browse2.tar.gz"
keys BackSpace
check "out of the tar.gz" title_is "$left"

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
check "tabs in the state" grep -qF "left_tabs = [\"$left\", \"$left/docs\"]" "$work/state/yagni-commander/state.toml"
check "right tab in the state" grep -qF "right_tabs = [\"$right\"]" "$work/state/yagni-commander/state.toml"

echo "restart without arguments: same folders, right panel active"
"$app" >"$work/app2.log" 2>&1 &
pids+=($!)
check "reopened on the right folder" xdotool search --name "^$right - yagni-commander\$"
shot 12-restart
xdotool windowfocus "$(xdotool search --name "^$right - yagni-commander\$" | head -1)"
sleep 0.5
keys Tab ctrl+Tab
check "the restored second tab reads docs" xdotool search --name "^$left/docs - yagni-commander\$"
shot 12b-restart-tabs
keys alt+F4
check "restarted app quit" bash -c '! xdotool search --name "yagni-commander"'

echo "operation log"
check "old log deleted at startup" test ! -e "$logs/operations-2020-01-01.log"
today_log=$logs/operations-$(date +%F).log
for line in "mkdir created directory $left/made" "new file created $left/new.txt" \
    "rename renamed $left/new.txt -> $left/renamed.txt" \
    "copy replacing $right/notes.txt" "move moved $left/made -> $right/made" \
    "trash trashed $left/renamed.txt" "delete deleted $left/doomed.txt" \
    "move created directory $right/new/sub"; do
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
