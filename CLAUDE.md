# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

**Requirements live in `Requirements.md`** (the working document: v1/v2 scope, keymap, product decisions, build order). Read it before planning features and keep it updated when decisions change.

## Status and handoff (update at the end of every session)

As of 2026-09-30:

- **Done (v1 build order in `Requirements.md`):** step 1 (gpui-kit + gpui-component, config and window state), step 2 (selection), step 3 (F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R, quick search), step 4 (Ctrl-. hidden files), step 5 (file-operation engine), step 6 (F5/F6/F8 dialogs and progress), plus themes-as-data and modal error boxes.
- **Next: step 7, F3 viewer** (see `Requirements.md`, F3 viewer): read-only UTF-8 text in its own window, memory-mapped, lines indexed lazily, must open GB-sized files instantly.
- **Temporary:** F12 opens a "Button test" dialog (`button_test` in `file_manager/commands.rs`) for trying the dialog button row. Remove before v1.
- **Also required before v1:** config changes must apply without restart; background directory loading; a directory watcher that reloads panels on outside changes; resizable/configurable columns (see Known issues).
- **Waiting on the owner to verify on real machines:** whether macOS still needs the full Xcode app with gpui-kit's runtime shaders; the "only Name sorting works" report; F4 with `editor = "code"` when launched from Finder (PATH); behavior on native Wayland (Plasma, Hyprland), which has never been tested here.

### How we work

- The owner reviews each step; commit only when asked (they say "commit"). Commits are local; the owner pushes (remote: `github.com/sebbarg/yagni-commander`).
- Every change: tests (core near 100% coverage; gpui tests for keymap/dialogs), `cargo clippy --workspace --all-targets` with zero warnings, `cargo fmt`, then a headless run with screenshots for anything visible.
- Update `Requirements.md` when behavior decisions are made, and this file's Status section when a step completes.
- User-facing text and docs: no em dashes.

## Goals

- Looks great and feels at home on macOS, Kubuntu (KDE Plasma) and Omarchy (Hyprland). Existing options fall short: Double Commander's macOS support is poor, Krusader lacks features.
- Keyboard-first, TC-style workflow with fully configurable keybindings.
- Fast (large directories, instant navigation) and secure (small attack surface, no `unsafe`).

## Decisions

- **GUI, not TUI.** Terminals intercept keys (Cmd shortcuts, Ctrl+Tab, etc.) inconsistently, so a TC keymap can't be guaranteed. A GUI also gives real multiple windows, drag and drop with Finder/Dolphin, and launching a configurable external editor (e.g. VS Code on F4).
- **gpui (Zed's UI framework), not iced.** Its action + context-scoped keymap system matches the TC model, `uniform_list` is virtualized with native smooth scrolling, text truncates with ellipsis, and redraw on resize is smooth. iced 0.14 was evaluated and rejected.
- **gpui comes via gpui-kit** (Longbridge, crates.io `gpui-kit = "0.7"`). gpui-kit pins `gpui-pre 0.3.7` (a snapshot of Zed commit `1a28cff`), re-exports gpui, provides `application()`, `init()` and `open_window()`, and includes gpui-component. Import everything through `gpui_kit::` (gpui APIs) and `gpui_kit::component::` (widgets); never add `gpui` directly, since two gpui crates in one build are incompatible types. The crates.io `gpui` itself (0.2.2, Oct 2025) is stale. Trade-off accepted: the snapshot is republished by a third party, not Zed, and the dependency tree is large (~700 crates). Its docs are thin: read the source under `~/.cargo/registry/src/*/gpui-component-0.7.0`, `gpui-base-0.7.0`, `gpui-pre-0.3.7`.
- **gpui-component** (via gpui-kit) provides inputs, dialogs, menus, the settings component and progress, which gpui lacks.
- **Themes are data.** A theme is a TOML file (`crates/yagni-commander/assets/themes/`) with `name`, `mode` (dark/light) and ~19 colors named by role (`accent`, `selected`, `directory`, ...), never by hue. Every background role has a matching text role (`accent`/`text_on_accent`). gpui-component's theme is derived from these roles in code (`Theme::component_config`), including hover/pressed shades, so theme authors only define roles and we don't depend on gpui-component's theme format. Theme selection and user theme files are v2 (see `Requirements.md`).
- **Errors are modal.** Errors show in a centered message box that stays until dismissed (`show_error` in `file_manager/commands.rs`); never toast notifications for errors.
- **Same keymap on all platforms**, Ctrl not Cmd on macOS (only Quit differs: Cmd-Q / Alt-F4). F-keys assume Fn is held on Mac keyboards.

## Architecture

- `crates/yagni-commander-core`: UI-agnostic core, no gpui. Modules: `commander` (dual-panel state, `Command`s, rename/mkdir/quick-search entry points), `panel` (listing, cursor, sort, selection, `Summary`), `entry` (reading directories), `sort`, `format` (size, time, permissions text), `fs_ops` (rename, no-replace rename, make directory, name validation), `file_ops` (copy/move/trash engine `run`, background `Job`), `launch` (external editor), `quick_search` (the typed prefix), `config`, `storage` (platform paths, TOML load/save, atomic writes).
- `crates/yagni-commander`: the gpui app.
  - `Commander` lives in a gpui `Entity`, shared by the root `FileManager` view and two `PanelView` entities. Mutations go through `execute()` in `file_manager.rs`, which calls `cx.notify()`; views react via `observe`.
  - Keys map to gpui actions (`actions.rs`), actions map to core `Command`s or `FileManager` methods. The keymap is data, so a user config file can plug in there.
  - `file_manager/file_ops.rs`: F5/F6/F8 around a core `file_ops::Job`. A `spawn_in` timer polls the job every 50 ms; the progress dialog (a `ProgressView` entity inside a gpui-component `Dialog`) opens after 6 polls or on the first conflict; the conflict dialog stacks on top of it. `finish_job` closes the progress dialog, reloads both panels and shows failures.
  - `button_row.rs`: the button row used by every dialog (see Gotchas).
  - `file_manager/commands.rs`: F2/F7/Shift-F4 name prompt (`prompt_name`, a gpui-component `Dialog` with an `Input`), F4 and Shift-F4 editor launch, quick search key handling, `show_error`.
  - The active `Theme` is a gpui `Global` (`theme.rs`); `Theme::install` also applies it to gpui-component. Built-in default: Tokyo Night (Omarchy's default).
  - Views take every color from `Theme::get(cx).colors`. `clippy.toml` bans gpui's color constructors (`rgb`, `hsla`, `black`, ...) so literals can't creep in; add a new role to `Colors` and every theme file instead.
  - The loaded `Config` is a gpui global, `CurrentConfig` (`main.rs`).
  - The window is opened with `gpui_kit::open_window`, which wraps `FileManager` in gpui-kit's `Root` (needed for dialogs).
  - State file: `app_state.rs` keeps an `AppState` global (window geometry, updated via `observe_window_bounds`, and `show_hidden`) and saves it on quit. It is loaded before the window opens, since `Commander::new` needs `show_hidden`. Window restore falls back to centered if the geometry no longer overlaps a display.
  - Hidden entries: `Panel` keeps them in a separate unsorted list while hidden, so `entries()` and every index (cursor, quick search, summary, selection) only see visible ones.
  - Column layout and cell text live in `columns.rs`; panel rendering in `panel_view.rs`.
- Settings: `Config` is a hand-editable TOML file, created from a commented template on first start. A file that fails to parse is never overwritten; the app shows the error in the status line and runs on defaults.
  - Config: `~/.config/yagni-commander/config.toml` (Linux), `~/Library/Application Support/yagni-commander/config.toml` (macOS). Keys: `editor`, `case_sensitive_sort`.
  - State: `~/.local/state/yagni-commander/state.toml` (Linux), same folder as config on macOS.
- `unsafe_code` is forbidden workspace-wide.

## Current features

- Two panels side by side, draggable divider, resizable window; position and size restored on start.
- Tab switches panels; Up/Down/Home/End/PageUp/PageDown move the cursor; Enter enters a directory or goes up on ".."; Backspace goes up. Going up leaves the cursor on the directory you came from.
- Selection: Space toggles the entry under the cursor and moves down; Ctrl-A selects all. Selected entries are orange (the cursor bar turns orange on a selected entry). The footer shows totals, or "N of M selected, size of total". Selection is per panel, kept by name across re-sorts, cleared on directory change. `Panel::targets()` (selection, else the cursor entry, never "..") is what file operations will act on.
- F5 copy, F6 move (destination prompt prefilled with the other panel), F8/Del trash (confirm), in the background with a progress dialog (after ~300 ms), Cancel, a per-file conflict prompt and an error summary; both panels reload at the end.
- F2 rename (name preselected up to the last extension; never overwrites; case-only rename allowed), F7 new directory (nested `a/b/c` allowed, nothing outside the current directory), F4 opens the entry (or the directory on "..") in `editor`; Shift-F4 asks for a file name, creates the file (or keeps an existing one), puts the cursor on it and opens it in `editor`. Alt-Z shows this directory in the other panel, Ctrl-U swaps panels, Ctrl-R reloads both. Typing opens a quick search box in the panel footer and jumps to the first name starting with the typed text; Down/Up step through matches (wrapping), Backspace shortens it, Escape or any other command closes it. The search state lives in `Commander` (`search_*`), so every command ends it.
- Mouse: click moves the cursor (and focuses that panel), double-click activates, wheel scrolls the view without moving the cursor.
- Directory names show as `[name]` (display only; ".." unbracketed).
- Columns: Name, Size, Modified (local time), Owner (`user:group`), Permissions (`ls -l` style). Clicking a header sorts that panel by it; clicking again reverses. Size and Modified start descending. ".." then directories always come first. `case_sensitive_sort` in the config switches name comparison.
- Hidden files (name starts with `.`) are hidden by default; Ctrl-. toggles them in both panels, remembered in the state file. Shown hidden names use the `hidden` theme role. Hiding deselects them and moves the cursor off them.
- Symlinks: Owner and Permissions describe the link itself; Size and Modified come from the target.
- Quit: Cmd+Q, Alt+F4, the macOS app menu, or closing the last window.

## Known issues and gaps

- At narrow panel widths the fixed columns (~400 px) squeeze the Name column to nothing. Needs resizable or configurable columns.
- The owner reported that only Name sorting seemed to work on their machine; Size and Modified sort correctly in headless tests. Unverified. Note that owner/permissions sorts look like name sorts in directories where those values are all equal.
- Directory reading is synchronous and blocks the UI: ~320 ms for 100k entries on Linux, ~70% of it one `stat` per entry. Near-instant on macOS. Background loading is the real fix and is required for network mounts anyway.
- Enter on a file does nothing yet (v2: open with associated program).
- Config changes need a restart; they should apply immediately (see `Requirements.md`, Config).
- No-replace rename (F2, move) is atomic only on Linux glibc (`renameat2`). On macOS it checks, then renames, so another process could create the target in between (TOCTOU); fixing it needs `renamex_np`, which nix doesn't offer without our own `unsafe`.
- No menu bar on Linux yet (gpui has no native Linux menu; needs an in-window menu, step 8). The macOS menu has only Quit.
- A config problem shows in the status line and clears on the next keyboard command, not on mouse clicks.
- `PanelView::visible_rows` reads gpui scroll-handle internals (public fields); likely to break on a gpui upgrade.
- gpui cannot write file paths to the clipboard (copy in app, paste in Finder/Dolphin), and drag-out to other apps works on macOS and Wayland only (not X11). Both need platform code or another crate.

## Gotchas

- Tests must never call the real `trash::delete` (it would fill the owner's trash): core `file_ops` tests use `run_with` / `Job::spawn_with` with a fake trash function, and app tests never confirm the F8 dialog.
- App tests of F5/F6 run a real worker thread: drive them with `advance_clock` + `run_until_parked` in a loop (`wait_until` in `file_manager.rs`); the poll timer only fires on the test clock.

- gpui-component `Dialog` maps Enter to its OK action (`on_ok`) for the whole dialog, even when a text input or another button has focus (upstream bug, not reported as of 2026-09-30), and has no arrow-key navigation. So every dialog's buttons are our `ButtonRow` (`button_row.rs`), never `DialogFooter`/`DialogAction`/`open_alert_dialog`: it keeps its own selected button and binds Left/Right, Tab/Shift-Tab, Enter and Space in its own key context. Focus the row when a dialog opens (`focus_when_open`), except prompts, which focus their text field (Enter there still goes to `on_ok`; don't also handle the input's `PressEnter`, or OK runs twice: `enter_submits_a_prompt_exactly_once`).
- Opening a dialog moves focus to it: focus a field inside it with `window.defer` after opening. After an error box closes, focus must be put back explicitly (`show_error`'s `refocus`).
- An event handler running inside an entity's update must not update that same entity again (panics "already being updated"); defer with `window.defer`.
- `ThemeConfigColors` (gpui-component) has private fields: build it from `default()` and assign fields, not with struct-update syntax.
- macOS (unverified): apps started from Finder get a minimal `PATH`, so an `editor` like `code` may not be found; use a full path in the config if so.

## Building

- Rust via rustup (latest stable).
- macOS: Xcode. gpui-kit enables gpui's `runtime_shaders` and `font-kit` features, so Metal shaders compile at runtime and the full Xcode app may no longer be required (unverified; if the build asks for `metal`, install Xcode and `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`).
- Kubuntu: `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- Arch/Omarchy: `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader` plus the Vulkan driver for your GPU.

### Fresh dev container (Ubuntu 24.04, no display)

Containers are ephemeral; redo this at the start of a session if `~/.cargo/bin/cargo` is missing.

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
sudo apt-get install -y build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
  mesa-vulkan-drivers xvfb xdotool imagemagick
~/.cargo/bin/rustup component add llvm-tools-preview && ~/.cargo/bin/cargo install cargo-llvm-cov --locked
```

Cargo is at `~/.cargo/bin/cargo` (not on PATH in non-login shells). A clean build takes a few minutes.

## Commands

- `cargo run --release -- [left-dir] [right-dir]`: run the app (the default workspace member).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets` (must be warning-free).
- `cargo llvm-cov --workspace --summary-only`: coverage. Core is kept near 100%; the uncovered lines need root or a file owned by an unknown uid.
- App tests use gpui's `TestAppContext` (`#[gpui_kit::test]`) and build the window like the app does (`gpui_kit::init` + `Root`), so dialogs work. Drive them with `simulate_keystrokes` / `simulate_input` and `run_until_parked`; check dialogs with `window.has_active_dialog(cx)`. gpui's test window does not expose the title, so title text lives in a pure function.
- `scripts/make-test-files.sh [count] [dir]`: create a large directory under `/tmp` for stress tests.
- `RUST_LOG=info` shows gpui's platform logs (renderer, fonts, windowing).

### Headless run (to see the real app in the container)

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
