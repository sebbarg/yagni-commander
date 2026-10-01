mod actions;
mod app_state;
mod button_row;
mod columns;
mod file_manager;
mod panel_view;
mod theme;
mod viewer_view;
mod windows;

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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let app_state = AppState::load();
    // Folders given on the command line must exist; the user typed them.
    // (Saved folders are not checked: an offline mount would block here.)
    for arg in &args {
        if !std::path::Path::new(arg).is_dir() {
            eprintln!("yagni-commander: not a directory: {arg}");
            std::process::exit(1);
        }
    }
    let home = storage::home_dir();
    let (left, right) = app_state.state.startup_dirs(&args, &home);
    let mut commander = Commander::start(left, right, app_state.state.show_hidden, home);
    if let Some(active) = app_state.state.active {
        commander.set_active(active);
    }

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
        let (main_window, _) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| FileManager::new(commander, notice, window, cx))
        })
        .expect("failed to open window");
        windows::close_all_when_main_closes(main_window, cx).detach();
        cx.activate(true);
    });
}
