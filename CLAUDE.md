# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

@ARCHITECTURE.md

## Where to look

- `SPEC.md`: what the app does and why (keys, dialogs, file operations, owner decisions). Read the relevant part before changing behavior.
- `ROADMAP.md`: v2 and v3, open decisions, and what waits for verification on real machines.
- `ARCHITECTURE.md` (imported above): goals, technical decisions, code map, known issues, gotchas, testing.
- `README.md`: features, building, configuration, for users.

## Current state

Not kept here (owner, 2026-11-22). What is on `main` but not released: `git log $(git describe --tags --abbrev=0)..main`. What waits for verification on real machines, and what comes next: `ROADMAP.md`.

## How we work

- We always work on `main`, also for commits, unless the owner says otherwise. No feature branches.
- The owner reviews each step; commit only when asked (they say "commit"). Commits are local; the owner pushes (remote: `github.com/sebbarg/yagni-commander`).
- Every change: tests (core near 100% coverage; gpui app tests for every key, mouse action and dialog), `cargo clippy --workspace --all-targets` with zero warnings, `cargo fmt`, then `scripts/smoke.sh` and a look at its screenshots. Extend the smoke script when a feature is visible or touches the OS (trash, editor, state file).
- Keep the docs current: `SPEC.md` when the owner decides behavior, `ROADMAP.md` when an item is done, added or verified, `ARCHITECTURE.md` when the code's structure or a technical decision changes. No history here: git log keeps it.
- User-facing text and docs: no em dashes.
- Dialog tab order (a recurring bug): every new or changed dialog is checked with Tab and Shift-Tab, in an app test (`mod tab_order`, `assert_order`) and in the headless app. The rule: reading order (top to bottom, left to right), buttons last, Tab into the buttons on the first and Shift-Tab on the last, disabled controls skipped, round at both ends, never out of the top dialog, every stop visibly focused (SPEC, "Tab order in every dialog"; ARCHITECTURE, Gotchas).

## Fresh dev container (Ubuntu 26.04, no display)

Containers are ephemeral; redo this at the start of a session if `~/.cargo/bin/cargo` is missing.

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
sudo apt-get install -y build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
  mesa-vulkan-drivers xvfb xdotool x11-utils imagemagick xclip openssh-sftp-server
~/.cargo/bin/rustup component add llvm-tools-preview && ~/.cargo/bin/cargo install cargo-llvm-cov --locked
```

Cargo is at `~/.cargo/bin/cargo` (not on PATH in non-login shells). A clean build takes a few minutes.

## Headless run (to see the real app in the container)

Xvfb gives a virtual X11 display, Mesa's llvmpipe renders Vulkan on the CPU, xdotool sends input, ImageMagick screenshots. Scratch `XDG_*` dirs keep config/state away from real files.

```sh
S=<scratchpad>; mkdir -p $S/xdg $S/cfg $S/state; chmod 700 $S/xdg
export DISPLAY=:99 XDG_RUNTIME_DIR=$S/xdg XDG_CONFIG_HOME=$S/cfg XDG_STATE_HOME=$S/state
(Xvfb :99 -noreset -screen 0 1280x860x24 >/dev/null 2>&1 &); sleep 1
setxkbmap -option srvrkeys:none        # else xdotool sends Alt with every F-key (Ubuntu 26.04)
(target/debug/yagni-commander /usr /etc >$S/app.log 2>&1 &); sleep 4
W=$(xdotool search --name yagni-commander | head -1); xdotool windowfocus $W; sleep 0.5
xdotool key Down F2; xdotool type --delay 40 "name"; xdotool key Return
import -window root $S/shot.png        # then read the PNG
xdotool key alt+F4; pkill -x Xvfb
```

- Pause (`sleep 0.3`+) after focusing, moving or resizing before sending keys; gpui also ignores a click sent in the same instant as the pointer move.
- Never `pkill -f` a pattern that matches your own shell command line; use `pkill -x yagni-commander`.
- `-noreset` keeps the `setxkbmap` option: Xvfb resets its keymap when its last client disconnects.
- X11 only, no window manager: Wayland, tiling (Hyprland) and macOS are untested here.
