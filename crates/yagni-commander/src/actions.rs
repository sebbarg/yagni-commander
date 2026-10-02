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
        MakeDirectory,
        Copy,
        Move,
        Trash,
        Delete,
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
        OpenSettings
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
            ScrollRight
        ]
    );
}

actions!(
    button_row,
    [PrevButton, NextButton, TabNext, TabPrev, PressButton]
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
    cx.bind_keys([
        KeyBinding::new(quit, Quit, None),
        KeyBinding::new("tab", SwitchPanel, context),
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
        KeyBinding::new("alt-z", SyncOtherPanel, context),
        KeyBinding::new("ctrl-u", SwapPanels, context),
        KeyBinding::new("ctrl-t", NewTab, context),
        KeyBinding::new("ctrl-w", CloseTab, context),
        KeyBinding::new("ctrl-tab", NextTab, context),
        KeyBinding::new("ctrl-shift-tab", PrevTab, context),
        KeyBinding::new("ctrl-r", Reload, context),
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
    ]);
}
