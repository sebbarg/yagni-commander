# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

## Goals

- Looks great and feels at home on macOS, Kubuntu (KDE Plasma) and Omarchy (Hyprland). Existing options fall short: Double Commander's macOS support is poor, Krusader lacks features.
- Keyboard-first, TC-style workflow with fully configurable keybindings.
- Fast (large directories, instant navigation) and secure (small attack surface, no `unsafe`).

## Decisions

- **GUI, not TUI.** Terminals intercept keys (Cmd shortcuts, Ctrl+Tab, etc.) inconsistently, so a TC keymap can't be guaranteed. A GUI also gives real multiple windows, drag and drop with Finder/Dolphin, and launching a configurable external editor (e.g. VS Code on F4).
- **gpui (Zed's UI framework), not iced.** Its action + context-scoped keymap system matches the TC model, `uniform_list` is virtualized with native smooth scrolling, text truncates with ellipsis, and redraw on resize is smooth. iced 0.14 was evaluated and rejected.
- **gpui is pinned to a Zed git commit** in the root `Cargo.toml` (`[workspace.dependencies]`). The crates.io `gpui` (0.2.2, Oct 2025) is stale. Upgrade the rev deliberately and expect small API ports; gpui gets 40-100 commits/month, mostly additive.
- **Open question: gpui-component.** gpui has no text input, dialogs, dropdowns or tables; gpui-component provides them. Its dependency `gpui-pre 0.3.7` is a snapshot of the same Zed commit we pin (`1a28cff`), so adopting it needs no gpui port today. Trade-off: `gpui-pre` is republished by a single third-party maintainer, not Zed.

## Architecture

- `crates/yagni-commander-core`: UI-agnostic core. Directory listing, sorting, panel state, and `Command`s executed by `Commander`. No gpui, no pixels, no keys.
- `crates/yagni-commander`: the gpui app.
  - `Commander` lives in a gpui `Entity`, shared by the root `FileManager` view and two `PanelView` entities. Mutations go through `execute()` in `file_manager.rs`, which calls `cx.notify()`; views react via `observe`.
  - Keys map to gpui actions (`actions.rs`), actions map to core `Command`s. The keymap is data, so a user config file can plug in there.
  - Theme is a gpui `Global` (`theme.rs`), currently hardcoded Tokyo Night (Omarchy's default).
  - Column layout and cell text live in `columns.rs`.
- `unsafe_code` is forbidden workspace-wide.

## Current features

- Two panels side by side, draggable divider, resizable window.
- Tab switches panels; Up/Down/Home/End/PageUp/PageDown move the cursor; Enter enters a directory or goes up on ".."; Backspace goes up. Going up leaves the cursor on the directory you came from.
- Mouse: click selects (and focuses that panel), double-click activates, wheel scrolls the view without moving the cursor.
- Columns: Name, Size, Modified (local time), Owner (`user:group`), Permissions (`ls -l` style). Clicking a header sorts that panel by it; clicking again reverses. Size and Modified start descending. ".." then directories always come first.
- Symlinks: Owner and Permissions describe the link itself; Size and Modified come from the target.
- Quit: Cmd+Q, Alt+F4, the macOS app menu, or closing the last window.

## Known issues and gaps

- At narrow panel widths the fixed columns (~400 px) squeeze the Name column to nothing. Needs resizable or configurable columns.
- The owner reported that only Name sorting seemed to work on their machine; Size and Modified sort correctly in headless tests. Unverified. Note that owner/permissions sorts look like name sorts in directories where those values are all equal.
- Directory reading is synchronous and blocks the UI: ~320 ms for 100k entries on Linux, ~70% of it one `stat` per entry. Near-instant on macOS. Background loading is the real fix and is required for network mounts anyway.
- Enter on a file does nothing yet.
- gpui cannot write file paths to the clipboard (copy in app, paste in Finder/Dolphin), and drag-out to other apps works on macOS and Wayland only (not X11). Both need platform code or another crate.

## Next steps

1. Collect the owner's list of TC features they actually use; write the product spec from it.
2. Decide on gpui-component.
3. Candidates already discussed: F4 opens a configurable external editor, background directory loading, resizable/configurable columns.

## Building

- Rust via rustup (latest stable).
- macOS: the full Xcode app, not just the Command Line Tools (gpui compiles Metal shaders): `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`. `gpui_platform` needs the `font-kit` feature on macOS or text renders no glyphs; it is enabled in the workspace manifest.
- Kubuntu: `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- Arch/Omarchy: `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader` plus the Vulkan driver for your GPU.

## Commands

- `cargo run --release -- [left-dir] [right-dir]`: run the app (the default workspace member).
- `cargo test --workspace` and `cargo clippy --workspace --all-targets`.
- `scripts/make-test-files.sh [count] [dir]`: create a large directory under `/tmp` for stress tests.
- `RUST_LOG=info` shows gpui's platform logs (renderer, fonts, windowing).
- Headless UI checks on Linux: run under `Xvfb` and drive with `xdotool`. gpui ignores a click sent in the same instant as the pointer move, so pause between them.
