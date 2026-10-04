# yagni-commander

A YAGNI-minded, keyboard-first, dual-pane file manager for macOS and Linux. It does the part of a classic commander that I actually use, not all of it ("you aren't gonna need it"). Written in Rust with [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI framework behind the Zed editor.

More than a little inspired by these GOATs:

- [DOS Navigator](https://en.wikipedia.org/wiki/DOS_Navigator)
- [Total Commander](https://en.wikipedia.org/wiki/Total_Commander)
- [Double Commander](https://en.wikipedia.org/wiki/Double_Commander)

and of course the OG, [Norton Commander](https://en.wikipedia.org/wiki/Norton_Commander).

Aimed at macOS, KDE Plasma and Hyprland (Omarchy). Tested on macOS and KDE on X11; Hyprland and Wayland are untested so far. Early days.

## Features

- Two panels, Tab switches between them. Columns Name, Size, Modified, Owner, Permissions; click a header to sort.
- File icons by name and extension (like `eza --icons`), from a bundled Nerd Font.
- Total Commander keys: F3 view, F4 edit (in your editor), F5 copy, F6 move, F7 new folder, F8 trash, Shift-F8 delete, F2 rename, Shift-F4 new file, Alt-Z, Ctrl-U, Ctrl-R. The full keymap is in [SPEC.md](SPEC.md#keyboard).
- Copy, move and delete run in the background with progress, Cancel and a prompt per conflict.
- Type to jump: a quick search box finds names starting with what you type.
- A viewer (F3) that opens multi-GB files instantly.
- Panels follow changes made by other programs.
- Hidden files on Ctrl-., folders and window position remembered between runs.
- Five themes (Tokyo Night, Gruvbox Dark, Everforest Dark, Catppuccin Latte, Classic), switched live in the settings dialog (Ctrl-,), and a plain TOML config file; an optional log of every file operation.

## Installing

Download the file for your system from [Releases](https://github.com/sebbarg/yagni-commander/releases), or build it yourself (below). `SHA256SUMS` there lists each file's checksum.

**macOS** (`yagni-commander-<version>-macos.zip`, Apple Silicon and Intel): unzip it and move `yagni-commander.app` to Applications. The app is not notarized by Apple (that needs a paid developer account), so macOS refuses to open a downloaded copy. Remove the download mark once:

```sh
xattr -dr com.apple.quarantine /Applications/yagni-commander.app
```

or try to open it, then choose Open Anyway in System Settings > Privacy & Security. Built from source, it opens without this step.

**Linux** (`yagni-commander-<version>-linux-x86_64.tar.gz`, built on Ubuntu 22.04, so it needs glibc 2.35 or newer):

```sh
tar -xzf yagni-commander-*-linux-x86_64.tar.gz
cd yagni-commander-*-linux-x86_64
./install.sh
```

It installs into `~/.local` (the binary, a launcher entry and the icons), so it shows in your app menu; no root needed. `./install.sh uninstall` removes it.

## Building

Install Rust with [rustup](https://rustup.rs/) (1.95 or newer; an older one stops with "requires rustc 1.95", fixed by `rustup update stable`), then the system packages:

- **macOS:** Xcode.
- **Kubuntu / Debian:** `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- **Arch / Omarchy:** `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader`, plus the Vulkan driver for your GPU.

Then:

```sh
cargo run --release -- [left-folder] [right-folder]
```

Without folders it reopens the ones from the last run.

To install your build as an app with its icon:

- **Linux:** `scripts/install-linux.sh` puts the binary in `~/.local/bin`, plus a launcher entry and icons under `~/.local/share`, so it shows in the app menu. `scripts/install-linux.sh uninstall` removes them.
- **macOS:** `scripts/bundle-mac.sh` builds `yagni-commander.app` and copies it to `~/Applications`, where Finder, Launchpad and Spotlight find it.

## Configuration

The settings dialog (Ctrl-,) edits a TOML file you can also edit by hand; Ctrl-R re-reads it.

- Linux: `~/.config/yagni-commander/config.toml`
- macOS: `~/Library/Application Support/yagni-commander/config.toml`

Set `editor` to the command F4 runs, for example `editor = "code --wait"`.

Set `theme` to `tokyo-night` (the default), `gruvbox-dark`, `everforest-dark`, `catppuccin-latte` (light) or `classic` (light), or pick one in the settings dialog; it switches at once.

## Development

`cargo test --workspace`, `cargo clippy --workspace --all-targets` and, on Linux, `scripts/smoke.sh`, which drives the real app under Xvfb. It needs `sudo apt install xvfb xdotool x11-utils imagemagick xclip mesa-vulkan-drivers` (Kubuntu) or `sudo pacman -S xorg-server-xvfb xdotool xorg-xprop imagemagick xclip vulkan-swrast` (Arch).

- [SPEC.md](SPEC.md): what the app does, and why.
- [ARCHITECTURE.md](ARCHITECTURE.md): how the code is organized, technical decisions, known issues.
- [ROADMAP.md](ROADMAP.md): what comes next.
- [CLAUDE.md](CLAUDE.md): how we work on it (with Claude Code).

## License

MIT, see [LICENSE](LICENSE). Bundled third-party material (the icon font, the icon table and JetBrains Mono) is listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
