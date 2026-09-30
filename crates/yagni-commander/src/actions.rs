//! Actions and the default keymap. The keymap is data, so a user config file
//! can later add or override bindings without touching the views.

use gpui_kit::{App, KeyBinding, actions};

/// Key context of the file manager's root view; scopes the bindings below.
pub const FILE_MANAGER_CONTEXT: &str = "FileManager";

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
        MakeDirectory,
        SyncOtherPanel,
        SwapPanels,
        Reload,
        ToggleHidden,
        Quit
    ]
);

pub fn bind_default_keys(cx: &mut App) {
    let context = Some(FILE_MANAGER_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("alt-f4", Quit, None),
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
        KeyBinding::new("f4", Edit, context),
        KeyBinding::new("f7", MakeDirectory, context),
        KeyBinding::new("alt-z", SyncOtherPanel, context),
        KeyBinding::new("ctrl-u", SwapPanels, context),
        KeyBinding::new("ctrl-r", Reload, context),
        KeyBinding::new("ctrl-.", ToggleHidden, context),
    ]);
}
