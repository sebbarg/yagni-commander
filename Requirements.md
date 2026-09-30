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
- Stored as a TOML config file in the platform config directory (e.g. `~/.config/yagni-commander/config.toml` on Linux, `~/Library/Application Support/yagni-commander/` on macOS). The settings page edits this file; editing it by hand also works.

### Window position and size

- Saved automatically on exit and restored on start.
- Kept in a state file separate from the config, so the config stays hand-editable.

### Tabs (v2)

Tabs per panel, like Double Commander.

### Toolbar (v2)

Toolbar with e.g. drive icons.

## Keyboard

| Key | Action |
|---|---|
| Space | Toggle selection of the entry under the cursor. Selected entries use a different color. |
| Ctrl-A | Select all files and directories in the panel. |
| F2 | Rename the file or directory under the cursor. |
| F3 | Built-in viewer (see below). |
| F4 | Open the file or directory in the configured external editor. |
| F5 | Copy the selection, or the entry under the cursor if nothing is selected, to the other panel's directory (see File operations). |
| F6 | Move, with the same rules as F5. |
| F7 | Create a directory. |
| F8, Del | Move the selection, or the entry under the cursor, to the trash, after confirmation. |
| Alt-Z | Set the other panel's path to this panel's path. |
| Ctrl-U | Swap the two panels. |
| Ctrl-R | Reload both panels. |
| 0-9, a-z, A-Z | Quick search (see below). |
| Enter | On a directory: enter it. On a file: open it with its associated program (v2). |
| Alt-F5 | Archive/compress the selection or current entry, with confirmation if the target exists (v2). |
| Alt-F9 | Extract the selection or current entry, with confirmation if the target exists (v2). |
| Ctrl-D | Directory hotlist (v2). |
| Alt-F7 | Find files (v2). |

### Quick search

- Typed characters build a prefix. The cursor moves to the first entry whose name starts with it, case-insensitively.
- If no entry matches, the keystroke is ignored and the cursor stays where it is.
- No visible search box. The prefix resets after a short pause in typing (about one second) and on any other key or cursor movement.
- Fuzzy matching (v2, if ever).

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
- Queue of operations, like TC's F2 queue (v2).
- Automatic reload when files change outside the app (v2).

## Build order (v1)

1. ~~Adopt gpui-component; config and state storage.~~ Done (settings page UI comes with the menu, step 7).
2. Selection model in core.
3. F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R, quick search.
4. File-operation engine in core (copy, move, trash), with tests.
5. F5/F6/F8 dialogs and progress.
6. F3 viewer.
7. Menu, About and the settings page.
