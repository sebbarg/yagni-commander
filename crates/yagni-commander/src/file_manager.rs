//! The root view: two panels side by side with a draggable divider, a status
//! line, and the keyboard actions that drive the shared [`Commander`].

use gpui_kit::{
    App, Context, Entity, FocusHandle, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, SharedString, Subscription, Window, div, prelude::*, px,
    relative,
};
use std::path::PathBuf;
use std::sync::Arc;

use yagni_commander_core::{Command, Commander, Config, Setting, Side, SortKey, config, oplog};

pub(crate) mod commands;
mod file_ops;
mod loads;
mod watch;

use crate::actions::{
    About, Activate, ButtonTest, CancelSearch, Copy, CursorDown, CursorEnd, CursorHome, CursorUp,
    Delete, Edit, EditNewFile, FILE_MANAGER_CONTEXT, GoUp, MakeDirectory, MenuAlt, Move,
    OpenSettings, PageDown, PageUp, Reload, Rename, SelectAll, SortByModified, SortByName,
    SortByOwner, SortByPermissions, SortBySize, SwapPanels, SwitchPanel, SyncOtherPanel,
    ToggleHidden, ToggleMenu, ToggleSelection, Trash, View,
};
use crate::app_state::AppState;
use crate::config_state::CurrentConfig;
use crate::menu_bar::{self, MenuBar};
use crate::menus::{self, MenuState};
use crate::panel_view::PanelView;
use crate::theme::Theme;

const PADDING: f32 = 6.0;
const DIVIDER_WIDTH: f32 = 6.0;
const STATUS_HEIGHT: f32 = 22.0;
/// An Alt press this soon after the window became active is the tail of a
/// window switch, never a lone Alt for the menu.
const ALT_AFTER_ACTIVATION: std::time::Duration = std::time::Duration::from_millis(200);
/// Rows to page by before the list has been laid out.
const FALLBACK_PAGE_ROWS: usize = 20;

/// Runs a command against the commander and notifies its observers.
pub fn execute(commander: &Entity<Commander>, command: Command, cx: &mut App) {
    commander.update(cx, |commander, cx| {
        // Enter on a file does nothing yet, so the outcome is ignored.
        let _ = commander.execute(command);
        cx.notify();
    });
}

/// The active panel's path, then the app name.
fn window_title(commander: &Commander) -> String {
    let path = commander.panel(commander.active()).path();
    format!("{} - yagni-commander", path.display())
}

pub struct FileManager {
    commander: Entity<Commander>,
    left: Entity<PanelView>,
    right: Entity<PanelView>,
    pub(crate) focus: FocusHandle,
    /// Left panel's share of the width, changed by dragging the divider.
    split_ratio: f32,
    dragging_split: bool,
    /// One-off message (e.g. a config problem) shown in the status line until
    /// the next command.
    notice: Option<SharedString>,
    /// The copy, move or trash operation in progress.
    job: Option<file_ops::RunningJob>,
    /// What F8 uses. Tests replace it so they never touch the real trash.
    trash: yagni_commander_core::file_ops::TrashFn,
    /// Runs directory reads; `None` reads them inline (tests).
    load: Option<loads::LoadFn>,
    /// Directory reads in progress.
    loads: Vec<loads::RunningLoad>,
    /// Whether the timer that collects their results is running.
    polling_loads: bool,
    /// Watches each panel's folder (left, right); `None` in tests, which
    /// report changes through `changes` by hand.
    watchers: Option<[yagni_commander_core::watch::PanelWatcher; 2]>,
    /// The folder each watcher was last pointed at.
    watched: [Option<PathBuf>; 2],
    /// Where the watchers report changes; tests report through it by hand.
    #[cfg(test)]
    changes: futures::channel::mpsc::UnboundedSender<Side>,
    /// The open settings dialog's view.
    pub(crate) settings: Option<Entity<crate::settings_dialog::SettingsView>>,
    /// Where the operation log goes. Tests replace it.
    pub(crate) log_dir: Option<PathBuf>,
    /// The in-window menu bar; `None` on macOS, which has the native one.
    menu_bar: Option<Entity<MenuBar>>,
    /// What the menus' check marks show; `None` before the first update.
    menu_state: Option<MenuState>,
    /// Alt went down alone and no mouse click or window switch followed (see
    /// [`Self::menu_alt`]).
    alt_armed: bool,
    /// When the window last became active or inactive (`None` before that).
    activated_at: Option<std::time::Instant>,
    _subscriptions: Vec<Subscription>,
}

impl FileManager {
    pub fn new(
        commander: Entity<Commander>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let left = cx.new(|cx| PanelView::new(commander.clone(), Side::Left, cx));
        let right = cx.new(|cx| PanelView::new(commander.clone(), Side::Right, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let menu_bar = cfg!(not(target_os = "macos"))
            .then(|| cx.new(|_| MenuBar::new(Vec::new(), focus.clone())));

        let subscriptions = vec![
            cx.observe_in(&commander, window, |this, commander, window, cx| {
                this.start_loads(window, cx);
                this.watch_panels(cx);
                AppState::remember_panels(&commander, cx);
                this.update_title(window, cx);
                this.update_menus(cx);
                cx.notify();
            }),
            cx.observe_window_bounds(window, |_, window, cx| {
                AppState::remember_window(window.window_bounds(), cx);
            }),
            // A window switch (Alt-Tab) between Alt's press and release must
            // not open the menu; an open menu closes.
            cx.observe_window_activation(window, |this, window, cx| {
                this.activated_at = Some(cx.background_executor().now());
                if !window.is_window_active() {
                    this.alt_armed = false;
                    if let Some(bar) = this.menu_bar.clone() {
                        bar.update(cx, |bar, cx| bar.close(window, cx));
                    }
                }
            }),
            // While a menu is open, keys other than the menu's own are
            // ignored. This runs before key bindings, which matters: the open
            // menu has focus, but the file manager's bindings (F5, ...) are on
            // the same dispatch path.
            cx.intercept_keystrokes({
                let bar = menu_bar.as_ref().map(Entity::downgrade);
                let handle = window.window_handle();
                move |event, window, cx| {
                    let Some(bar) = bar.as_ref().and_then(|bar| bar.upgrade()) else {
                        return;
                    };
                    if window.window_handle() == handle
                        && bar.read(cx).is_open()
                        && !menu_bar::passes(&event.keystroke)
                    {
                        cx.stop_propagation();
                    }
                }
            }),
        ];
        AppState::remember_window(window.window_bounds(), cx);
        AppState::remember_panels(&commander, cx);

        let (changes, received) = futures::channel::mpsc::unbounded();
        let this = Self {
            commander,
            left,
            right,
            focus,
            split_ratio: 0.5,
            dragging_split: false,
            notice: notice.map(Into::into),
            job: None,
            trash: yagni_commander_core::file_ops::system_trash,
            load: Some(loads::spawn_load),
            loads: Vec::new(),
            polling_loads: false,
            watchers: Some(watch::spawn_watchers(&changes)),
            watched: [None, None],
            #[cfg(test)]
            changes,
            settings: None,
            log_dir: yagni_commander_core::storage::log_dir(),
            menu_bar,
            menu_state: None,
            alt_armed: false,
            activated_at: None,
            _subscriptions: subscriptions,
        };
        this.update_title(window, cx);
        Self::receive_changes(received, window, cx);
        // Startup's reads: the observer above starts them.
        this.commander.update(cx, |_, cx| cx.notify());
        this
    }

    fn update_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&window_title(self.commander.read(cx)));
    }

    /// Rebuilds the menus when their check marks change.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        let state = MenuState::of(self.commander.read(cx));
        if self.menu_state == Some(state) {
            return;
        }
        self.menu_state = Some(state);
        let mac = cfg!(target_os = "macos");
        cx.set_menus(menus::to_gpui(&menus::menus(state, mac)));
        if let Some(bar) = &self.menu_bar {
            bar.update(cx, |bar, cx| bar.set_menus(menus::menus(state, false), cx));
        }
    }

    /// F10: opens or closes the menu bar (Linux).
    fn toggle_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        if let Some(bar) = self.menu_bar.clone() {
            bar.update(cx, |bar, cx| bar.toggle(window, cx));
        }
    }

    /// A lone Alt (gpui's "alt" binding: pressed and released with no key in
    /// between). gpui doesn't count mouse clicks or window switches in
    /// between, so `alt_armed` covers those.
    fn menu_alt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.alt_armed) {
            self.toggle_menu(window, cx);
        }
    }

    /// The settings dialog changed `setting`: saves just that key, then
    /// applies the file's settings. Not while the file is broken or
    /// missing. A failed save shows an error box; the change still applies.
    pub(crate) fn change_setting(
        &mut self,
        setting: Setting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = cx.global::<CurrentConfig>();
        if current.problem.is_some() {
            return;
        }
        let path = current.path.clone();
        let mut config = current.config.clone();
        config.set(&setting);
        if config == current.config {
            return;
        }
        match path.map(|path| config::save_setting(&path, &setting)) {
            Some(Ok(on_disk)) => config = on_disk,
            Some(Err(e)) => {
                // Deferred: a save on close runs while the settings dialog
                // is being closed, which would close this error box instead.
                let message = e.to_string();
                window.defer(cx, move |window, cx| {
                    commands::show_error("Settings not saved", message, None, window, cx)
                });
            }
            None => {}
        }
        self.apply_config(config, cx);
    }

    /// Makes `config` the settings in use: sort, log, and the global the
    /// views read (editor). A copy or move already running keeps the log it
    /// started with.
    pub(crate) fn apply_config(&mut self, config: Config, cx: &mut Context<Self>) {
        let old = cx.global::<CurrentConfig>().config.clone();
        if config.case_sensitive_sort != old.case_sensitive_sort {
            self.commander.update(cx, |c, cx| {
                c.set_case_sensitive_sort(config.case_sensitive_sort);
                cx.notify();
            });
        }
        if config.hidden_columns() != old.hidden_columns() {
            self.commander.update(cx, |c, cx| {
                c.hide_columns(&config.hidden_columns());
                cx.notify();
            });
        }
        if config.log != old.log {
            let (log, problem) = if config.log {
                oplog::start(self.log_dir.as_deref(), true, config.log_keep_days)
            } else {
                (None, None)
            };
            self.commander
                .update(cx, |c, _| c.set_log(log.map(Arc::new)));
            if let Some(problem) = problem {
                self.notice = Some(problem.into());
            }
        }
        cx.global_mut::<CurrentConfig>().config = config;
        cx.notify();
    }

    /// Ctrl-R: re-reads the config file. If it doesn't parse, the settings
    /// stay and the status line says why.
    fn reload_config(&mut self, cx: &mut Context<Self>) {
        let Some(path) = cx.global::<CurrentConfig>().path.clone() else {
            return;
        };
        match Config::load_or_create(&path) {
            Ok(config) => {
                cx.global_mut::<CurrentConfig>().problem = None;
                self.apply_config(config, cx);
            }
            Err(e) => {
                let problem = format!("Config ignored: {e}");
                self.notice = Some(problem.clone().into());
                cx.global_mut::<CurrentConfig>().problem = Some(problem);
            }
        }
    }

    fn active_panel<'a>(&self, cx: &'a App) -> &'a yagni_commander_core::Panel {
        let commander = self.commander.read(cx);
        commander.panel(commander.active())
    }

    fn execute(&mut self, command: Command, cx: &mut Context<Self>) {
        self.notice = None;
        execute(&self.commander, command, cx);
    }

    /// Closes the quick search box. Commands that go through
    /// [`Self::execute`] end it anyway; this is for those that open a dialog.
    pub(crate) fn end_search(&mut self, cx: &mut Context<Self>) {
        self.commander.update(cx, |commander, cx| {
            if commander.search_cancel() {
                cx.notify();
            }
        });
    }

    /// Runs a quick search step (see `Commander::search_*`); if no search is
    /// open, runs `otherwise` instead.
    fn search_or(
        &mut self,
        step: fn(&mut Commander) -> bool,
        otherwise: Command,
        cx: &mut Context<Self>,
    ) {
        let searching = self.commander.update(cx, |commander, cx| {
            let searching = step(commander);
            cx.notify();
            searching
        });
        if !searching {
            self.execute(otherwise, cx);
        }
    }

    /// Escape: closes the quick search box, else stops loading the active
    /// panel (see `Commander::cancel_load`).
    fn escape(&mut self, cx: &mut Context<Self>) {
        self.commander.update(cx, |c, cx| {
            let active = c.active();
            if c.search_cancel() || c.cancel_load(active) {
                cx.notify();
            }
        });
    }

    /// Whether the active panel is reading a folder; its dialogs and file
    /// operations wait until it is done.
    fn active_loading(&self, cx: &App) -> bool {
        self.active_panel(cx).loading().is_some()
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.execute(Command::ToggleHidden, cx);
        let show = self.commander.read(cx).shows_hidden();
        AppState::remember_show_hidden(show, cx);
    }

    /// Sorts the active panel by `key`; the same key again reverses.
    fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        let side = self.commander.read(cx).active();
        self.execute(Command::SortBy(side, key), cx);
    }

    fn page(&mut self, down: bool, cx: &mut Context<Self>) {
        let panel = match self.commander.read(cx).active() {
            Side::Left => &self.left,
            Side::Right => &self.right,
        };
        let rows = panel.read(cx).visible_rows().unwrap_or(FALLBACK_PAGE_ROWS);
        let delta = rows.saturating_sub(1).max(1) as isize;
        self.execute(Command::CursorBy(if down { delta } else { -delta }), cx);
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging_split {
            return;
        }
        if event.pressed_button == Some(MouseButton::Left) {
            let width = f32::from(window.viewport_size().width) - 2.0 * PADDING;
            let x = f32::from(event.position.x) - PADDING - DIVIDER_WIDTH / 2.0;
            self.split_ratio = (x / width).clamp(0.1, 0.9);
        } else {
            // Released outside the window.
            self.dragging_split = false;
        }
        cx.notify();
    }
}

impl Render for FileManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;

        let divider = div()
            .id("divider")
            .debug_selector(|| "divider".into())
            .w(px(DIVIDER_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .when(self.dragging_split, |d| d.bg(colors.accent))
            .hover(|d| d.bg(colors.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.dragging_split = true;
                    cx.notify();
                }),
            );

        let status = match (self.commander.read(cx).error(), &self.notice) {
            (Some(err), _) => div().text_color(colors.error).child(err.to_owned()),
            (None, Some(notice)) => div().text_color(colors.error).child(notice.clone()),
            (None, None) => div()
                .text_color(colors.text_dim)
                .child("Tab switch · ↑↓ move · Enter open · Backspace up"),
        };

        div()
            .key_context(FILE_MANAGER_CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &SwitchPanel, _, cx| this.execute(Command::SwitchPanel, cx)),
            )
            // While a quick search is open, Up/Down step through its matches
            // and Backspace shortens it.
            .on_action(cx.listener(|this, _: &CursorUp, _, cx| {
                this.search_or(|c| c.search_step(false), Command::CursorUp, cx)
            }))
            .on_action(cx.listener(|this, _: &CursorDown, _, cx| {
                this.search_or(|c| c.search_step(true), Command::CursorDown, cx)
            }))
            .on_action(cx.listener(|this, _: &CancelSearch, _, cx| this.escape(cx)))
            .on_action(
                cx.listener(|this, _: &CursorHome, _, cx| this.execute(Command::CursorHome, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorEnd, _, cx| this.execute(Command::CursorEnd, cx)),
            )
            .on_action(cx.listener(|this, _: &Activate, _, cx| this.execute(Command::Activate, cx)))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| {
                this.search_or(Commander::search_backspace, Command::GoUp, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSelection, _, cx| {
                this.execute(Command::ToggleSelection, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectAll, _, cx| this.execute(Command::SelectAll, cx)),
            )
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.page(false, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.page(true, cx)))
            .on_action(
                cx.listener(|this, _: &SwapPanels, _, cx| this.execute(Command::SwapPanels, cx)),
            )
            .on_action(cx.listener(|this, _: &SyncOtherPanel, _, cx| {
                this.execute(Command::SyncOtherPanel, cx)
            }))
            .on_action(cx.listener(|this, _: &Reload, _, cx| {
                this.execute(Command::Reload, cx);
                this.reload_config(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| this.toggle_hidden(cx)))
            .on_action(cx.listener(|this, _: &Rename, window, cx| this.rename(window, cx)))
            .on_action(
                cx.listener(|this, _: &MakeDirectory, window, cx| this.make_directory(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Edit, window, cx| this.edit(window, cx)))
            .on_action(cx.listener(|this, _: &View, window, cx| this.view_file(window, cx)))
            .on_action(cx.listener(|this, _: &Copy, window, cx| {
                this.copy_or_move(file_ops::Kind::Copy, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Move, window, cx| {
                this.copy_or_move(file_ops::Kind::Move, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Trash, window, cx| {
                this.trash(file_ops::Kind::Trash, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Delete, window, cx| {
                this.trash(file_ops::Kind::Delete, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ButtonTest, window, cx| this.button_test(window, cx)))
            .on_action(
                cx.listener(|this, _: &EditNewFile, window, cx| this.edit_new_file(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SortByName, _, cx| this.sort_by(SortKey::Name, cx)))
            .on_action(cx.listener(|this, _: &SortBySize, _, cx| this.sort_by(SortKey::Size, cx)))
            .on_action(
                cx.listener(|this, _: &SortByModified, _, cx| this.sort_by(SortKey::Modified, cx)),
            )
            .on_action(cx.listener(|this, _: &SortByOwner, _, cx| this.sort_by(SortKey::Owner, cx)))
            .on_action(cx.listener(|this, _: &SortByPermissions, _, cx| {
                this.sort_by(SortKey::Permissions, cx)
            }))
            .on_action(cx.listener(|this, _: &About, window, cx| this.about(window, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleMenu, window, cx| this.toggle_menu(window, cx)))
            .on_action(cx.listener(|this, _: &MenuAlt, window, cx| this.menu_alt(window, cx)))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                // Wayland reports an Alt still held from Alt-Tab right after
                // the window gets focus; that press belongs to the switch.
                let now = cx.background_executor().now();
                let settled = this
                    .activated_at
                    .is_none_or(|at| now - at > ALT_AFTER_ACTIVATION);
                if event.modifiers == Modifiers::alt() && settled {
                    this.alt_armed = true;
                }
            }))
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.alt_armed = false))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging_split = false;
                    cx.notify();
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.window_bg)
            .text_color(colors.text)
            .text_size(px(14.0))
            .when_some(self.menu_bar.clone(), |d, bar| d.child(bar))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .p(px(PADDING))
                    .child(
                        div()
                            .w(relative(self.split_ratio))
                            .flex_none()
                            .h_full()
                            .child(self.left.clone()),
                    )
                    .child(divider)
                    .child(div().flex_1().min_w_0().h_full().child(self.right.clone())),
            )
            .child(
                status
                    .h(px(STATUS_HEIGHT))
                    .flex_none()
                    .px(px(10.0))
                    .text_size(px(12.0)),
            )
    }
}

#[cfg(test)]
mod tests;
