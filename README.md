# yagni-commander

A keyboard-first, dual-pane file manager in the spirit of Total Commander, for macOS and Linux (KDE Plasma, Hyprland). Written in Rust with [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI framework behind the Zed editor.

It does the part of Total Commander that I actually use, not all of it ("you aren't gonna need it"). Early days: no releases yet, build it from source.

## Features

- Two panels, Tab switches between them. Columns Name, Size, Modified, Owner, Permissions; click a header to sort.
- File icons by name and extension (like `eza --icons`), from a bundled Nerd Font.
- Total Commander keys: F3 view, F4 edit (in your editor), F5 copy, F6 move, F7 new folder, F8 trash, Shift-F8 delete, F2 rename, Shift-F4 new file, Alt-Z, Ctrl-U, Ctrl-R. The full keymap is in [SPEC.md](SPEC.md#keyboard).
- Copy, move and delete run in the background with progress, Cancel and a prompt per conflict.
- Type to jump: a quick search box finds names starting with what you type.
- A viewer (F3) that opens multi-GB files instantly.
- Panels follow changes made by other programs.
- Hidden files on Ctrl-., folders and window position remembered between runs.
- Three themes (Tokyo Night, Gruvbox Dark, Classic), switched live in the settings dialog (Ctrl-,), and a plain TOML config file; an optional log of every file operation.

## Building

Install Rust with [rustup](https://rustup.rs/) (latest stable), then the system packages:

- **macOS:** Xcode.
- **Kubuntu / Debian:** `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- **Arch / Omarchy:** `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader`, plus the Vulkan driver for your GPU.

Then:

```sh
cargo run --release -- [left-folder] [right-folder]
```

Without folders it reopens the ones from the last run. `cargo install --path crates/yagni-commander` puts the binary in `~/.cargo/bin`.

## Configuration

The settings dialog (Ctrl-,) edits a TOML file you can also edit by hand; Ctrl-R re-reads it.

- Linux: `~/.config/yagni-commander/config.toml`
- macOS: `~/Library/Application Support/yagni-commander/config.toml`

Set `editor` to the command F4 runs, for example `editor = "code --wait"`.

Set `theme` to `tokyo-night` (the default), `gruvbox-dark` or `classic` (light), or pick one in the settings dialog; it switches at once.

## Development

`cargo test --workspace`, `cargo clippy --workspace --all-targets` and, on Linux, `scripts/smoke.sh`, which drives the real app under Xvfb. It needs `sudo apt install xvfb xdotool x11-utils imagemagick xclip mesa-vulkan-drivers` (Kubuntu) or `sudo pacman -S xorg-server-xvfb xdotool xorg-xprop imagemagick xclip vulkan-swrast` (Arch).

- [SPEC.md](SPEC.md): what the app does, and why.
- [ARCHITECTURE.md](ARCHITECTURE.md): how the code is organized, technical decisions, known issues.
- [ROADMAP.md](ROADMAP.md): what comes next.
- [CLAUDE.md](CLAUDE.md): how we work on it (with Claude Code).

## License

MIT, see [LICENSE](LICENSE). Bundled third-party material (the icon font, the icon table and JetBrains Mono) is listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
