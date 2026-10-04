//! Window lifetime rules: the main window owns the app. Closing it closes
//! every viewer and quits, so no viewer is left without a file manager (and
//! macOS doesn't keep a windowless process around).

use std::time::Duration;

use gpui_kit::{AnyWindowHandle, App, Subscription, WindowBounds};

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

/// How long after opening a window `maximize_late` looks: by then the window
/// manager has mapped it and reported its state.
const MAXIMIZE_AFTER: Duration = Duration::from_millis(200);

/// Maximizes a window opened with `WindowBounds::Maximized` on X11. gpui
/// asks for it (a `_NET_WM_STATE` message) before mapping the window, and
/// window managers only handle that message for mapped windows (EWMH), so
/// KWin never maximizes it. The request is a toggle, so it is repeated only
/// if the window isn't maximized by `MAXIMIZE_AFTER`. Wayland needs nothing
/// (asking before the first commit is allowed; Omarchy ignores it on purpose).
pub fn maximize_late(window: AnyWindowHandle, bounds: WindowBounds, cx: &mut App) {
    if !wants_late_maximize(cx.compositor_name(), bounds) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(MAXIMIZE_AFTER).await;
        let _ = window.update(cx, |_, window, _| {
            if !window.is_maximized() {
                window.zoom_window();
            }
        });
    })
    .detach();
}

fn wants_late_maximize(compositor: &str, bounds: WindowBounds) -> bool {
    compositor == "X11" && matches!(bounds, WindowBounds::Maximized(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn only_a_maximized_window_on_x11_is_maximized_late() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(600.0)));
        assert!(wants_late_maximize("X11", WindowBounds::Maximized(bounds)));
        assert!(!wants_late_maximize("X11", WindowBounds::Windowed(bounds)));
        assert!(!wants_late_maximize(
            "X11",
            WindowBounds::Fullscreen(bounds)
        ));
        // Wayland compositors report their own name; macOS an empty one.
        assert!(!wants_late_maximize(
            "Hyprland",
            WindowBounds::Maximized(bounds)
        ));
        assert!(!wants_late_maximize("", WindowBounds::Maximized(bounds)));
    }
}
