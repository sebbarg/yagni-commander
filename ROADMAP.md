# Roadmap

What comes next. v1 is complete; what remains is verification on real machines, then v2 and v3, in the owner's order. How the existing features behave is in [SPEC.md](SPEC.md); a "(v2)" note there points here.

## To verify on real machines

Everything so far was built and tested in a Linux container (X11 under Xvfb, no window manager).

- **Builds and tests:** `cargo test` on macOS (only ever run in the Linux container); `scripts/smoke.sh` on Kubuntu and Omarchy (packages in the README; names not yet verified there); whether macOS still needs the full Xcode app now that gpui-kit compiles Metal shaders at runtime (if the build asks for `metal`, install Xcode and `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`).
- **Platforms:** native Wayland (Plasma, Hyprland), never tested; the native macOS menu (shortcut labels, check marks, About and Quit in the app menu); the file icons on macOS (CoreText font registration; whether it needs the `m` patch too) and native Wayland.
- **Keys reaching the app** under KDE, Hyprland and macOS (a desktop shortcut could take them): Ctrl-Tab and Ctrl-W; Alt-F5/F6/F9; Alt-Enter; Alt-F7 (KWin binds it to "Move Window" in some Plasma versions; the menu item works either way); Ctrl-F and Shift-F3 in the viewer; Ctrl-C in the viewer; that no desktop shortcut takes Ctrl-Shift-T (none found in KDE's or Omarchy's defaults; secondary sources).
- **Lone Alt and the menu:** lone Alt and Alt-Tab under KDE and Hyprland (Wayland reports a held Alt on refocus; Alt within 200 ms of activation is ignored for that reason); Alt+drag of a window on Plasma 5 (KWin grabs the click, so the app may open the menu on Alt release).
- **Windows and focus:** that a new F3 viewer window gets keyboard focus under a real window manager (KDE, Hyprland, macOS), also the viewer of an archive entry; drag and auto-scroll in the viewer under KDE, Hyprland and macOS.
- **Speed:** the viewer on a multi-GB file; a viewer search through one (speed, Escape); F5 byte progress on a real disk (the container clones files, so copies finish instantly); browsing a multi-GB `.tar.xz` (the listing read decompresses it all once); an Alt-F7 search of `/` or home (speed, Stop answering at once); the directory watcher's CPU when a panel shows `/` or home on macOS.
- **Directory watcher:** on macOS (FSEvents; watched paths compared ignoring case), on native Wayland (KDE, Hyprland) and on a network mount (only changes made from that machine show).
- **Archives:** extracting a real-world `.tar.xz` and a Windows-made zip; a 7-Zip AES zip and a Windows Explorer ZipCrypto zip.
- **Terminal (Ctrl-Shift-T):** Konsole on Kubuntu (`--workdir`, also when started through `x-terminal-emulator`), Omarchy (`$TERMINAL` = `xdg-terminal-exec` reaching an app started from the launcher), macOS (Terminal.app at the folder).
- **Fonts:** JetBrains Mono NL in the F3 viewer on macOS and KDE; that the Size and Modified columns line up now that they use tabular figures (`tnum`; on macOS they did not before, 2026-10-04), also under KDE.
- **Zoom:** Ctrl-= on keyboards where `=` needs Shift (macOS, KDE layouts), and the zoom items in the native macOS menu (labels, keys), on macOS and KDE. The macOS menu's Zoom items dispatch `ZoomIn` and friends, handled only in the `FileManager` context, so they probably do nothing while a viewer window has focus: check, and route them to the viewer if so.
- **Clipboard:** pasting a large viewer copy (tens of MiB) into another app.
- **macOS PATH:** F4 with `editor = "code"` when launched from Finder (apps started from Finder get a minimal `PATH`; a full path in the config is the workaround).
- **Reported, unconfirmed:** the owner saw only Name sorting work; Size and Modified sort correctly in headless tests (owner/permissions sorts look like name sorts where those values are all equal).

Verified by the owner: F8 trash on Kubuntu lands in `~/.local/share/Trash/files` (Dolphin's trash view needs a manual refresh); Enter on a file on Linux (KDE).

## Packaging

- A `.desktop` file and icon-theme install for Linux, an `.app` bundle with the `.icns` for macOS. The icons are ready in `packaging/icons/` (see ARCHITECTURE.md).

## v2

- **Settings:** a "Browse..." button for the editor field that picks the program with a file dialog and fills in its path; the field stays editable for arguments like `--wait` (decided 2026-10-01). Choosing the terminal emulator for Ctrl-Shift-T (a Settings field used instead of the detection).
- **Themes:** theme selection (`theme` key, Settings dropdown) and five built-ins (Tokyo Night, Gruvbox Dark, Everforest Dark, Catppuccin Latte, Classic) are done. Remaining: more built-in themes, user themes in `~/.config/yagni-commander/themes/*.toml`, and following the system light/dark appearance.
- **File list:** resizable columns (moved from v1 on 2026-10-02; until then, hiding columns in Settings gives Name more room).
- **Zoom:** our own menu popup, so menu rows scale with the zoom (gpui-component's `PopupMenu` rows are a fixed 26 px). Fit dialogs to the window (clamp the width, scroll the body): at large zoom in a small window some overflow (24 px in 1200x800: Settings clips at the bottom; 32 px: the hotlist Configure dialog is wider than the window).
- **Toolbar:** with e.g. drive icons.
- **Quick search:** fuzzy matching (if ever).
- **Hidden files:** files hidden only by the macOS Finder flag (if needed; they are always shown now).
- **F3 viewer:** go to line (the line scan already keeps per-MiB line counts for it); other encodings; following a growing file.
- **File operations:** a queue of operations, like TC's F2 queue (now one operation at a time).

## v3

- **Right-click context menu** (postponed by the owner on 2026-10-03, nothing useful to put in it yet): it reuses the menu's item model and popup builder (`menus::popup`) with its own `MenuEntry` list. Open decision: whether right-click selects, as in TC's default on Linux and Windows.
- **New and updated files pulse briefly** (discussed 2026-10-02, parked). Leanings so far: every reload of the folder a panel already shows pulses (watcher, Ctrl-R, the reload after F5/F6), never a navigation or a first listing; "updated" means same name, different size or modified time (compared in `Panel::apply`); a file that keeps changing pulses about once a second. Look not decided: a background tint in a new theme role `changed`, fading out over about 1.5 s, was proposed; no pulse with reduce motion on. Drive it from a per-name timestamp, not gpui's per-element animation state, since list rows are recreated on scroll.

## Later, unversioned

- **Archives:** creating password-protected zips, writing into an opened archive, F6 out of one, nested archives, Ctrl-PgDn, packing other formats (`.tar.gz` first), Alt-Shift-F5 (move into an archive).
