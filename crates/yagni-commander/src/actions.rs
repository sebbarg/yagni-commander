//! Actions and the default keymap. The keymap is data, so a user config file
//! can later add or override bindings without touching the views.

use gpui_kit::component::input;
use gpui_kit::{App, KeyBinding, actions};

/// Key context of the file manager's root view; scopes the bindings below.
pub const FILE_MANAGER_CONTEXT: &str = "FileManager";
/// Key context of gpui-base's text fields.
const INPUT_CONTEXT: &str = "Input";
/// Key context of a dialog's [`crate::button_row::ButtonRow`].
pub const BUTTON_ROW_CONTEXT: &str = "ButtonRow";

actions!(
    yagni_commander,
    [
        SwitchPanel,
        CursorUp,
        CursorDown,
        CursorHome,
        CursorEnd,
        PageUp,
        PageDown,
        Activate,
        GoUp,
        ToggleSelection,
        SelectAll,
        Rename,
        Edit,
        EditNewFile,
        CopyPath,
        MakeDirectory,
        Copy,
        Move,
        Trash,
        Delete,
        Pack,
        Extract,
        /// Files menu only; no key by default.
        CompareContents,
        /// Alt-Enter: the Properties box.
        ShowProperties,
        /// Alt-F7: the find dialog.
        FindFiles,
        /// Ctrl-Shift-T: a terminal in the active panel's folder.
        OpenTerminal,
        SyncOtherPanel,
        SwapPanels,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        Reload,
        ToggleHidden,
        CancelSearch,
        View,
        Quit,
        SortByName,
        SortBySize,
        SortByModified,
        SortByOwner,
        SortByPermissions,
        About,
        ToggleMenu,
        MenuAlt,
        OpenFilesMenu,
        OpenSettings,
        DirectoryHotlist
    ]
);

/// Key context of a viewer window.
pub const VIEWER_CONTEXT: &str = "Viewer";

/// Viewer actions; a module of their own because some names repeat the file
/// manager's.
pub mod viewer {
    gpui_kit::actions!(
        viewer,
        [
            Close,
            LineUp,
            LineDown,
            PageUp,
            PageDown,
            Start,
            End,
            ToggleWrap,
            ScrollLeft,
            ScrollRight,
            Find,
            FindNext,
            FindPrevious
        ]
    );
}

/// Key context of the directory hotlist popup (Ctrl-D).
pub const HOTLIST_CONTEXT: &str = "Hotlist";

/// Hotlist popup actions; a module of their own because the names repeat.
pub mod hotlist {
    gpui_kit::actions!(hotlist, [Up, Down, Pick, Close]);
}

/// Key context of the hotlist Configure dialog's list.
pub const HOTLIST_LIST_CONTEXT: &str = "HotlistList";

pub mod hotlist_list {
    gpui_kit::actions!(hotlist_list, [Up, Down, Remove, MoveUp, MoveDown]);
}

/// Key context of the find dialog's results list.
pub const FIND_RESULTS_CONTEXT: &str = "FindResults";

/// Key context of the whole find dialog: its view and its footer.
pub const FIND_DIALOG_CONTEXT: &str = "FindDialog";

/// Key context of the find dialog's "Skip folders" control, and of its
/// popup.
pub const FIND_SKIP_BUTTON_CONTEXT: &str = "FindSkipButton";
pub const FIND_SKIP_CONTEXT: &str = "FindSkip";

/// Key context of the find dialog's option boxes.
pub const FIND_OPTION_CONTEXT: &str = "FindOption";

pub mod find_results {
    gpui_kit::actions!(
        find_results,
        [
            Up, Down, PageUp, PageDown, Home, End, Open, View, Toggle, Feed, SkipOpen, SkipUp,
            SkipDown, SkipToggle, SkipClose
        ]
    );
}

actions!(
    button_row,
    [
        PrevButton,
        NextButton,
        TabNext,
        TabPrev,
        PressButton,
        CopyText
    ]
);

pub fn bind_default_keys(cx: &mut App) {
    let context = Some(FILE_MANAGER_CONTEXT);
    // gpui-component's menus (Linux) show an action's last-registered
    // binding, the native macOS menu its first. So an action with a second
    // key has its primary key registered both before and after it.
    let quit = if cfg!(target_os = "macos") {
        "cmd-q"
    } else {
        "alt-f4"
    };
    let copy_path = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    cx.bind_keys([
        KeyBinding::new(quit, Quit, None),
        KeyBinding::new("tab", SwitchPanel, context),
        KeyBinding::new("shift-tab", SwitchPanel, context),
        KeyBinding::new("up", CursorUp, context),
        KeyBinding::new("down", CursorDown, context),
        KeyBinding::new("home", CursorHome, context),
        KeyBinding::new("end", CursorEnd, context),
        KeyBinding::new("pageup", PageUp, context),
        KeyBinding::new("pagedown", PageDown, context),
        KeyBinding::new("enter", Activate, context),
        KeyBinding::new("backspace", GoUp, context),
        KeyBinding::new("space", ToggleSelection, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("alt-enter", ShowProperties, context),
        KeyBinding::new("alt-f7", FindFiles, context),
        KeyBinding::new("ctrl-shift-t", OpenTerminal, context),
        // Only while a panel has focus: a text field's context is deeper, so
        // its own Ctrl-C/Ctrl-Ins win there. macOS also takes Cmd-C, shown
        // in its menu (the first binding there, the last on Linux).
        KeyBinding::new(copy_path, CopyPath, context),
        KeyBinding::new("ctrl-c", CopyPath, context),
        KeyBinding::new("ctrl-insert", CopyPath, context),
        KeyBinding::new(copy_path, CopyPath, context),
        KeyBinding::new("f2", Rename, context),
        KeyBinding::new("f3", View, context),
        KeyBinding::new("f4", Edit, context),
        KeyBinding::new("shift-f4", EditNewFile, context),
        KeyBinding::new("f5", Copy, context),
        KeyBinding::new("f6", Move, context),
        KeyBinding::new("f7", MakeDirectory, context),
        KeyBinding::new("f8", Trash, context),
        KeyBinding::new("delete", Trash, context),
        KeyBinding::new("f8", Trash, context),
        KeyBinding::new("shift-f8", Delete, context),
        KeyBinding::new("shift-delete", Delete, context),
        KeyBinding::new("shift-f8", Delete, context),
        KeyBinding::new("alt-f5", Pack, context),
        // Alt-F9 like TC (Alt-F6 failed on Windows 95). Alt-F6 registered
        // first and last so both menus show it.
        KeyBinding::new("alt-f6", Extract, context),
        KeyBinding::new("alt-f9", Extract, context),
        KeyBinding::new("alt-f6", Extract, context),
        KeyBinding::new("alt-z", SyncOtherPanel, context),
        KeyBinding::new("ctrl-u", SwapPanels, context),
        KeyBinding::new("ctrl-t", NewTab, context),
        KeyBinding::new("ctrl-w", CloseTab, context),
        KeyBinding::new("ctrl-tab", NextTab, context),
        KeyBinding::new("ctrl-shift-tab", PrevTab, context),
        KeyBinding::new("ctrl-r", Reload, context),
        KeyBinding::new("ctrl-d", DirectoryHotlist, context),
        KeyBinding::new("ctrl-.", ToggleHidden, context),
        KeyBinding::new("ctrl-,", OpenSettings, context),
        KeyBinding::new("escape", CancelSearch, context),
    ]);
    // The in-window menu bar (Linux; macOS has the native one). "alt" alone
    // is gpui's modifier-only binding: Alt pressed and released with no
    // other key in between.
    if cfg!(not(target_os = "macos")) {
        cx.bind_keys([
            KeyBinding::new("f10", ToggleMenu, context),
            KeyBinding::new("alt", MenuAlt, context),
            KeyBinding::new("alt-f", OpenFilesMenu, context),
        ]);
    }
    // Text fields: the classic clipboard keys next to gpui-base's Ctrl-C/X/V.
    // Registered after gpui-base's own, so Shift-Del cuts instead of deleting
    // a character.
    let input = Some(INPUT_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-insert", input::Copy, input),
        KeyBinding::new("shift-delete", input::Cut, input),
        KeyBinding::new("shift-insert", input::Paste, input),
    ]);
    // More specific than the dialog's own Enter (always OK) and Root's Tab.
    let row = Some(BUTTON_ROW_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", PrevButton, row),
        KeyBinding::new("right", NextButton, row),
        KeyBinding::new("tab", TabNext, row),
        KeyBinding::new("shift-tab", TabPrev, row),
        KeyBinding::new("enter", PressButton, row),
        KeyBinding::new("space", PressButton, row),
        // A message box's text (see `ButtonRow::set_copy_text`).
        KeyBinding::new("ctrl-c", CopyText, row),
        KeyBinding::new("ctrl-insert", CopyText, row),
    ]);
    if cfg!(target_os = "macos") {
        cx.bind_keys([KeyBinding::new("cmd-c", CopyText, row)]);
    }
    // More specific than the file manager's Up/Down/Enter/Escape.
    let popup = Some(HOTLIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", hotlist::Up, popup),
        KeyBinding::new("down", hotlist::Down, popup),
        KeyBinding::new("enter", hotlist::Pick, popup),
        KeyBinding::new("escape", hotlist::Close, popup),
    ]);
    let list = Some(HOTLIST_LIST_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", hotlist_list::Up, list),
        KeyBinding::new("down", hotlist_list::Down, list),
        KeyBinding::new("delete", hotlist_list::Remove, list),
        KeyBinding::new("alt-up", hotlist_list::MoveUp, list),
        KeyBinding::new("alt-down", hotlist_list::MoveDown, list),
    ]);
    let results = Some(FIND_RESULTS_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", find_results::Up, results),
        KeyBinding::new("down", find_results::Down, results),
        KeyBinding::new("pageup", find_results::PageUp, results),
        KeyBinding::new("pagedown", find_results::PageDown, results),
        KeyBinding::new("home", find_results::Home, results),
        KeyBinding::new("end", find_results::End, results),
        KeyBinding::new("enter", find_results::Open, results),
        KeyBinding::new("f3", find_results::View, results),
        KeyBinding::new("space", find_results::Toggle, Some(FIND_OPTION_CONTEXT)),
        KeyBinding::new(
            "space",
            find_results::SkipOpen,
            Some(FIND_SKIP_BUTTON_CONTEXT),
        ),
        KeyBinding::new(
            "enter",
            find_results::SkipOpen,
            Some(FIND_SKIP_BUTTON_CONTEXT),
        ),
        KeyBinding::new("up", find_results::SkipUp, Some(FIND_SKIP_CONTEXT)),
        KeyBinding::new("down", find_results::SkipDown, Some(FIND_SKIP_CONTEXT)),
        KeyBinding::new("space", find_results::SkipToggle, Some(FIND_SKIP_CONTEXT)),
        KeyBinding::new("enter", find_results::SkipClose, Some(FIND_SKIP_CONTEXT)),
        KeyBinding::new("escape", find_results::SkipClose, Some(FIND_SKIP_CONTEXT)),
        // TC's "Feed to listbox" key.
        KeyBinding::new("alt-l", find_results::Feed, Some(FIND_DIALOG_CONTEXT)),
    ]);
    let ctx = Some(VIEWER_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", viewer::Close, ctx),
        KeyBinding::new("q", viewer::Close, ctx),
        // Without a window manager (or where it doesn't take Alt-F4), close
        // only this window instead of quitting the app.
        KeyBinding::new("alt-f4", viewer::Close, ctx),
        KeyBinding::new("up", viewer::LineUp, ctx),
        KeyBinding::new("down", viewer::LineDown, ctx),
        KeyBinding::new("pageup", viewer::PageUp, ctx),
        KeyBinding::new("pagedown", viewer::PageDown, ctx),
        KeyBinding::new("home", viewer::Start, ctx),
        KeyBinding::new("ctrl-home", viewer::Start, ctx),
        KeyBinding::new("end", viewer::End, ctx),
        KeyBinding::new("ctrl-end", viewer::End, ctx),
        KeyBinding::new("w", viewer::ToggleWrap, ctx),
        KeyBinding::new("left", viewer::ScrollLeft, ctx),
        KeyBinding::new("right", viewer::ScrollRight, ctx),
        KeyBinding::new("ctrl-f", viewer::Find, ctx),
        KeyBinding::new("f3", viewer::FindNext, ctx),
        KeyBinding::new("enter", viewer::FindNext, ctx),
        KeyBinding::new("shift-f3", viewer::FindPrevious, ctx),
    ]);
}
