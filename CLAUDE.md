# yagni-commander

A personal, cross-platform dual-pane file manager in the spirit of Total Commander (TC), written in Rust with gpui. It implements the ~20% of TC the owner actually uses, not a full clone.

**Requirements live in `Requirements.md`** (the working document: v1/v2 scope, keymap, product decisions, build order). Read it before planning features and keep it updated when decisions change.

## Status and handoff (update at the end of every session)

As of 2026-10-02, end of session (everything committed, last commit "Session handoff: next is the directory watcher"):

- **Done (v1 build order in `Requirements.md`):** steps 1 to 7: gpui-kit + gpui-component, config and window state; selection; F2, F4, F7, Alt-Z, Ctrl-U, Ctrl-R; Ctrl-. hidden files; the file-operation engine; F5/F6/F8 with progress; the F3 viewer. Step 8: the menu (TC-style command menus with shortcut labels and check marks; native bar on macOS, in-window bar on Linux opened by F10 or a lone Alt), About, and the Settings dialog (Ctrl-,; changes apply at once and are saved key by key with comments kept; Ctrl-R re-reads the config). Beyond the build order: background directory loading; both panel folders and the active panel restored on start; `[name]` directory display, Shift-F4 new file, quick search box (Up/Down step through matches), our own dialog `ButtonRow` (arrow keys; Enter presses the highlighted button), Shift-F8/Shift-Del permanent delete, the operation log, themes-as-data, modal error boxes. Plans with their reasoning: `docs/superpowers/plans/` (F3 viewer, background loading, menu, settings).
- **Tests:** `cargo test --workspace` (172 app, 251 core; app tests cover every key, mouse action and dialog) and `scripts/smoke.sh` (real app under Xvfb, 40+ checks on disk, screenshots in `target/smoke/`). Both green.
- **Next: the directory watcher (the owner chose it 2026-10-01).** Spec line in `Requirements.md`, File operations: "A directory watcher reloads a panel automatically when its directory changes outside the app". Not designed yet: brainstorm with the owner first. Questions to settle: debounce (a big copy into a watched folder fires thousands of events); keeping cursor and selection across an automatic reload (check what `Command::Reload` keeps today); network mounts (inotify sees no remote changes; fall back to nothing, or polling?); watch both panels only, non-recursive; what happens while a panel is loading or a job runs (the job already reloads both panels at its end). Leads from the source:
  - The `notify` crate 7.0.0 is already in the dependency tree (via gpui-component), so using it adds no new crate; check its API in `~/.cargo/registry/src/*/notify-7.0.0` before designing.
  - Reloads go through the core load state machine: `Command::Reload`, and a reload asked for while a read is pending marks the panel `stale` (see Architecture, `commander`). Watcher events should feed that, from the UI thread (`file_manager/loads.rs` shows the thread-plus-poll pattern).
  - Watcher threads must never block the UI and must not use gpui's background pool (same reason as directory reads).
- **After that, left before v1:** resizable/configurable columns (see Known issues); removing the F12 test dialog.
- **Deferred minors (from reviews, not fixed):** Ctrl-R doesn't retry a log that failed to start (`apply_config` compares with the in-memory config); Alt-F4 does nothing while a menu is open (matters on Hyprland, where the WM doesn't handle it); opening a menu with the mouse doesn't end the quick search; the F5/F6 many-files list line was checked by an app test only, never looked at on screen.
- **Later: right-click context menu.** Reuse `menus::popup` with its own `MenuEntry` list. Open decision: whether right-click selects, as in TC's default on Linux and Windows.
- **Owner decisions (2026-10-01):** F5/F6 on one entry edit its full target path, on several an editable folder plus a read-only list (see `Requirements.md`, File operations); missing folders are created. Text fields also take Ctrl-Ins/Shift-Del/Shift-Ins. Settings is a dialog (Ctrl-,), not a page; the editor field gets a "Browse..." button in v2.
- **Owner decisions (2026-09-30):** Enter = Delete in the Shift-F8 confirm (like TC); Enter = Overwrite in the conflict prompt (like TC); quick search wraps around; Shift-F4 field starts empty and accepts a relative path (`a/b/c.txt`, like F7).
- **Upstream bug, unreported by choice:** gpui-component `Dialog` binds Enter to OK for the whole dialog, ignoring the focused button (still on gpui-kit `main`; nearest related PR #3097). The owner decided not to file it for now; our `ButtonRow` works around it.
- **Temporary:** F12 opens a "Button test" dialog (`button_test` in `file_manager/commands.rs`). Remove before v1.
- **Also required before v1:** a directory watcher that reloads panels on outside changes; resizable/configurable columns (see Known issues).
- **Verified by the owner:** F8 trash on Kubuntu lands in `~/.local/share/Trash/files` (Dolphin's trash view needs a manual refresh to show it).
- **Waiting on the owner to verify on real machines:** `cargo test` on macOS (only ever run in the Linux container); `scripts/smoke.sh` on Kubuntu/Omarchy (needs the packages under Building); whether macOS still needs the full Xcode app with gpui-kit's runtime shaders; the "only Name sorting works" report; F4 with `editor = "code"` when launched from Finder (PATH); native Wayland (Plasma, Hyprland), never tested here; F5 byte progress on a real disk (the container clones files, so copies finish instantly); that a new F3 viewer window gets keyboard focus under a real window manager (KDE, Hyprland, macOS); the viewer's speed on a multi-GB file; lone Alt and Alt-Tab under KDE and Hyprland (Wayland reports a held Alt on refocus; Alt within 200 ms of activation is ignored for that reason); Alt+drag of a window on Plasma 5 (KWin grabs the click, so the app may open the menu on Alt release); the native macOS menu (shortcut labels, check marks, About and Quit in the app menu).

### How we work

- The owner reviews each step; commit only when asked (they say "commit"). Commits are local; the owner pushes (remote: `github.com/sebbarg/yagni-commander`).
- Every change: tests (core near 100% coverage; gpui app tests for every key, mouse action and dialog), `cargo clippy --workspace --all-targets` with zero warnings, `cargo fmt`, then `scripts/smoke.sh` and a look at its screenshots. Extend the smoke script when a feature is visible or touches the OS (trash, editor, state file).
- Update `Requirements.md` when behavior decisions are made, and this file's Status section when a step completes.
- User-facing text and docs: no em dashes.

## Goals

- Looks great and feels at home on macOS, Kubuntu (KDE Plasma) and Omarchy (Hyprland). Existing options fall short: Double Commander's macOS support is poor, Krusader lacks features.
- Keyboard-first, TC-style workflow with fully configurable keybindings.
- Fast (large directories, instant navigation) and secure (small attack surface, no `unsafe`).

## Decisions

- **GUI, not TUI.** Terminals intercept keys (Cmd shortcuts, Ctrl+Tab, etc.) inconsistently, so a TC keymap can't be guaranteed. A GUI also gives real multiple windows, drag and drop with Finder/Dolphin, and launching a configurable external editor (e.g. VS Code on F4).
- **gpui (Zed's UI framework), not iced.** Its action + context-scoped keymap system matches the TC model, `uniform_list` is virtualized with native smooth scrolling, text truncates with ellipsis, and redraw on resize is smooth. iced 0.14 was evaluated and rejected.
- **gpui comes via gpui-kit** (Longbridge, crates.io `gpui-kit = "0.7"`). gpui-kit pins `gpui-pre 0.3.7` (a snapshot of Zed commit `1a28cff`), re-exports gpui, provides `application()`, `init()` and `open_window()`, and includes gpui-component. Import everything through `gpui_kit::` (gpui APIs) and `gpui_kit::component::` (widgets); never add `gpui` directly, since two gpui crates in one build are incompatible types. The crates.io `gpui` itself (0.2.2, Oct 2025) is stale. Trade-off accepted: the snapshot is republished by a third party, not Zed, and the dependency tree is large (~700 crates). Its docs are thin: read the source under `~/.cargo/registry/src/*/gpui-component-0.7.0`, `gpui-base-0.7.0`, `gpui-pre-0.3.7`.
- **gpui-component** (via gpui-kit) provides inputs, dialogs, menus, the settings component and progress, which gpui lacks.
- **Themes are data.** A theme is a TOML file (`crates/yagni-commander/assets/themes/`) with `name`, `mode` (dark/light) and ~19 colors named by role (`accent`, `selected`, `directory`, ...), never by hue. Every background role has a matching text role (`accent`/`text_on_accent`). gpui-component's theme is derived from these roles in code (`Theme::component_config`), including hover/pressed shades, so theme authors only define roles and we don't depend on gpui-component's theme format. Theme selection and user theme files are v2 (see `Requirements.md`).
- **Errors are modal.** Errors show in a centered message box that stays until dismissed (`show_error` in `file_manager/commands.rs`); never toast notifications for errors.
- **Same keymap on all platforms**, Ctrl not Cmd on macOS (only Quit differs: Cmd-Q / Alt-F4). F-keys assume Fn is held on Mac keyboards.

## Architecture

- `crates/yagni-commander-core`: UI-agnostic core, no gpui. Modules: `commander` (dual-panel state, `Command`s, rename/mkdir/quick-search entry points; a load state machine: navigation and reload commands queue a `LoadRequest` and keep the old listing, the UI takes them with `take_requests` and returns results through `finish_load`; ids drop stale results (Escape = `cancel_load`, newer navigation); commands for a loading panel are ignored, except that a click focuses it; a reload asked for while a read is pending marks the panel `stale` and runs once that read ends, cancelled or failed or not), `listing` (`read_listing`, with the startup fallback to the nearest readable parent, then home), `panel` (listing, cursor, sort, selection, `Summary`), `entry` (reading directories), `sort`, `format` (size, time, permissions text), `fs_ops` (rename, no-replace rename, make directory, name validation), `file_ops` (copy/move/trash/delete engine `run`, background `Job`; delete walks with `openat`/`unlinkat` and `O_NOFOLLOW` so a directory swapped for a symlink mid-delete can't redirect it), `launch` (external editor), `oplog` (operation log: day files, startup pruning; the engine writes through `file_ops::Settings::log`, F2/F7/Shift-F4 through `Commander::set_log`), `quick_search` (the typed prefix), `viewer` (F3: `source` opens regular files only, with `O_NONBLOCK` so a FIFO can't block; `layout` turns a byte window into display rows; `document` caches 64 KiB blocks and navigates by byte position; `lines` counts lines in the background, keeping a count per MiB), `config`, `storage` (platform paths, TOML load/save, atomic writes).
- `crates/yagni-commander`: the gpui app.
  - `Commander` lives in a gpui `Entity`, shared by the root `FileManager` view and two `PanelView` entities. Mutations go through `execute()` in `file_manager.rs`, which calls `cx.notify()`; views react via `observe`.
  - Keys map to gpui actions (`actions.rs`), actions map to core `Command`s or `FileManager` methods. The keymap is data, so a user config file can plug in there.
  - `file_manager/file_ops.rs`: F5/F6/F8 around a core `file_ops::Job`. A `spawn_in` timer polls the job every 50 ms; the progress dialog (a `ProgressView` entity inside a gpui-component `Dialog`) opens after 6 polls or on the first conflict; the conflict dialog stacks on top of it. `finish_job` closes the progress dialog, reloads both panels and shows failures.
  - `button_row.rs`: the button row used by every dialog (see Gotchas).
  - `menus.rs`: the menus as data (`MenuDef`, `MenuEntry`), built by `menus(MenuState, mac)`; `to_gpui` feeds `cx.set_menus` (native on macOS), `popup` builds a gpui-component `PopupMenu` (the Linux bar now, a right-click menu later). Items dispatch the same actions as the keys, to `FileManager`'s focus. `FileManager::update_menus` (from its `Commander` observer) rebuilds them when the check marks (hidden files, the active panel's sort column) change.
  - `menu_bar.rs`: the Linux in-window bar (`MenuBar`). Not gpui-component's `AppMenuBar`: that one can't be opened from the keyboard (private methods). F10 (`ToggleMenu`) and a lone Alt (`MenuAlt`) open it; Left/Right arrive as gpui-base's `SelectLeft`/`SelectRight` propagated from the popup.
  - `file_manager/loads.rs`: runs the commander's directory reads, one `std::thread` per read (never gpui's background pool: a dead NFS read would hold a pool thread forever), polled every 10 ms while any runs; after 15 polls the panel header shows "Loading <path>... N entries" and the old listing dims; it redraws only when the count changed (checked every 100 ms), so a hung read stays quiet. The `FileManager` observer of the `Commander` starts new reads; dialogs and file operations check `active_loading` first (F5/F6 also wait for the other panel, their destination).
  - `viewer_view.rs`: the F3 viewer window (`ViewerView`, its own `Viewer` key context and `actions::viewer` actions, byte-based scrollbar). Rows are laid out around the top byte position on every render; nothing depends on the file size. Giant lines get forced row breaks at 64 KiB-aligned offsets (when the 64 KiB before has no `\n`), so scrolling up never scans far back.
  - `windows.rs`: closing the main window closes every viewer and quits.
  - `config_state.rs`: the `CurrentConfig` global: the settings in use, the file's path, and `problem` (why the file is not in use: parse error or no config directory; while set, the settings dialog is disabled and nothing is saved). `FileManager::change_setting` (dialog) saves one key with core `config::save_setting` (`toml_edit`: comments, layout and hand edits survive; a broken file is never written) and then `apply_config` (sort, log, the global). Ctrl-R calls `reload_config`, which applies a good file or keeps the settings and shows "Config ignored: ...".
  - `settings_dialog.rs`: the Settings dialog (`SettingsView`: two `text_field`s and two gpui-component `Switch`es, plus a `ButtonRow` with Close). A text field is saved only if typed into (so its opening text can't undo a hand edit of the file), on blur and on every way out; Enter and Close stay in the dialog while the days value is invalid, Escape drops it. Save errors show after the dialog has closed (`window.defer`), since closing pops the top dialog.
  - `file_manager/commands.rs`: F2/F7/Shift-F4 name prompt (`prompt_name`, a gpui-component `Dialog` with an `Input`), F4 and Shift-F4 editor launch, quick search key handling, `show_error` / `show_message`, About.
  - The active `Theme` is a gpui `Global` (`theme.rs`); `Theme::install` also applies it to gpui-component. Built-in default: Tokyo Night (Omarchy's default).
  - Views take every color from `Theme::get(cx).colors`. `clippy.toml` bans gpui's color constructors (`rgb`, `hsla`, `black`, ...) so literals can't creep in; add a new role to `Colors` and every theme file instead.
  - The loaded `Config` is a gpui global, `CurrentConfig` (`main.rs`).
  - The window is opened with `gpui_kit::open_window`, which wraps `FileManager` in gpui-kit's `Root` (needed for dialogs).
  - State file: `app_state.rs` keeps an `AppState` global (window geometry, updated via `observe_window_bounds`; both panel folders and the active side, updated by `FileManager`'s observer of the `Commander`; `show_hidden`; the last viewer geometry) and saves it on quit. It is loaded before the window opens, since `Commander::start` needs `show_hidden` and the folders (`State::startup_dirs`: arguments, else saved folders, else home; nothing is read there, the background read falls back). Window restore falls back to centered if the geometry no longer overlaps a display.
  - Hidden entries: `Panel` keeps them in a separate unsorted list while hidden, so `entries()` and every index (cursor, quick search, summary, selection) only see visible ones.
  - Column layout and cell text live in `columns.rs`; panel rendering in `panel_view.rs`.
- Settings: `Config` is a hand-editable TOML file, created from a commented template on first start, edited by the Settings dialog one key at a time. A file that fails to parse is never overwritten; the app shows the error in the status line and runs on defaults (at startup) or keeps its settings (Ctrl-R).
  - Config: `~/.config/yagni-commander/config.toml` (Linux), `~/Library/Application Support/yagni-commander/config.toml` (macOS). Keys: `editor`, `case_sensitive_sort`, `log`, `log_keep_days`.
  - State: `~/.local/state/yagni-commander/state.toml` (Linux), same folder as config on macOS.
  - Operation log (when `log = true`): `logs/operations-YYYY-MM-DD.log` next to the state file (see `Requirements.md`, Operation log).
- `unsafe_code` is forbidden workspace-wide.

## Current features

- Two panels side by side, draggable divider, resizable window; position and size, both panel folders and the active panel restored on start (a deleted folder falls back to its nearest readable parent, then home; command-line folders override).
- Tab switches panels; Up/Down/Home/End/PageUp/PageDown move the cursor; Enter enters a directory or goes up on ".."; Backspace goes up. Going up leaves the cursor on the directory you came from.
- Selection: Space toggles the entry under the cursor and moves down; Ctrl-A selects all. Selected entries are orange (the cursor bar turns orange on a selected entry). The footer shows totals, or "N of M selected, size of total". Selection is per panel, kept by name across re-sorts, cleared on directory change. `Panel::targets()` (selection, else the cursor entry, never "..") is what file operations will act on.
- F5 copy, F6 move (wide destination prompt: one entry gets its full target path with the name preselected, so it can be renamed or sent anywhere, missing folders created; several entries get an editable target folder and a read-only list of names; core `file_ops::Destination::{Into, As}`), F8/Del trash (confirm), Shift-F8/Shift-Del permanent delete (confirm), in the background with a progress dialog (after ~300 ms), Cancel, a per-file conflict prompt and an error summary; both panels reload at the end.
- F2 rename (name preselected up to the last extension; never overwrites; case-only rename allowed), F7 new directory (nested `a/b/c` allowed, nothing outside the current directory), F4 opens the entry (or the directory on "..") in `editor`; Shift-F4 asks for a file name or relative path (`a/b/c.txt`, missing folders created), creates the file (or keeps an existing one), puts the cursor on it (or its first folder) and opens it in `editor`. Alt-Z shows this directory in the other panel, Ctrl-U swaps panels, Ctrl-R reloads both. Typing opens a quick search box in the panel footer and jumps to the first name starting with the typed text; Down/Up step through matches (wrapping), Backspace shortens it, Escape or any other command closes it. The search state lives in `Commander` (`search_*`), so every command ends it.
- Mouse: click moves the cursor (and focuses that panel), double-click activates, wheel scrolls the view without moving the cursor.
- F3 views the file under the cursor in a new window (directories and ".." do nothing; FIFOs and devices show an error). Opens GB-sized files instantly. Word wrap by default, `W` toggles no-wrap (Left/Right scroll, rows cut at 10,000 characters); Up/Down/PageUp/PageDown/Home/End (Ctrl too), mouse wheel, draggable byte-based scrollbar; status line with size, percentage, mode and "line N of M" once the background count finishes. Escape or `q` close it. Invalid UTF-8 and control characters show as a dim `·`.
- Directory names show as `[name]` (display only; ".." unbracketed).
- Columns: Name, Size, Modified (local time), Owner (`user:group`), Permissions (`ls -l` style). Clicking a header sorts that panel by it; clicking again reverses. Size and Modified start descending. ".." then directories always come first. `case_sensitive_sort` in the config switches name comparison.
- Hidden files (name starts with `.`) are hidden by default; Ctrl-. toggles them in both panels, remembered in the state file. Shown hidden names use the `hidden` theme role. Hiding deselects them and moves the cursor off them.
- Optional operation log (`log = true`): every file created, copied, moved, renamed, trashed or deleted, one file per day, pruned after `log_keep_days`.
- Symlinks: Owner and Permissions describe the link itself; Size and Modified come from the target.
- Menu: Files, Commands, Show (hidden files, sort column, with check marks), Help (About); each item shows its key. macOS: the native menu bar, About and Quit in the app menu. Linux: an always-visible bar above the panels; F10 or a lone Alt opens it, arrows, Enter and Escape inside; other keys are ignored while it is open. Sorting is also available as actions (`SortBy*`), unbound by default.
- Text fields (dialogs): Ctrl-C/X/V plus Ctrl-Ins, Shift-Del, Shift-Ins (bound in gpui-base's `Input` context in `actions.rs`; Shift-Del in the panels is still permanent delete).
- Settings (Ctrl-, or the menu): editor, case-sensitive sorting, operation log on/off, days to keep logs. Changes apply at once and are saved to the config file without losing its comments. Ctrl-R also re-reads the config.
- Quit: Cmd+Q (macOS), Alt+F4 (Linux), the menu, or closing the last window.

## Known issues and gaps

- At narrow panel widths the fixed columns (~400 px) squeeze the Name column to nothing. Needs resizable or configurable columns.
- The owner reported that only Name sorting seemed to work on their machine; Size and Modified sort correctly in headless tests. Unverified. Note that owner/permissions sorts look like name sorts in directories where those values are all equal.
- Reading a big folder takes ~320 ms per 100k entries on Linux (~70% of it one `stat` per entry), near-instant on macOS. It runs in the background now; making the `stat` cheaper would only shorten the wait.
- A folder given on the command line is checked synchronously before the window opens, so one on a dead mount still hangs startup (accepted: the user typed it).
- Enter on a file does nothing yet (v2: open with associated program).
- No-replace rename (F2, move) is atomic only on Linux glibc (`renameat2`). On macOS it checks, then renames, so another process could create the target in between (TOCTOU); fixing it needs `renamex_np`, which nix doesn't offer without our own `unsafe`.
- A config problem shows in the status line and clears on the next keyboard command, not on mouse clicks.
- `PanelView::visible_rows` reads gpui scroll-handle internals (public fields); likely to break on a gpui upgrade.
- gpui cannot write file paths to the clipboard (copy in app, paste in Finder/Dolphin), and drag-out to other apps works on macOS and Wayland only (not X11). Both need platform code or another crate.

## Gotchas

- App tests read folders inline (`open` sets `FileManager::load = None`), so keys take effect at once. To keep a panel loading, `use_held_loads` plus a `hold` file in the folder being read; drive it with `wait_until`. Core tests finish reads with `run_loads_now` (or the `run` helper).
- Tests must never call the real `trash::delete` (it would fill the owner's trash): core `file_ops` tests pass `file_ops::Settings { trash: fake_trash, .. }` to `run` / `Job::spawn`, and app tests set `FileManager::trash` to a fake (`use_fake_trash`). Only `scripts/smoke.sh` uses the real trash, with `XDG_DATA_HOME` in a temporary folder.
- App tests of F5/F6 run a real worker thread: drive them with `advance_clock` + `run_until_parked` in a loop (`wait_until` in `file_manager/tests.rs`); the poll timer only fires on the test clock.

- gpui-component `Dialog` maps Enter to its OK action (`on_ok`) for the whole dialog, even when a text input or another button has focus (upstream bug, not reported as of 2026-09-30), and has no arrow-key navigation. So every dialog's buttons are our `ButtonRow` (`button_row.rs`), never `DialogFooter`/`DialogAction`/`open_alert_dialog`: it keeps its own selected button and binds Left/Right, Tab/Shift-Tab, Enter and Space in its own key context. Focus the row when a dialog opens (`focus_when_open`), except prompts, which focus their text field (Enter there still goes to `on_ok`; don't also handle the input's `PressEnter`, or OK runs twice: `enter_submits_a_prompt_exactly_once`).
- Opening a dialog moves focus to it: focus a field inside it with `window.defer` after opening. After an error box closes, focus must be put back explicitly (`show_error`'s `refocus`).
- An event handler running inside an entity's update must not update that same entity again (panics "already being updated"); defer with `window.defer`.
- Tests of the settings dialog: point `CurrentConfig.path` (helper `use_config`) and `FileManager::log_dir` (`use_log_dir`) at temporary folders, never the real ones. Call `activate(cx)` first (blur events need an active window), and use `press(cx, key)` for Space/Enter on a switch or button (gpui clicks on key-up; `simulate_keystrokes` sends only key-down).
- Text fields: use `commands::text_field`, never a bare `Input::new`. gpui-component's default input (32 px, 8 px vertical padding) leaves 14 px for a 20 px line and cuts off descenders; `text_field` sets 5 px.
- `ThemeConfigColors` (gpui-component) has private fields: build it from `default()` and assign fields, not with struct-update syntax.
- Viewer windows in app tests: find them with `viewer_view::tests::viewer_in` over `cx.windows()` and drive them with `VisualTestContext::from_window`. In the smoke script (no window manager) the new viewer window must be focused with `xdotool windowfocus`.
- Menus show an action's highest-precedence binding, which is the one registered last: register each action's primary key last (F8 after Del). Quit has one binding per platform for the same reason.
- Lone Alt is gpui's modifier-only binding `"alt"` (fires on release if no key came in between). gpui doesn't count mouse clicks or window switches, so `FileManager::alt_armed` is cleared by those. A keystroke interceptor (`intercept_keystrokes` in `FileManager::new`) swallows every key but the menu's own while a menu is open, because the panel bindings are on the open popup's dispatch path. gpui test windows start inactive, and gpui reports focus changes (`on_focus_out`) only for the active window: call `window.activate_window()` first in such tests. An open menu closes when focus leaves its popup (e.g. a dialog opens). Lone-Alt tests that follow an activation advance the test clock past `ALT_AFTER_ACTIVATION`. The menu-bar app tests are in `mod menu_bar` in `file_manager/tests.rs`, compiled on Linux only.
- Icons (e.g. menu check marks) need gpui-kit's assets: `main.rs` registers `gpui_kit::assets::Assets`; without it icons render blank.
- Alt-F4 is bound to Quit (Linux); the `Viewer` context rebinds it to close only that viewer.
- macOS (unverified): apps started from Finder get a minimal `PATH`, so an `editor` like `code` may not be found; use a full path in the config if so.

## Building

- Rust via rustup (latest stable).
- macOS: Xcode. gpui-kit enables gpui's `runtime_shaders` and `font-kit` features, so Metal shaders compile at runtime and the full Xcode app may no longer be required (unverified; if the build asks for `metal`, install Xcode and `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`).
- Kubuntu: `sudo apt install build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`
- Arch/Omarchy: `sudo pacman -S --needed base-devel clang cmake libxkbcommon libxkbcommon-x11 wayland fontconfig freetype2 libx11 libxcb vulkan-icd-loader` plus the Vulkan driver for your GPU.
- For `scripts/smoke.sh` (Linux only; package names not yet verified on the owner's machines): Kubuntu `sudo apt install xvfb xdotool imagemagick mesa-vulkan-drivers`; Arch `sudo pacman -S xorg-server-xvfb xdotool imagemagick vulkan-swrast`.

### Fresh dev container (Ubuntu 24.04, no display)

Containers are ephemeral; redo this at the start of a session if `~/.cargo/bin/cargo` is missing.

```sh
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt
sudo apt-get install -y build-essential pkg-config clang cmake libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig-dev libfreetype-dev libx11-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
  mesa-vulkan-drivers xvfb xdotool imagemagick
~/.cargo/bin/rustup component add llvm-tools-preview && ~/.cargo/bin/cargo install cargo-llvm-cov --locked
```

Cargo is at `~/.cargo/bin/cargo` (not on PATH in non-login shells). A clean build takes a few minutes.

## Commands

- `cargo run --release -- [left-dir] [right-dir]`: run the app (the default workspace member). Without arguments it reopens the folders from the last run.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets` (must be warning-free).
- `cargo llvm-cov --workspace --summary-only`: coverage. Core is kept near 100%; the uncovered lines need root or a file owned by an unknown uid.
- App tests live in `file_manager/tests.rs`. Mouse tests find elements by `debug_selector` (`row-left-3`, `header-right-Size`, `divider`, `search-left`) and `cx.debug_bounds`; a missing selector also means the element was not drawn. Jobs that must stay running use `use_fake_trash` plus a `hold` file.
- App tests use gpui's `TestAppContext` (`#[gpui_kit::test]`) and build the window like the app does (`gpui_kit::init` + `Root`), so dialogs work. Drive them with `simulate_keystrokes` / `simulate_input` and `run_until_parked`; check dialogs with `window.has_active_dialog(cx)`. gpui's test window does not expose the title, so title text lives in a pure function.
- `scripts/smoke.sh [screenshot-dir]`: runs the real debug build under Xvfb through the main features (quick search, F7, Shift-F4, F2, F5 with a conflict, F6, F8 into a temporary trash, Shift-Del, F12, Ctrl-., F3 on a 200,000-line file and a 300 KB single line, quit, restart without arguments onto the saved folders), checks the results on disk (including the operation log and its startup pruning) and saves screenshots to `target/smoke/`. Config, state and trash are in a temporary folder. Linux only; needs Xvfb, xdotool, ImageMagick.
- `scripts/make-test-files.sh [count] [dir]`: create a large directory under `/tmp` for stress tests.
- `RUST_LOG=info` shows gpui's platform logs (renderer, fonts, windowing).

### Headless run (to see the real app in the container)

Xvfb gives a virtual X11 display, Mesa's llvmpipe renders Vulkan on the CPU, xdotool sends input, ImageMagick screenshots. Scratch `XDG_*` dirs keep config/state away from real files.

```sh
S=<scratchpad>; mkdir -p $S/xdg $S/cfg $S/state; chmod 700 $S/xdg
export DISPLAY=:99 XDG_RUNTIME_DIR=$S/xdg XDG_CONFIG_HOME=$S/cfg XDG_STATE_HOME=$S/state
(Xvfb :99 -screen 0 1280x860x24 >/dev/null 2>&1 &); sleep 1
(target/debug/yagni-commander /usr /etc >$S/app.log 2>&1 &); sleep 4
W=$(xdotool search --name yagni-commander | head -1); xdotool windowfocus $W; sleep 0.5
xdotool key Down F2; xdotool type --delay 40 "name"; xdotool key Return
import -window root $S/shot.png        # then read the PNG
xdotool key alt+F4; pkill -x Xvfb
```

- Pause (`sleep 0.3`+) after focusing, moving or resizing before sending keys; gpui also ignores a click sent in the same instant as the pointer move.
- Never `pkill -f` a pattern that matches your own shell command line; use `pkill -x yagni-commander`.
- X11 only, no window manager: Wayland, tiling (Hyprland) and macOS are untested here.
