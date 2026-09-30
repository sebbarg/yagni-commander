mod actions;
mod app_state;
mod button_row;
mod columns;
mod file_manager;
mod panel_view;
mod theme;

use gpui_kit::{App, AppContext, Global, Menu, MenuItem, WindowOptions};
use yagni_commander_core::{Commander, Config, oplog, storage};

use crate::actions::Quit;
use crate::app_state::AppState;
use crate::file_manager::FileManager;
use crate::theme::Theme;

/// The loaded settings, available to every view.
pub(crate) struct CurrentConfig(pub Config);

impl Global for CurrentConfig {}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut args = std::env::args().skip(1);
    let left = args.next().unwrap_or_else(|| ".".into());
    let right = args.next().unwrap_or_else(|| left.clone());
    let app_state = AppState::load();
    let mut commander = match Commander::new(&left, &right, app_state.state.show_hidden) {
        Ok(commander) => commander,
        Err(e) => {
            eprintln!("yagni-commander: cannot open {left} / {right}: {e}");
            std::process::exit(1);
        }
    };

    // A broken config is reported in the UI and never overwritten; the app
    // runs on defaults until the user fixes it.
    let (config, notice) = match storage::config_file().map(|path| Config::load_or_create(&path)) {
        Some(Ok(config)) => (config, None),
        Some(Err(e)) => (Config::default(), Some(format!("Config ignored: {e}"))),
        None => (
            Config::default(),
            Some("No config directory found".to_owned()),
        ),
    };
    commander.set_case_sensitive_sort(config.case_sensitive_sort);
    let log_dir = storage::log_dir();
    let (log, log_problem) = oplog::start(log_dir.as_deref(), config.log, config.log_keep_days);
    commander.set_log(log.map(std::sync::Arc::new));
    let notice = notice.or(log_problem);

    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        Theme::default().install(cx);
        cx.set_global(CurrentConfig(config));
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

        let options = WindowOptions {
            window_bounds: Some(app_state.initial_bounds(cx)),
            ..Default::default()
        };
        cx.set_global(app_state);
        cx.on_app_quit(|cx| {
            cx.global::<AppState>().save();
            async {}
        })
        .detach();

        let commander = cx.new(|_| commander);
        // Wraps the view in gpui-kit's Root, which hosts dialogs and notifications.
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| FileManager::new(commander, notice, window, cx))
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
