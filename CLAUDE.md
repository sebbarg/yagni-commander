# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

**Requirements live in `Requirements.md`** (the working document: v1/v2 scope, keymap, product decisions, build order). Read it before planning features and keep it updated when decisions change.

## Goals

- Looks great and feels at home on macOS, Kubuntu (KDE Plasma) and Omarchy (Hyprland). Existing options fall short: Double Commander's macOS support is poor, Krusader lacks features.
- Keyboard-first, TC-style workflow with fully configurable keybindings.
- Fast (large directories, instant navigation) and secure (small attack surface, no `unsafe`).

## Decisions

- **GUI, not TUI.** Terminals intercept keys (Cmd shortcuts, Ctrl+Tab, etc.) inconsistently, so a TC keymap can't be guaranteed. A GUI also gives real multiple windows, drag and drop with Finder/Dolphin, and launching a configurable external editor (e.g. VS Code on F4).
- **gpui (Zed's UI framework), not iced.** Its action + context-scoped keymap system matches the TC model, `uniform_list` is virtualized with native smooth scrolling, text truncates with ellipsis, and redraw on resize is smooth. iced 0.14 was evaluated and rejected.
- **gpui comes via gpui-kit** (Longbridge, crates.io `gpui-kit = "0.7"`). gpui-kit pins `gpui-pre 0.3.7` (a snapshot of Zed commit `1a28cff`), re-exports gpui, provides `application()`, `init()` and `open_window()`, and includes gpui-component. Import everything through `gpui_kit::` (gpui APIs) and `gpui_kit::component::` (widgets); never add `gpui` directly, since two gpui crates in one build are incompatible types. The crates.io `gpui` itself (0.2.2, Oct 2025) is stale. Trade-off accepted: the snapshot is republished by a third party, not Zed, and the dependency tree is large (~700 crates).
- **gpui-component** (via gpui-kit) provides inputs, dialogs, menus, the settings component and progress, which gpui lacks. Its theme is fed our palette from `assets/tokyo-night.json` (see `theme.rs`).

## Architecture

- `crates/yagni-commander-core`: UI-agnostic core. Directory listing, sorting, panel state, and `Command`s executed by `Commander`. No gpui, no pixels, no keys.
- `crates/yagni-commander`: the gpui app.
  - `Commander` lives in a gpui `Entity`, shared by the root `FileManager` view and two `PanelView` entities. Mutations go through `execute()` in `file_manager.rs`, which calls `cx.notify()`; views react via `observe`.
  - Keys map to gpui actions (`actions.rs`), actions map to core `Command`s. The keymap is data, so a user config file can plug in there.
  - Theme is a gpui `Global` (`theme.rs`), currently hardcoded Tokyo Night (Omarchy's default). `apply_component_theme` installs the same palette into gpui-component.
  - The window is opened with `gpui_kit::open_window`, which wraps `FileManager` in gpui-kit's `Root` (needed for dialogs and notifications).
  - Window position/size: `window_state.rs` keeps the latest geometry in a `WindowState` global (updated via `observe_window_bounds`) and saves it on quit; restore falls back to centered if the geometry no longer overlaps a display.
  - Column layout and cell text live in `columns.rs`.
- Settings: `Config` (core, `config.rs`) is a hand-editable TOML file, created from a commented template on first start. Storage helpers (`storage.rs`, core) resolve platform paths, treat a missing file as defaults, write atomically, and never overwrite a file that fails to parse; the app then shows the error in the status line and runs on defaults.
  - Config: `~/.config/yagni-commander/config.toml` (Linux), `~/Library/Application Support/yagni-commander/config.toml` (macOS).
  - State: `~/.local/state/yagni-commander/state.toml` (Linux), same folder as config on macOS.
- `unsafe_code` is forbidden workspace-wide.

## Current features

- Two panels side by side, draggable divider, resizable window.
- Tab switches panels; Up/Down/Home/End/PageUp/PageDown move the cursor; Enter enters a directory or goes up on ".."; Backspace goes up. Going up leaves the cursor on the directory you came from.
- Mouse: click selects (and focuses that panel), double-click activates, wheel scrolls the view without moving the cursor.
- Columns: Name, Size, Modified (local time), Owner (`user:group`), Permissions (`ls -l` style). Clicking a header sorts that panel by it; clicking again reverses. Size and Modified start descending. ".." then directories always come first.
- Symlinks: Owner and Permissions describe the link itself; Size and Modified come from the target.
- Quit: Cmd+Q, Alt+F4, the macOS app menu, or closing the last window.
- Window position and size are restored on start.
- Config: `case_sensitive_sort` applies to both panels; `editor` is loaded but not used until F4 exists.

## Known issues and gaps

- At narrow panel widths the fixed columns (~400 px) squeeze the Name column to nothing. Needs resizable or configurable columns.
- The owner reported that only Name sorting seemed to work on their machine; Size and Modified sort correctly in headless tests. Unverified. Note that owner/permissions sorts look like name sorts in directories where those values are all equal.
- Directory reading is synchronous and blocks the UI: ~320 ms for 100k entries on Linux, ~70% of it one `stat` per entry. Near-instant on macOS. Background loading is the real fix and is required for network mounts anyway.
- Enter on a file does nothing yet.
- gpui cannot write file paths to the clipboard (copy in app, paste in Finder/Dolphin), and drag-out to other apps works on macOS and Wayland only (not X11). Both need platform code or another crate.

## Next steps

Follow the v1 build order in `Requirements.md`. Also pending from earlier: background directory loading and resizable/configurable columns (see Known issues).

## Building

- Rust via rustup (latest stable).
- macOS: Xcode. gpui-kit enables gpui's `runtime_shaders` and `font-kit` features, so Metal shaders compile at runtime and the full Xcode app may no longer be required (unverified; if the build asks for `metal`, install Xcode and `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`).
- Kubuntu: `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- Arch/Omarchy: `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader` plus the Vulkan driver for your GPU.

## Commands

- `cargo run --release -- [left-dir] [right-dir]`: run the app (the default workspace member).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets`.
- `cargo llvm-cov --workspace --summary-only`: coverage (needs `cargo install cargo-llvm-cov` and `rustup component add llvm-tools-preview`). Core is kept near 100%; the uncovered lines need root or a file owned by an unknown uid.
- App tests use gpui's `TestAppContext` (`#[gpui_kit::test]`, `simulate_keystrokes`) to drive the real keymap in a headless window. gpui's test window does not expose the title, so title text lives in a pure function.
- `scripts/make-test-files.sh [count] [dir]`: create a large directory under `/tmp` for stress tests.
- Try config/state without touching your real files: set `XDG_CONFIG_HOME` and `XDG_STATE_HOME` (Linux) to scratch directories.
- `RUST_LOG=info` shows gpui's platform logs (renderer, fonts, windowing).
- Headless UI checks on Linux: run under `Xvfb` and drive with `xdotool`. gpui ignores a click sent in the same instant as the pointer move, so pause between them.
