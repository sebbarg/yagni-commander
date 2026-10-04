# Roadmap

What comes next. v1 is complete; what remains is verification on real machines, then v2 and v3, in the owner's order. How the existing features behave is in [SPEC.md](SPEC.md); a "(v2)" note there points here.

## To verify on real machines

Built and tested in a Linux container (X11 under Xvfb, no window manager); since then the owner has checked macOS and KDE on X11 and Wayland (see "Verified by the owner" below). What is left is Linux on real machines.

- **Builds and tests:** `scripts/smoke.sh` on Kubuntu and Omarchy (packages in the README; names not yet verified there).
- **Platforms:** Wayland on Hyprland, never tested (the file icons included).
- **Keys reaching the app** under Hyprland (a desktop shortcut could take them): Ctrl-Tab and Ctrl-W; Alt-F5/F6/F9; Alt-Enter; Alt-F7; Ctrl-F and Shift-F3 in the viewer; Ctrl-C in the viewer; that no desktop shortcut takes Ctrl-Shift-T (none found in Omarchy's defaults; secondary sources).
- **Lone Alt and the menu:** lone Alt and Alt-Tab under Hyprland (Wayland reports a held Alt on refocus; Alt within 200 ms of activation is ignored for that reason); Alt+drag of a window on Plasma 5 (KWin grabs the click, so the app may open the menu on Alt release).
- **Windows and focus:** that a new F3 viewer window gets keyboard focus under Hyprland, also the viewer of an archive entry; drag and auto-scroll in the viewer under Hyprland.
- **Speed:** browsing a multi-GB `.tar.xz` (the listing read decompresses it all once).
- **Directory watcher:** on Hyprland and on a network mount (only changes made from that machine show).
- **Mount points (2026-10-04):** on Linux, Shift-F8 on a folder holding a bind mount or a mounted share, and on the mount point itself: the mount's contents stay, with an error for it (the container can't mount, so only reading the real mount table is tested there); on both platforms, F6 to another filesystem of a folder holding a mount.
- **Archives:** a Windows Explorer ZipCrypto zip.
- **Terminal (Ctrl-Shift-T):** Omarchy (`$TERMINAL` = `xdg-terminal-exec` reaching an app started from the launcher).
- **Clipboard:** pasting a large viewer copy (tens of MiB) into another app.

Verified by the owner: on macOS (2026-10-04), `cargo test`, the build, the native menu, file icons, keys, viewer focus, drag and auto-scroll, the watcher (FSEvents) and its CPU, Terminal.app, F4 (also from the viewer), fonts and tabular figures, zoom keys and menu items (in a viewer window the keys zoom the viewer; every menu item but Quit is disabled there, which the owner accepts); Shift-F8 stops at a mounted disk image, inside the folder deleted and as the entry itself (macOS, 2026-10-04); F8 trash on Kubuntu lands in `~/.local/share/Trash/files` (Dolphin's trash view needs a manual refresh); Enter on a file on Linux (KDE, X11); on Kubuntu in a Plasma (Wayland) session on real hardware (2026-10-04, native Wayland on a GPU): the window frame (move, resize, close, maximize), the icon in the task manager and Alt-Tab, Tab, F5/F6/F8, Ctrl-Tab, lone Alt opening the menu, Alt-Tab away and back without a stuck Alt, F3 viewer window focus, the watcher, About (1.0.0, the link opens the browser); file icons, JetBrains Mono in the viewer and tabular figures in Size and Modified, Konsole from Ctrl-Shift-T in the panel's folder, sorting by Size and Modified (closes the report that only Name sorting worked), viewer drag with auto-scroll and Ctrl-C, keyboard focus in the viewer of a zip entry, zoom keys on the owner's layout (Ctrl-+ right of 0, Ctrl--, Ctrl-0), Alt-F9 of a real `.tar.xz`, a Windows-made zip and a 7-Zip AES zip (asks for the password), F5 byte progress on a real disk, a viewer search through a multi-GB file (no freeze, Escape stops it), an Alt-F7 search of home (Stop answers at once). On Kubuntu under X11 (2026-10-04): lone Alt, Alt-Tab and F3 viewer window focus.

## Packaging

- Installing from source on your own machines: `scripts/install-linux.sh` and `scripts/bundle-mac.sh` (2026-10-04). Verified by the owner (2026-10-04): the Linux install on KDE; the macOS bundle (icon in Finder and the Dock), and F4 from the viewer launching `editor = "/usr/local/bin/code"` (a full path) with the app started from the bundle; a bare `editor = "code"` did not start it there (minimal `PATH`); now fixed by `shell_path`, to check on macOS: a bare `code` from the bundle, and startup with a slow or noisy shell profile. To check on real machines: the launcher entry and the icon on Omarchy.
- Releases (decided 2026-10-04): a tag builds a universal macOS zip (ad hoc signed, not notarized; the README explains clearing the quarantine flag) and a Linux tarball with `install.sh`, published as a GitHub Release (`.github/workflows/release.yml`), plus a `curl | sh` installer (`scripts/install.sh`) for both. `bundle-mac.sh --dist` verified on the owner's Mac (universal: `x86_64 arm64`). Released as 1.0.0 (2026-10-04; the repo is public). Verified by the owner on macOS: the zip downloaded with a browser (blocked until `xattr`, as the README says) and the `curl | sh` installer (opens without a prompt). On Kubuntu (X11): the `curl | sh` installer and the tarball by hand. On Xubuntu 24.04 (X11, Xfce, in a VM): the one-liner, no extra packages. On Ubuntu 22.04 (VM, GNOME on Xorg): the one-liner after `sudo apt install curl`; the window stayed empty (see-through) with 22.04's Mesa 23.2 on llvmpipe until Mesa from the kisak PPA (README). On Fedora 44 (VM, GNOME, Wayland): native Wayland connects and falls back to client-side decorations, then llvmpipe (Mesa with LLVM 22) aborts compiling a shader ("Cannot select ... X86ISD::MGATHER"; upstream, see ghostty discussion 12037; `LP_NATIVE_VECTOR_WIDTH=128` does not help); dropped by the owner. To check: 22.04 with a real GPU, Arch with Hyprland (for Omarchy; a VM needs 3D acceleration). Not done, on purpose: notarization (needs a paid Apple Developer membership), `.deb` (no updates without an apt repository), Flatpak and Snap (their sandbox gets in the way of starting editors, terminals and `xdg-open` on the host).
- Later, if asked: a Homebrew tap (the official cask repo needs notability and a notarized app), an AUR `PKGBUILD`, a `.deb` via `cargo-deb`.

## v2

- **Toolbar:** with e.g. drive icons.
- **File operations:** a queue of operations, like TC's F2 queue (now one operation at a time).

## v3

- **Right-click context menu** (postponed by the owner on 2026-10-03, nothing useful to put in it yet): it reuses the menu's item model and popup builder (`menus::popup`) with its own `MenuEntry` list. Open decision: whether right-click selects, as in TC's default on Linux and Windows.
- **New and updated files pulse briefly** (discussed 2026-10-02, parked). Leanings so far: every reload of the folder a panel already shows pulses (watcher, Ctrl-R, the reload after F5/F6), never a navigation or a first listing; "updated" means same name, different size or modified time (compared in `Panel::apply`); a file that keeps changing pulses about once a second. Look not decided: a background tint in a new theme role `changed`, fading out over about 1.5 s, was proposed; no pulse with reduce motion on. Drive it from a per-name timestamp, not gpui's per-element animation state, since list rows are recreated on scroll.

## Later, unversioned

- **Archives:** creating password-protected zips, writing into an opened archive, F6 out of one, nested archives, Ctrl-PgDn, packing other formats (`.tar.gz` first), Alt-Shift-F5 (move into an archive).
