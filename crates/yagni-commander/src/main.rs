mod actions;
mod app_state;
mod button_row;
mod columns;
mod config_state;
mod file_manager;
mod find_dialog;
mod fonts;
mod hotlist_dialog;
mod hotlist_popup;
mod info_dialog;
mod menu_bar;
mod menus;
mod option_box;
mod panel_view;
mod settings_dialog;
mod theme;
mod viewer_view;
mod windows;
mod zoom;

use gpui_kit::{App, AppContext, WindowOptions};
use yagni_commander_core::{Commander, oplog, storage};

use crate::actions::Quit;
use crate::app_state::AppState;
use crate::config_state::CurrentConfig;
use crate::file_manager::FileManager;
use crate::theme::Theme;

fn main() {
    // gpui's macOS text system warns about every duplicate face in the system
    // font family (harmless: it skips them), once per font key loaded.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,gpui_macos::text_system=error"),
    )
    .init();

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
    // Private copies of archive entries a crashed run left behind (other
    // running instances keep theirs).
    if let Some(dir) = storage::viewer_temp_dir() {
        storage::clear_stale_temp(&dir);
    }
    let home = storage::home_dir();
    let (left, right) = app_state.state.startup_tabs(&args, &home);
    let mut commander = Commander::start_tabs(left, right, app_state.state.show_hidden, home);
    if let Some(active) = app_state.state.active {
        commander.set_active(active);
    }

    // A broken config is reported in the UI and never overwritten; the app
    // runs on defaults until the user fixes it.
    let current = CurrentConfig::load(storage::config_file());
    let config = &current.config;
    commander.set_case_sensitive_sort(config.case_sensitive_sort);
    commander.hide_columns(&config.hidden_columns());
    commander.set_icons(config.icons);
    let log_dir = storage::log_dir();
    let (log, log_problem) = oplog::start(log_dir.as_deref(), config.log, config.log_keep_days);
    commander.set_log(log.map(std::sync::Arc::new));
    let notice = current.problem.clone().or(log_problem);

    // The icon SVGs (e.g. the menus' check mark).
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            fonts::register(cx);
            Theme::default().install(cx);
            cx.set_global(app_state.state.zoom());
            zoom::apply_ui(cx);
            cx.set_global(current);
            actions::bind_default_keys(cx);

            // gpui has no built-in quit. The menus are set by the FileManager.
            cx.on_action(|_: &Quit, cx| cx.quit());

            let options = WindowOptions {
                window_bounds: Some(app_state.initial_bounds(cx)),
                app_id: Some(windows::APP_ID.into()),
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
