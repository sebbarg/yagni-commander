//! Window lifetime rules: the main window owns the app. Closing it closes
//! every viewer and quits, so no viewer is left without a file manager (and
//! macOS doesn't keep a windowless process around).

use gpui_kit::{AnyWindowHandle, App, Subscription};

/// Every window's app id: the Wayland `app_id` and the X11 `WM_CLASS`.
/// Desktops match it to the `.desktop` file (icon, taskbar grouping) and
/// window rules use it (Hyprland `class:yagni-commander`).
pub const APP_ID: &str = "yagni-commander";

pub fn close_all_when_main_closes(main: AnyWindowHandle, cx: &mut App) -> Subscription {
    let main_id = main.window_id();
    cx.on_window_closed(move |cx, closed| {
        if closed != main_id && !cx.windows().is_empty() {
            return;
        }
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        cx.quit();
    })
}
