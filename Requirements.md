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

- Main menu, following the framework's and platform's conventions (the macOS menu bar; an in-window menu on Linux).
- v1 contents: Quit (Cmd-Q on macOS, Alt-F4 on Linux) and About (a dialog with name and version).

### Config

- Settings page in the UI:
  - Path to the external editor (used by F4).
  - File name sorting: case sensitive or insensitive.
- Changes take effect immediately, without restarting: from the settings page, and when the file is edited by hand (watch the file, or at least reload it on Ctrl-R). Today a restart is needed; fix before v1.
- Stored as a TOML config file in the platform config directory (e.g. `~/.config/yagni-commander/config.toml` on Linux, `~/Library/Application Support/yagni-commander/` on macOS). The settings page edits this file; editing it by hand also works.

### Window position and size

- Saved automatically on exit and restored on start.
- Kept in a state file separate from the config, so the config stays hand-editable.

### Themes

- v1: all colors come from a theme file; one built-in theme (Tokyo Night). No hardcoded colors in views.
- Theme selection in the config (`theme = "..."`), more built-in themes, and user themes in `~/.config/yagni-commander/themes/*.toml` (v2).
- Following the system light/dark appearance (v2).

### File list

- Directories show as `[name]` (display only: sorting and quick search use the bare name). ".." has no brackets.
- File icons in the lists (v2).

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
| Shift-F4 | Create a file and open it in the editor. Asks for a name (a single name, no `/`); if that file already exists it is opened as is. Without a configured editor, shows the error instead of asking. |
| F7 | Create a directory. Nested paths like `a/b/c` are allowed; nothing outside the current directory. |
| F8, Del | Move the selection, or the entry under the cursor, to the trash, after confirmation. |
| Alt-Z | Set the other panel's path to this panel's path. |
| Ctrl-U | Swap the two panels. |
| Ctrl-R | Reload both panels. |
| Ctrl-. | Toggle showing hidden files, in both panels (see Hidden files). |
| 0-9, a-z, A-Z | Quick search (see below). |
| Enter | On a directory: enter it. On a file: open it with its associated program (v2). |
| Alt-F5 | Archive/compress the selection or current entry, with confirmation if the target exists (v2). |
| Alt-F9 | Extract the selection or current entry, with confirmation if the target exists (v2). |
| Ctrl-D | Directory hotlist (v2). |
| Alt-F7 | Find files (v2). |

### Quick search

- Letters and digits (including non-ASCII letters like æ, ø) build a prefix. The cursor moves to the first entry whose name starts with it, case-insensitively.
- If no entry matches, the keystroke is ignored and the cursor stays where it is.
- No visible search box. The prefix resets after a short pause in typing (about one second) and on any other key or cursor movement.
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

- Read-only, UTF-8 text, in its own window.
- Must open huge files (GB range) instantly: memory-mapped, with lines indexed lazily as you scroll.
- Other encodings, hex view and search inside the viewer (v2).

## File operations

Applies to F5 (copy), F6 (move) and F8 (trash).

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
- In the conflict prompt, Enter means Skip (never overwrite by accident) and Escape means Cancel. It shows both files' size and modification time.
- One operation at a time (the queue is v2).
- After a copy that finished without errors, the source panel's selection is cleared (like TC). Errors are listed in one error box at the end (the first 10, then "and N more").
- A move deletes each original only after it was copied; skipped or failed entries (and the directories holding them) stay behind.
- Queue of operations, like TC's F2 queue (v2).
- A directory watcher reloads a panel automatically when its directory changes outside the app (v1; was v2).

## Build order (v1)

1. ~~Adopt gpui-component; config and state storage.~~ Done (settings page UI comes with the menu, step 8).
2. ~~Selection model in core.~~ Done.
3. ~~F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R, quick search.~~ Done.
4. ~~Ctrl-. hidden files toggle.~~ Done.
5. ~~File-operation engine in core (copy, move, trash), with tests.~~ Done.
6. ~~F5/F6/F8 dialogs and progress.~~ Done.
7. F3 viewer.
8. Menu, About and the settings page.
