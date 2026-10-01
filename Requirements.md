# Requirements

Working document for what yagni-commander should do. Items are v1 unless marked (v2).

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
  - **Files:** View (F3), Edit (F4), New file (Shift-F4), Copy (F5), Move (F6), New folder (F7), Rename (F2), Move to trash (F8), Delete permanently (Shift-F8); Select all (Ctrl-A); Quit (Linux only, Alt-F4).
  - **Commands:** Same folder in other panel (Alt-Z), Swap panels (Ctrl-U), Reload (Ctrl-R).
  - **Show:** Hidden files (Ctrl-.), checked while shown; Sort by Name, Size, Modified, Owner, Permissions, with a check on the active panel's sort column. Choosing the checked column reverses the order, like clicking the header. The sort commands are actions, so the keymap can bind them; unbound by default.
  - **Help (Linux):** About. On macOS, About and Quit sit in the app menu (platform convention) and there is no Help menu.
  - Settings gets an entry with the settings page. The temporary F12 Button test is not in the menu.
- Check marks follow the state: the menus are rebuilt when hidden files or the active panel's sort change.
- Menu items act on the active panel, exactly like their keys (same rules while a panel is loading or a dialog is open).
- Linux keyboard: F10 or a lone Alt (pressed and released with no other key, button or modifier in between) opens the first menu; Up/Down and Enter inside, Left/Right between menus; Escape, F10 or a lone Alt closes it and the panel gets focus back. Alt-Z, Alt-F4 and Alt-Tab never open it (a key, mouse button or window deactivation between press and release cancels). Not while a dialog is open. On macOS F10 and lone Alt do nothing.
- About: a dialog with the name and version (`CARGO_PKG_VERSION`) and an OK button.
- Right-click context menu (later): it reuses the menu's item model and popup builder with its own item list. Open decision: whether right-click selects, as in TC's default on Linux and Windows.

### Config

- Settings page in the UI:
  - Path to the external editor (used by F4).
  - File name sorting: case sensitive or insensitive.
- Changes made in the settings page take effect immediately. Hand edits of the file take effect on restart or Ctrl-R (which also re-reads the config); no file watcher (decided 2026-10-01: with a settings page, hand edits are rare). Both come with the settings page (step 8).
- Stored as a TOML config file in the platform config directory (e.g. `~/.config/yagni-commander/config.toml` on Linux, `~/Library/Application Support/yagni-commander/` on macOS). The settings page edits this file; editing it by hand also works.

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

- Directories show as `[name]` (display only: sorting and quick search use the bare name). ".." has no brackets.
- File icons in the lists (v2).

### Directory loading

- Directories are read off the UI thread, so a huge directory or a slow or hung network mount never freezes the app (decided 2026-10-01; the owner uses network mounts).
- While a panel loads, it keeps showing its previous listing. A load that finishes within about 150 ms switches without any indicator. After that the panel header shows "Loading <path>... N entries" (a live count) and the old listing dims.
- While loading, that panel accepts only Escape (cancel: stay on the old listing; at startup: go to the home folder) and Tab. Dialogs and file operations on the loading panel are ignored. Quit always works, and the other panel works normally.
- A failed load leaves the panel where it was, with the error in the status line.
- Startup loads in the background too, including the fallback for a missing remembered folder (nearest readable parent, then home). Folders given on the command line are checked before the window opens; one that is not a directory ends the app with an error.
- A newer navigation replaces a pending one. A reload requested while a navigation is pending is dropped (the navigation brings a fresh listing anyway).
- A read the operating system never returns (a dead NFS server) cannot be interrupted: Escape abandons it, and its thread ends when the read returns or the app exits.
- Showing entries while a slow directory is still being read (a growing listing) is out of scope.

### Tabs (v2)

Tabs per panel, like Double Commander.

### Toolbar (v2)

Toolbar with e.g. drive icons.

### Error messages

- Errors are shown in a centered, modal message box with a Dismiss button. It stays until dismissed (button, Enter or Escape). No fading toasts for errors.
- Dialog buttons: Left/Right or Tab/Shift-Tab move between buttons, Enter or Space presses the highlighted one, Escape cancels. The default button is highlighted when the dialog opens. In a prompt, the arrow keys edit the text and Tab moves to the buttons.
- After dismissing an error from a prompt (e.g. rename), focus returns to the prompt's text field so the input can be corrected.

## Keyboard

| Key | Action |
|---|---|
| Space | Toggle selection of the entry under the cursor and move the cursor down. Selected entries are orange. ".." can't be selected. Changing directory clears the selection. |
| Ctrl-A | Select all files and directories in the panel. |
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
| Ctrl-U | Swap the two panels. |
| Ctrl-R | Reload both panels. |
| Ctrl-. | Toggle showing hidden files, in both panels (see Hidden files). |
| F10, Alt (alone) | Open the menu (Linux; see Menu). |
| Letters, digits, `.` and other printable keys | Quick search (see below). |
| Enter | On a directory: enter it. On a file: open it with its associated program (v2). |
| Alt-F5 | Archive/compress the selection or current entry, with confirmation if the target exists (v2). |
| Alt-F9 | Extract the selection or current entry, with confirmation if the target exists (v2). |
| Ctrl-D | Directory hotlist (v2). |
| Alt-F7 | Find files (v2). |

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
- F5/F6 ask for the destination directory, prefilled with the other panel's path. A relative path is taken from the active panel's directory. It must be an existing directory other than the source directory (the prompt says so and stays open).
- F8 and Del ask for confirmation first.
- The progress dialog appears only if the job takes longer than about 300 ms, so quick copies don't flash a dialog. It counts files and bytes. Escape or Cancel stops the job at the next file (or 4 MiB chunk).
- In the conflict prompt, Enter means Overwrite (preselected, like TC) and Escape means Cancel. It shows both files' size and modification time.
- One operation at a time (the queue is v2).
- After a copy that finished without errors, the source panel's selection is cleared (like TC). Errors are listed in one error box at the end (the first 10, then "and N more").
- A move deletes each original only after it was copied; skipped or failed entries (and the directories holding them) stay behind.
- Queue of operations, like TC's F2 queue (v2).
- A directory watcher reloads a panel automatically when its directory changes outside the app (v1; was v2).

## Operation log

- Off by default; `log = true` in the config turns it on.
- One file per day, `operations-YYYY-MM-DD.log` (local date), in a `logs` folder next to the state file (`~/.local/state/yagni-commander/logs/` on Linux). Private to the user (mode 0600), never written through a symlink. Not in `/tmp`: shared with other users and emptied on reboot.
- One line per file or directory the app creates, copies, moves, renames, trashes or deletes, plus skips ("exists") and failures, with a timestamp. Copy/move/trash/delete also log a start line (with each selected source) and a finish line (failed and skipped counts). F2, F7 and Shift-F4 log one line each; Shift-F4 on an existing file logs nothing.
- At startup, log files older than `log_keep_days` (default 7) are deleted, even when logging is off. Only files named like log files are touched.
- Logging problems never stop an operation. A log that can't be opened at startup is reported in the status line and the app runs without it.

## Build order (v1)

1. ~~Adopt gpui-component; config and state storage.~~ Done (settings page UI comes with the menu, step 8).
2. ~~Selection model in core.~~ Done.
3. ~~F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R, quick search.~~ Done.
4. ~~Ctrl-. hidden files toggle.~~ Done.
5. ~~File-operation engine in core (copy, move, trash), with tests.~~ Done.
6. ~~F5/F6/F8 dialogs and progress.~~ Done.
7. ~~F3 viewer.~~ Done.
8. Menu, About and the settings page. (Menu and About done; the settings page is next.)
