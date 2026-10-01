//! Window lifetime rules: the main window owns the app. Closing it closes
//! every viewer and quits, so no viewer is left without a file manager (and
//! macOS doesn't keep a windowless process around).

use gpui_kit::{AnyWindowHandle, App, Subscription};

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
