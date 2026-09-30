mod actions;
mod columns;
mod file_manager;
mod panel_view;
mod theme;

use gpui::{App, AppContext, Bounds, Menu, MenuItem, WindowBounds, WindowOptions, px, size};
use yagni_commander_core::Commander;

use crate::actions::Quit;
use crate::file_manager::FileManager;
use crate::theme::Theme;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut args = std::env::args().skip(1);
    let left = args.next().unwrap_or_else(|| ".".into());
    let right = args.next().unwrap_or_else(|| left.clone());
    let commander = match Commander::new(&left, &right) {
        Ok(commander) => commander,
        Err(e) => {
            eprintln!("yagni-commander: cannot open {left} / {right}: {e}");
            std::process::exit(1);
        }
    };

    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_global(Theme::tokyo_night());
        actions::bind_default_keys(cx);

        // gpui has no built-in quit: handle it and expose it in the menu bar.
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus([Menu::new("yagni-commander").items([MenuItem::action("Quit", Quit)])]);
        // Keep macOS from leaving a windowless process behind.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        let commander = cx.new(|_| commander);
        cx.open_window(options, |window, cx| {
            cx.new(|cx| FileManager::new(commander, window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
