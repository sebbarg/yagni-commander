# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

@ARCHITECTURE.md

## Where to look

- `SPEC.md`: what the app does and why (keys, dialogs, file operations, owner decisions). Read the relevant part before changing behavior.
- `ROADMAP.md`: v2 and v3, open decisions, and what waits for verification on real machines.
- `ARCHITECTURE.md` (imported above): goals, technical decisions, code map, known issues, gotchas, testing.
- `docs/superpowers/specs/` and `docs/superpowers/plans/`: per-feature designs and plans with their reasoning (historical: they still say `Requirements.md`, now `SPEC.md`).
- `README.md`: features, building, configuration, for users.

## Current state (update at the end of every session)

As of 2026-10-04: v1 is complete, including the app icon (`packaging/icons/`, not wired into packaging), and zoom is merged into `main` (Ctrl-=/+/-/0 and keypad +/-, a UI level and a viewer level, modeled on Zed; spec `docs/superpowers/specs/2026-10-04-zoom-design.md`, plan `docs/superpowers/plans/2026-10-04-zoom.md`; known limits in SPEC (Zoom) and ROADMAP v2). Themes are done and merged on `main` (Tokyo Night, Gruvbox Dark, Everforest Dark, Catppuccin Latte, Classic; `theme` key and Settings dropdown; spec `docs/superpowers/specs/2026-10-04-themes-design.md`). `cargo test --workspace` (451 app, 652 core), clippy and fmt are green and `scripts/smoke.sh` passes end to end. Next: the owner's choice from `ROADMAP.md`.

## How we work

- The owner reviews each step; commit only when asked (they say "commit"). Commits are local; the owner pushes (remote: `github.com/sebbarg/yagni-commander`).
- Every change: tests (core near 100% coverage; gpui app tests for every key, mouse action and dialog), `cargo clippy --workspace --all-targets` with zero warnings, `cargo fmt`, then `scripts/smoke.sh` and a look at its screenshots. Extend the smoke script when a feature is visible or touches the OS (trash, editor, state file).
- Keep the docs current: `SPEC.md` when the owner decides behavior, `ROADMAP.md` when an item is done, added or verified, `ARCHITECTURE.md` when the code's structure or a technical decision changes, and Current state above at the end of a session. No history here: git log and the specs keep it.
- User-facing text and docs: no em dashes.

## Fresh dev container (Ubuntu 24.04, no display)

Containers are ephemeral; redo this at the start of a session if `~/.cargo/bin/cargo` is missing.

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
sudo apt-get install -y build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
  mesa-vulkan-drivers xvfb xdotool x11-utils imagemagick xclip
~/.cargo/bin/rustup component add llvm-tools-preview && ~/.cargo/bin/cargo install cargo-llvm-cov --locked
```

Cargo is at `~/.cargo/bin/cargo` (not on PATH in non-login shells). A clean build takes a few minutes.

## Headless run (to see the real app in the container)

Xvfb gives a virtual X11 display, Mesa's llvmpipe renders Vulkan on the CPU, xdotool sends input, ImageMagick screenshots. Scratch `XDG_*` dirs keep config/state away from real files.

```sh
S=<scratchpad>; mkdir -p $S/xdg $S/cfg $S/state; chmod 700 $S/xdg
export DISPLAY=:99 XDG_RUNTIME_DIR=$S/xdg XDG_CONFIG_HOME=$S/cfg XDG_STATE_HOME=$S/state
(Xvfb :99 -screen 0 1280x860x24 >/dev/null 2>&1 &); sleep 1
(target/debug/yagni-commander /usr /etc >$S/app.log 2>&1 &); sleep 4
W=$(xdotool search --name yagni-commander | head -1); xdotool windowfocus $W; sleep 0.5
xdotool key Down F2; xdotool type --delay 40 "name"; xdotool key Return
import -window root $S/shot.png        # then read the PNG
xdotool key alt+F4; pkill -x Xvfb
```

- Pause (`sleep 0.3`+) after focusing, moving or resizing before sending keys; gpui also ignores a click sent in the same instant as the pointer move.
- Never `pkill -f` a pattern that matches your own shell command line; use `pkill -x yagni-commander`.
- X11 only, no window manager: Wayland, tiling (Hyprland) and macOS are untested here.
