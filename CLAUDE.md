# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

@ARCHITECTURE.md

## Where to look

- `SPEC.md`: what the app does and why (keys, dialogs, file operations, owner decisions). Read the relevant part before changing behavior.
- `ROADMAP.md`: v2 and v3, open decisions, and what waits for verification on real machines.
- `ARCHITECTURE.md` (imported above): goals, technical decisions, code map, known issues, gotchas, testing.
- `README.md`: features, building, configuration, for users.

## Current state (update at the end of every session; not for work that is built but not released yet: git log has it)

As of 2026-11-13: v1 is complete, including the app icon (`app-icon/`), install scripts and a release workflow (`packaging/`, `install/`, `release/`, `.github/workflows/release.yml`; ARCHITECTURE, Code map, "Scripts by purpose"; latest release 1.14.0 (2026-11-13), the repo is public), and zoom is merged into `main` (Ctrl-=/+/-/0 and keypad +/-, a UI level and a viewer level, modeled on Zed; known limits in SPEC (Zoom)). Themes are done and merged on `main` (Tokyo Night, Gruvbox Dark, Everforest Dark, Catppuccin Latte, Classic; `theme` key and Settings dropdown). `cargo test --workspace` (735 app, 917 core; needs OpenSSH's `sftp-server`, see README), clippy and fmt are green and `scripts/smoke.sh` passes end to end. A narrow panel drops optional columns (Permissions, Owner, Modified) so Name keeps 120 px (SPEC, File list). The mounts dropdown (Alt-F1/Alt-F2, SPEC Mounts) is on `main`; a USB stick verified on Kubuntu and macOS, the rest of the filter waits for real machines (ROADMAP). The cursor bar dims while the file manager lacks focus (SPEC, File list). Shift with the cursor keys toggles each entry the cursor leaves (SPEC, Keyboard). Ctrl-Shift-C copies the cursor entry's name, Ctrl-Alt-C its parent folder (SPEC, Keyboard). On Wayland the main window opens once gpui knows the displays, so the saved size is restored (verified on Kubuntu 26.04; the position stays the compositor's). Dark themes draw dialogs in the popups' color, and dialogs use our own `Dropdown` instead of gpui-component's `Select` (ARCHITECTURE, Themes). Remote folders over SFTP are released in 1.7.0 (SPEC Remote folders: Ctrl-F Connect over `~/.ssh/config`, every file operation, F3/F4, ssh prompts in the app); ROADMAP v2 lists what waits for real hosts. Markdown mode in the F3 viewer (`M`, SPEC F3 viewer; `viewer_view/markdown.rs`) is released in 1.8.0; headings scale with the viewer's zoom. Local images show (core `viewer::images`), remote ones as alt text, nothing fetched. Limits for speed: files up to 1 MiB, paragraphs up to 64 KiB, else a box and the text view stays (measured: within them it shows at once; one 4 MiB paragraph froze the window for 65 s). Released in 1.8.1 to 1.8.3: `zip` 9.1.3 and `rustls` 0.23.46 for Dependabot's advisories (none exploitable here: listing and extraction both read zip's central directory); inside an archive Enter opens a read-only private copy with the system's opener and tabs read `zip:docs` (SPEC, Keyboard (Enter) and Tabs); Space toggles in place and Insert toggles and moves down (SPEC, Keyboard); `?` (or a button at the right of the status line) shows the F3 viewer's keys over the text (SPEC, F3 viewer; `viewer_view/help.rs`), and `T`, `H` and `M` pick text, hex and Markdown mode (no toggles). Released in 1.8.4: Ctrl-Shift-T on a remote folder offers its ssh command (copy, or copy and start the terminal in home; SPEC, Keyboard). Released in 1.8.5: F3 opens `.md`/`.markdown` files in Markdown mode (SPEC, F3 viewer). Released in 1.8.6: the scripts by purpose (root `build.sh`, `run.sh`, `test.sh`, `clean.sh`; `install/`, `packaging/`, `release/`; ARCHITECTURE, "Scripts by purpose"), verified on Linux and macOS. Released in 1.9.0: the Git branch in the path header (`main ↑2 ↓1` and a drawn dot when anything changed; never waits for git; SPEC, Git branch; core `git`, `file_manager/git.rs`; `git` key and a Git switch in Settings' Panels group), and the Settings dialog in two columns, one in a narrow window or at high zoom (SPEC, Config). The other Git ideas wait in ROADMAP v2. The landing page is in `docs/` (GitHub Pages from `main`'s `docs/` at `commander.yagni.dk`; ARCHITECTURE, Code map); it goes live once the DNS record and the Pages setting are in place. Released in 1.9.3: the F3 viewer lights a reload button beside `?` when its file changes on disk, Ctrl-R or a click reloads (SPEC, F3 viewer; `viewer_view/reload.rs`). Released in 1.10.0: the update button at the right of the status bar (`check_updates`, off by default; SPEC, Update button; `file_manager/self_update.rs`; install attempts in the operation log), verified by the owner on Linux and macOS; a taller, centered status bar; the F3 viewer's reload and `?` buttons give focus back to the text. Released in 1.11.0: user commands (Ctrl-S, your own programs for the cursor entry; SPEC, Keyboard; core `user_commands`; Ctrl-S stays theirs, owner 2026-11-13: TC's Ctrl-S quick filter isn't wanted, Alt-F7's Feed to listbox covers it). Released in 1.12.x: image mode in the F3 viewer (`I`, zoom and pan; `viewer_view/picture.rs`). Released in 1.13.0: TC's selection keys (Num+/Num- by mask, Num*, Alt-Num+/-, Deselect all) in a new Mark menu with the keypad keys labeled `Num +` (they don't exist on macOS: menu only; core `select`, `file_manager/select.rs`), folder history per tab (Alt-Left/Alt-Right, Alt-Down; core `history`, `file_manager/history.rs`), folder sizes (Alt-Shift-Enter; `file_manager/sizes.rs`; links to folders not followed) and Shift-F5 (copy beside itself); SPEC, Keyboard and Menu. Released in 1.14.0: Alt letters in every dialog (a label's underlined letter, Alt and it from anywhere in the dialog; none on a button Enter or Escape already gives, except Find files' Search; SPEC, Error messages, "Alt letters in every dialog"; `button_row::dialog_keys`, `DialogAlt`), and Settings with Cancel and OK (changes still preview at once; Cancel or Escape writes the file back as it was; SPEC, Config). On macOS Cmd works wherever Ctrl does (not Ctrl-Tab), and the native menus show Cmd (`actions::with_cmd_twins`; SPEC, Modifier keys). macOS menus from the keyboard stay the system's Ctrl-F2 (owner, 2026-11-13: no in-app way without the Accessibility permission; README says how to turn it on). Next: verification on real hosts, then the owner's choice from `ROADMAP.md`.

Distro testing (2026-10-04): Kubuntu (Wayland and X11) and Omarchy (Hyprland, on a Dell Precision 5510) passed their checklists (ROADMAP). Still to verify on real machines: the window-size changes on KDE Wayland and for viewers (X11 and Hyprland passed), `cargo test` on Omarchy (Kubuntu passed, 2026-10-27), `scripts/smoke.sh` on both, and the remaining ROADMAP items.

## How we work

- We always work on `main`, also for commits, unless the owner says otherwise. No feature branches.
- The owner reviews each step; commit only when asked (they say "commit"). Commits are local; the owner pushes (remote: `github.com/sebbarg/yagni-commander`).
- Every change: tests (core near 100% coverage; gpui app tests for every key, mouse action and dialog), `cargo clippy --workspace --all-targets` with zero warnings, `cargo fmt`, then `scripts/smoke.sh` and a look at its screenshots. Extend the smoke script when a feature is visible or touches the OS (trash, editor, state file).
- Keep the docs current: `SPEC.md` when the owner decides behavior, `ROADMAP.md` when an item is done, added or verified, `ARCHITECTURE.md` when the code's structure or a technical decision changes, and Current state above at the end of a session. No history here: git log keeps it.
- User-facing text and docs: no em dashes.
- Dialog tab order (a recurring bug): every new or changed dialog is checked with Tab and Shift-Tab, in an app test (`mod tab_order`, `assert_order`) and in the headless app. The rule: reading order (top to bottom, left to right), buttons last, Tab into the buttons on the first and Shift-Tab on the last, disabled controls skipped, round at both ends, never out of the top dialog, every stop visibly focused (SPEC, "Tab order in every dialog"; ARCHITECTURE, Gotchas).

## Fresh dev container (Ubuntu 24.04, no display)

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
