# Requirements

Working document for what yagni-commander should do. Items are v1 unless marked (v2) or (v3, later than v2).

## Decisions

- **UI widgets:** adopt gpui-component for inputs, dialogs, menus, the settings page and progress.
- **Delete:** F8/Del moves to the system trash (macOS Trash, freedesktop trash on Linux). Permanent delete is not in v1.
- **Quick search:** prefix match, no search box (see Keyboard).
- **Function keys on macOS:** assume Fn is held or the system setting "Use F1, F2, etc. keys as standard function keys" is on. No Mac-specific alternatives.
- **Modifier keys:** the same keymap on all platforms, using Ctrl (not Cmd) on macOS. The only exception is Quit: Cmd-Q on macOS, Alt-F4 on Linux. Revisit if Ctrl turns out to be a problem on macOS.

## UI

### Menu

- Main menu, following the platform's conventions: the native menu bar on macOS; on Linux an in-window menu bar, always visible, in a row above the panels. Decided 2026-10-01.
- Contents (TC-style command menus, so the menu doubles as a list of the shortcuts). Each item shows its key binding, taken from the keymap.
  - **Files:** View (F3), Edit (F4), New file (Shift-F4), Copy (F5), Move (F6), New folder (F7), Rename (F2), Move to trash (F8), Delete permanently (Shift-F8), Pack (Alt-F5), Extract (Alt-F6); Select all (Ctrl-A), Copy path (Ctrl-C, Cmd-C on macOS); Quit (Linux only, Alt-F4).
  - **Commands:** Same folder in other panel (Alt-Z), Swap panels (Ctrl-U), Reload (Ctrl-R), Directory hotlist (Ctrl-D).
  - **Show:** Hidden files (Ctrl-.), checked while shown; Sort by Name, Size, Modified, Owner, Permissions, with a check on the active panel's sort column. Choosing the checked column reverses the order, like clicking the header. The sort commands are actions, so the keymap can bind them; unbound by default.
  - **Help (Linux):** About. On macOS, About and Quit sit in the app menu (platform convention) and there is no Help menu.
  - **Settings...** (Ctrl-,): Linux at the end of Files, above Quit; macOS in the app menu, above Quit.
- Check marks follow the state: the menus are rebuilt when hidden files or the active panel's sort change.
- Menu items act on the active panel, exactly like their keys (same rules while a panel is loading or a dialog is open).
- Linux keyboard: F10 or a lone Alt (pressed and released with no other key, button or modifier in between) opens the first menu; Alt-F opens Files (also from another open menu; it doesn't close it); Up/Down and Enter inside, Left/Right between menus; Escape, F10 or a lone Alt closes it and the panel gets focus back. Alt-Z, Alt-F4 and Alt-Tab never open it (a key, mouse button or window deactivation between press and release cancels). Not while a dialog is open. On macOS F10, lone Alt and Alt-F do nothing.
- About: a dialog with the name and version (`CARGO_PKG_VERSION`) and an OK button.
- Right-click context menu (v3; postponed by the owner on 2026-10-03, nothing useful to put in it yet): it reuses the menu's item model and popup builder with its own item list. Open decision: whether right-click selects, as in TC's default on Linux and Windows.

### Config

- Settings dialog (decided 2026-10-01): a modal dialog titled "Settings", opened with Ctrl-, (all platforms) or the menu (Linux: Files > Settings..., above Quit; macOS: the app menu's Settings...). Fields, top to bottom:
  - **Editor** (`editor`): text field, hint "e.g. code --wait". Empty means no editor. Applied and saved when the field loses focus or the dialog closes. A "Browse..." button that picks the program with a file dialog and fills in its path (v2, decided 2026-10-01); the field stays editable for arguments like `--wait`.
  - **Sort names case-sensitively** (`case_sensitive_sort`): switch. Applies at once: both panels re-sort, keeping their selection.
  - **Log file operations** (`log`): switch. On starts the log at once (including the startup cleanup of old log files); off stops logging. A copy or move already running keeps logging until it ends.
  - **Keep logs for N days** (`log_keep_days`): number, 1 to 3650, described as "Older log files are deleted at startup", followed by "Log files are stored in <folder> (click to copy)" (the full path; a click copies the path and the hint turns to "(copied)"; decided 2026-10-02). An invalid value shows an error at the field and is not saved.
  - **Columns** (decided 2026-10-02): first an **Icons** switch (`icons`, on by default; see File list), then switches for Modified (`show_modified`), Owner (`show_owner`) and Permissions (`show_permissions`), all on by default; "Name and Size are always shown." Apply at once to both panels; Name takes the freed width. A panel sorted by a column that gets hidden goes back to Name ascending; the Show menu offers sort items only for shown columns, and a sort action for a hidden column does nothing.
  - One button, Close (our `ButtonRow`). Escape closes; Enter closes too. Both save what was typed first. While the days value is invalid, Enter and Close keep the dialog open with the error showing; Escape closes and drops it. Tab/Shift-Tab move through the fields and the button; Space toggles a switch.
- Saving: each change writes only its own key and keeps the rest of the file as it is (comments, layout, hand edits). A key that is only commented out (`# editor = ...`) gets a real line; emptying the editor removes its key. The new text is checked by parsing it before the file is replaced (atomic write). If writing fails, an error box says so and the change still applies for this session.
- A config file that did not parse, or no config directory: the dialog shows that problem at the top and its fields are disabled. Fix the file, then Ctrl-R. A broken file is never overwritten.
- Hand edits of the file take effect on restart or Ctrl-R; no file watcher (decided 2026-10-01: with a settings dialog, hand edits are rare). Ctrl-R reloads both panels and re-reads the config, applying editor, sort, logging, columns and icons. If the file does not parse, the status line shows "Config ignored: ..." and the current settings stay (at startup the app runs on defaults instead).
- Stored as a TOML config file in the platform config directory (e.g. `~/.config/yagni-commander/config.toml` on Linux, `~/Library/Application Support/yagni-commander/` on macOS). The settings dialog edits this file; editing it by hand also works.

### Window position and size, panel folders

- Saved automatically on exit and restored on start.
- Kept in a state file separate from the config, so the config stays hand-editable.
- Each panel reopens the folder it showed at exit, and the active panel stays active. A folder that is gone or unreadable falls back to its nearest readable parent, then to the home folder. First start (no state): both panels show the home folder.
- Command-line folders override the saved ones: `yagni-commander a b` opens `a` and `b`; with one argument the right panel keeps its saved folder.

### Themes

- v1: all colors come from a theme file; one built-in theme (Tokyo Night). No hardcoded colors in views.
- Theme selection in the config (`theme = "..."`), more built-in themes, and user themes in `~/.config/yagni-commander/themes/*.toml` (v2).
- Following the system light/dark appearance (v2).

### File list

- Directories show as `[name]` when icons are off (display only: sorting and quick search use the bare name). ".." has no brackets.
- File icons (decided 2026-10-02, spec `docs/superpowers/specs/2026-10-02-file-icons-design.md`): a Nerd Font glyph before every name, chosen by exact file name, then the name in lowercase, then extension (longest compound first), like `eza --icons`. Folders: the home folder itself gets a home glyph (decided 2026-10-02: by path, so a symlink to home does not; ".." stays an arrow), then a small special list (`.git`, `src`, `Documents`, ...), else a plain folder; ".." an up arrow; executables without a match a terminal; symlinks follow their target, broken ones a generic file. The icon takes the row's text color. With icons on, folders lose their `[ ]` brackets. On by default; `icons = false` or the Settings switch turns them off. The font (Symbols Nerd Font Mono) is bundled; the table is generated from nvim-web-devicons.
- New and updated files pulse briefly (v3; discussed 2026-10-02, parked). Leanings so far: every reload of the folder a panel already shows pulses (watcher, Ctrl-R, the reload after F5/F6), never a navigation or a first listing; "updated" means same name, different size or modified time (compared in `Panel::apply`); a file that keeps changing pulses about once a second. Look not decided: a background tint in a new theme role `changed`, fading out over about 1.5 s, was proposed; no pulse with reduce motion on. Drive it from a per-name timestamp, not gpui's per-element animation state, since list rows are recreated on scroll.
- Resizable columns (v2, moved from v1 on 2026-10-02). Until then, hiding columns in Settings gives Name more room.

### Directory loading

- Directories are read off the UI thread, so a huge directory or a slow or hung network mount never freezes the app (decided 2026-10-01; the owner uses network mounts).
- While a panel loads, it keeps showing its previous listing. A load that finishes within about 150 ms switches without any indicator. After that the panel header shows "Loading <path>... N entries" (a live count) and the old listing dims.
- While loading, that panel accepts only Escape (cancel: stay on the old listing; at startup: go to the home folder) and Tab/Shift-Tab. Dialogs and file operations on the loading panel are ignored. Quit always works, and the other panel works normally.
- A failed load leaves the panel where it was, with the error in the status line.
- Startup loads in the background too, including the fallback for a missing remembered folder (nearest readable parent, then home). Folders given on the command line are checked before the window opens; one that is not a directory ends the app with an error.
- A newer navigation replaces a pending one. A reload requested while a navigation is pending is dropped (the navigation brings a fresh listing anyway).
- A read the operating system never returns (a dead NFS server) cannot be interrupted: Escape abandons it, and its thread ends when the read returns or the app exits.
- Showing entries while a slow directory is still being read (a growing listing) is out of scope.

### Tabs

Tabs per side (v1; pulled forward from v2 on 2026-10-02; design: `docs/superpowers/specs/2026-10-02-tabs-design.md`). Ctrl-T clones the visible tab into a new one after it, Ctrl-W closes the active tab (never the last one), Ctrl-Tab/Ctrl-Shift-Tab cycle with wrap-around; all on the active side. The tab header is always shown above the path header; a tab's label is its folder's last component, tabs shrink evenly and truncate. A click activates a tab; only Ctrl-W closes one. Background tabs keep their listing; only the visible tab per side is watched, and a tab re-reads quietly when it comes to the front. Ctrl-U swaps whole sides. Tabs and each side's active tab are restored on restart; a command-line folder replaces the active tab's folder.

### Toolbar (v2)

Toolbar with e.g. drive icons.

### Error messages

- Errors are shown in a centered, modal message box with a Dismiss button. It stays until dismissed (button, Enter or Escape). No fading toasts for errors. Ctrl-C or Ctrl-Ins (also Cmd-C on macOS) copy the box's title and text to the clipboard (title, newline, text); the box stays open. Same for every message box (About). Decided with the owner 2026-10-03.
- Dialog buttons: Left/Right or Tab/Shift-Tab move between buttons, Enter or Space presses the highlighted one, Escape cancels. The default button is highlighted when the dialog opens. In a prompt, the arrow keys edit the text and Tab moves to the buttons.
- After dismissing an error from a prompt (e.g. rename), focus returns to the prompt's text field so the input can be corrected.

## Keyboard

| Key | Action |
|---|---|
| Space | Toggle selection of the entry under the cursor and move the cursor down. Selected entries are orange. ".." can't be selected. Changing directory clears the selection. |
| Ctrl-A | Select all files and directories in the panel. |
| Ctrl-C, Ctrl-Ins (also Cmd-C on macOS) | Copy the full path of the entry under the cursor (the panel's folder on "..") to the clipboard as text. Only while a panel has focus: in a text field these keys copy its text. Ignores the selection. Decided by the owner 2026-10-03. |
| F2 | Rename the file or directory under the cursor, in a dialog with the name preselected up to the last extension (like TC). Never overwrites an existing entry; a case-only rename (`foo` to `Foo`) is allowed. |
| F3 | Built-in viewer (see below). |
| F4 | Open the file or directory in the configured external editor (the panel's own directory when the cursor is on ".."). The `editor` setting is split like a shell command, e.g. `code --wait`. |
| F5 | Copy the selection, or the entry under the cursor if nothing is selected, to the other panel's directory (see File operations). |
| F6 | Move, with the same rules as F5. |
| Shift-F4 | Create a file and open it in the editor. Asks for a name, which may be a relative path like `notes/2026/todo.md`: missing folders are created, nothing outside the current directory (like F7), and the cursor goes to the first part. The field starts empty. If that file already exists it is opened as is. Without a configured editor, shows the error instead of asking. |
| F7 | Create a directory. Nested paths like `a/b/c` are allowed; nothing outside the current directory. |
| F8, Del | Move the selection, or the entry under the cursor, to the trash, after confirmation. |
| Shift-F8, Shift-Del | Delete the selection, or the entry under the cursor, permanently, after confirmation ("This cannot be undone"). Enter confirms, like TC. Runs like F8: background, progress by files, Cancel, error summary. Symlinks are deleted, never followed. |
| Alt-Z | Set the other panel's path to this panel's path. |
| Ctrl-U | Swap the two sides, with all their tabs. |
| Ctrl-R | Reload the visible tab of each side and re-read the config (see Config). |
| Ctrl-T | New tab on the active side, a copy of the visible one (see Tabs). |
| Ctrl-W | Close the active tab; the last tab of a side stays. |
| Tab, Shift-Tab | Switch to the other panel (two panels, so both keys do the same). |
| Ctrl-Tab / Ctrl-Shift-Tab | Next / previous tab on the active side, wrapping around. |
| Ctrl-, | Settings (see Config). |
| In text fields | Ctrl-C/X/V and the classic Ctrl-Ins (copy), Shift-Del (cut), Shift-Ins (paste). |
| Ctrl-. | Toggle showing hidden files, in both panels (see Hidden files). |
| F10, Alt (alone) | Open the menu (Linux; see Menu). |
| Letters, digits, `.` and other printable keys | Quick search (see below). |
| Enter | On a directory: enter it. On a file (also a double-click): a program runs: an executable file (any execute bit) that starts with an ELF or Mach-O header or `#!`, started directly (no shell) in the panel's folder, like TC. The execute bit alone isn't enough, since NTFS, FAT and SMB mounts set it on every file. Any other file goes to the system's opener (`xdg-open` on Linux, `open` on macOS), which picks the associated application. If a program can't start, or the opener can't start or exits with an error (no application for the type), an error box says so; a program's own exit code is not reported. Decided with the owner 2026-10-03 (first "never run", changed the same day after KDE's opener refused executables). |
| Alt-F5 | Pack the selection (or the entry under the cursor) into a zip, in the background like F5. The prompt holds the zip's full path in the other panel (`<name>.zip` for one entry, a file's name without its extension; `<current folder>.zip` for several), editable, `.zip` added when missing; an existing zip is replaced only after "Overwrite?". Each symbolic link asks: Follow (pack what it points to), Store as link (Windows shows it as a small text file), Leave out, Cancel, with "Same for the remaining links". Zip only (extracts on Windows, macOS, Linux desktops and phones without extra software). Decided with the owner 2026-10-03 (spec `docs/superpowers/specs/2026-10-03-archives-design.md`). |
| Alt-F6, Alt-F9 | Extract the selected archives (or the one under the cursor) into a folder, the other panel's by default; like TC (Alt-F9 is TC's alias). Formats: zip, `.tar`, `.tar.gz`/`.tgz`, `.tar.bz2`/`.tbz2`, `.tar.xz`/`.txz`, `.tar.zst`/`.tzst` (no 7z or rar). Password-protected zips (ZipCrypto and AES) ask for the password (masked, with show/hide) at their first encrypted entry; it is remembered for the rest of the job and asked again when it doesn't fit ("Wrong password."); "Skip archive" leaves that archive's encrypted entries out; never saved or logged. Decided with the owner 2026-10-03 (spec `docs/superpowers/specs/2026-10-03-zip-passwords-design.md`). Smart extraction: one top-level folder goes in as it is, anything else into a folder named after the archive. The F5 conflict prompt for existing files. Safe: nothing is written outside the folder (absolute and `..` names refused, never through a symlink), permission bits minus the umask (no setuid, setgid or sticky), no ownership. |
| Ctrl-D | Directory hotlist: a popup over the active panel with the bookmarked folders, then "Add current folder" and "Configure...". `&` in a name marks its letter (`&Projects`: P, underlined; `&&` is a literal `&`); the letter jumps there. Up/Down (wrapping), Enter, Escape, click; other keys are ignored while it is open. Picking an entry is a normal navigation; a missing folder shows its error and the panel stays (no parent fallback). "Add current folder" asks for a name (the folder's, preselected) and saves at once; folders under home are stored as `~/...`. Configure is an OK/Cancel dialog: add the current folder, rename, change paths (absolute or `~/...`; checked on OK), remove (Del) and reorder (Alt-Up/Alt-Down). Stored in `config.toml` (`[[hotlist]]` tables with `name` and `path`; a hand-written relative path is relative to home). While the config file is broken, the entries still work but Add and Configure are refused. Decided with the owner 2026-10-03 (spec `docs/superpowers/specs/2026-10-03-directory-hotlist-design.md`). |
| Alt-F7 | Find files (v2). |

- Archives later: creating password-protected zips, browsing into an archive like a folder (TC's Ctrl-PgDn), packing other formats (`.tar.gz` first), Alt-Shift-F5 (move into an archive).

### Quick search

- Typing any printable character except space (letters, digits, `.`, `-`, ...) opens a small box in the active panel's footer showing the typed text, and moves the cursor to the first entry whose name starts with it, case-insensitively.
- While the box is open, Down and Up move to the next and previous match, wrapping around at the ends. Backspace removes the last character (removing the last one closes the box); Escape closes it.
- If no entry matches the longer text, the keystroke is ignored: the box and the cursor stay as they are.
- Any other key, and any mouse click in a panel, closes the box. Enter closes it and opens the entry as usual. There is no timeout.
- Fuzzy matching (v2, if ever).

### Hidden files

- Hidden means the name starts with `.` (Linux and macOS). Files hidden only by the macOS Finder flag are always shown (v2, if needed).
- Hidden files are not shown by default. Ctrl-. toggles them for both panels, and the choice is remembered across runs (state file).
- When shown, hidden entries are drawn in a dimmer color (a theme role).
- Hiding them deselects any selected hidden entries, so file operations never act on something invisible. Quick search only matches visible entries.
- If the cursor is on an entry that gets hidden, it moves to the nearest visible entry (the next one below, else above).
- One theme role (`hidden`) colors all hidden names, so a hidden directory is not drawn in the directory color.
- If the entry under the cursor disappears on a re-read (deleted, or renamed to a hidden name), the cursor stays on the same row.

### F3 viewer

- Read-only, UTF-8 text. Every F3 opens its own window, so several files can be compared side by side. F3 on a directory or ".." does nothing; on a FIFO, socket or device it shows an error (reading a FIFO would block).
- Window title `<file name> - yagni-commander`. The first viewer opens over the main window at its size; after that the last viewer geometry is reused (one slot in the state file). Closing the main window quits the app and closes all viewers.
- Must open huge files (GB range) instantly, like TC's Lister. Works by byte position, never reading the whole file before showing it: open reads the first screenful; paging reads the next few KB (positional reads, `read_at`); Ctrl-End reads backwards from the end; the scrollbar is a byte position; the row containing it is shown (never past the last screen).
- Not memory-mapped: in Rust that needs `unsafe` (forbidden), and another program shortening the file would crash the app (SIGBUS). Positional reads use the same OS file cache and are as fast here. If the file shrinks while open, reads past the end show nothing and the next scroll clamps.
- Monospace font. Lines end at `\n`; a `\r` before it is dropped. Tabs expand to the next multiple of 8 columns; wide (CJK) characters take 2 columns. Invalid UTF-8 bytes and control characters (including a lone `\r`) show as a dim `·` (theme role `hidden`), so binary files never break the layout. A position inside a multi-byte character snaps to the next character.
- Wrap mode (always the start mode): word wrap at the window width, breaking at the last space or tab that fits, else mid-word. `W` toggles no-wrap mode, keeping the top line in place: rows are file lines, Left/Right scroll horizontally (8 columns), and lines longer than 10,000 characters are cut into 10,000-character rows.
- Giant lines (e.g. minified JSON): inside a line longer than 64 KiB, rows also break at every 64 KiB-aligned offset, so scrolling up never scans more than one block back. The visible effect is one short row every 64 KiB.
- Keys: Escape or `q` close (not F3, which is find next in v2); Up/Down one row; PageUp/PageDown one screen; Home/Ctrl-Home start of file; End/Ctrl-End end of file; `W` wrap toggle; Left/Right in no-wrap mode. Mouse wheel scrolls rows; the scrollbar can be dragged.
- Status line: file name, size, percentage (by byte), wrap or no wrap, and "line N of M" (of the top row). Line numbers need a full scan; it runs in the background ("counting lines..." until done) and stops when the window closes. The viewer is usable at once.
- Go to line (v2; the line scan already keeps per-MiB line counts for it).
- Search (v2): Ctrl-F prompts for the text; F3 finds the next match, Shift-F3 the previous one.
- Selection and copy (v2), like TC's Lister: no keyboard cursor. Drag selects (auto-scrolling past the top or bottom edge), Shift-click extends, double-click selects a word, triple-click a line, Ctrl-A selects all, Ctrl-C copies, a click clears. The selection is a byte range in the file, so it survives scrolling, `W` and resizing. Copy takes the original bytes (tabs and `\r\n` kept; invalid UTF-8 becomes U+FFFD) and is capped at 64 MiB (an error box above that). Keyboard selection only if it turns out to be missing.
- Other encodings, following a growing file (v2).
- Hex mode (v2): classic layout, 16 bytes per row: offset, hex codes, then the same bytes as characters (`.` for non-printable).

## File operations

Applies to F5 (copy), F6 (move), F8 (trash) and Shift-F8 (delete).

- Runs in the background: the UI stays responsive, with a progress dialog and Cancel.
- If a target exists, ask per file: Overwrite, Skip, Overwrite all, Skip all, Cancel.
- A move across filesystems becomes copy, then delete.
- Symlinks are copied as links. Modification times and permissions are preserved.
- Errors on individual files are reported without aborting the rest of the operation, with a summary at the end.
- Affected panels reload when the operation finishes.
- An existing directory is merged into without asking; only files and symlinks get the conflict prompt. A file never replaces a directory or the other way round (error).
- Overwriting writes a hidden temporary file next to the target and renames it over the target at the end, so a cancelled or failed overwrite leaves the old file intact. A cancelled new copy is removed.
- Copying or moving something onto itself, or a directory into itself, is an error for that entry.
- Times and permissions are preserved best effort: filesystems that can't store them (FAT, some network mounts) are not an error. A symlink's own time is not preserved.
- Pipes, sockets and device files are not copied (error for that entry).
- F5/F6 ask for the destination (decided 2026-10-01). The prompt is wide (720 px) for long paths. A relative path is taken from the active panel's directory. Missing folders on the way are created (and logged, like F7). Problems are reported by the prompt, which stays open.
  - **One entry (file or folder):** the field holds its full target path, the other panel's folder plus its name, with the name preselected up to the extension (like F2). Typing a new name or path copies or moves it there: a duplicate or a rename within one folder is fine. An existing folder, or text ending in `/`, means into that folder with the name kept. Refused: the entry itself as target, a folder into itself.
  - **Two or more:** the field holds the target folder (the other panel's), editable; the title gives the count ("Move 3 entries to"), with no list of names (owner decision 2026-10-02). Every entry keeps its name. Refused: the source folder itself, a file as target.
- F8 and Del ask for confirmation first.
- The progress dialog appears only if the job takes longer than about 300 ms, so quick copies don't flash a dialog. It counts files and bytes. Escape or Cancel stops the job at the next file (or 4 MiB chunk).
- In the conflict prompt, Enter means Overwrite (preselected, like TC) and Escape means Cancel. It shows both files' size and modification time.
- One operation at a time (the queue is v2).
- After a copy that finished without errors, the source panel's selection is cleared (like TC). Errors are listed in one error box at the end (the first 10, then "and N more").
- A move deletes each original only after it was copied; skipped or failed entries (and the directories holding them) stay behind.
- Queue of operations, like TC's F2 queue (v2).
- A directory watcher reloads a panel automatically when its directory changes outside the app (v1; was v2). Decided 2026-10-02 (design: `docs/superpowers/specs/2026-10-02-directory-watcher-design.md`):
  - Each panel watches its own folder, not subfolders. Reloads are throttled: 100 ms after the first change, then at most once per second while changes continue, and a final one at most 1 s after the last.
  - A watcher reload is quiet: no "Loading..." indicator, no dimming, and keys and dialogs keep working on the old listing. Cursor, selection and quick search stay.
  - It keeps running during F5/F6/F8, so files appear and vanish as the job runs (like TC).
  - If the watched folder disappears, the panel moves to the nearest existing parent (then home), cursor at the top. No error.
  - Network mounts get no fallback: only changes made from this machine are seen there; Ctrl-R covers the rest.

## Operation log

- Off by default; `log = true` in the config turns it on.
- One file per day, `operations-YYYY-MM-DD.log` (local date), in a `logs` folder next to the state file (`~/.local/state/yagni-commander/logs/` on Linux). Private to the user (mode 0600), never written through a symlink. Not in `/tmp`: shared with other users and emptied on reboot.
- One line per file or directory the app creates, copies, moves, renames, trashes or deletes, plus skips ("exists") and failures, with a timestamp. Copy/move/trash/delete also log a start line (with each selected source) and a finish line (failed and skipped counts). F2, F7 and Shift-F4 log one line each; Shift-F4 on an existing file logs nothing.
- At startup, log files older than `log_keep_days` (default 7) are deleted, even when logging is off. Only files named like log files are touched.
- Logging problems never stop an operation. A log that can't be opened at startup is reported in the status line and the app runs without it.

## Build order (v1)

1. ~~Adopt gpui-component; config and state storage.~~ Done (settings dialog: step 8).
2. ~~Selection model in core.~~ Done.
3. ~~F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R, quick search.~~ Done.
4. ~~Ctrl-. hidden files toggle.~~ Done.
5. ~~File-operation engine in core (copy, move, trash), with tests.~~ Done.
6. ~~F5/F6/F8 dialogs and progress.~~ Done.
7. ~~F3 viewer.~~ Done.
8. ~~Menu, About and the settings page.~~ Done.
