//! The root view: two panels side by side with a draggable divider, a status
//! line, and the keyboard actions that drive the shared [`Commander`].

use gpui_kit::{
    App, Context, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, SharedString,
    Subscription, Window, div, prelude::*, px, relative,
};
use yagni_commander_core::{Command, Commander, Side};

mod commands;
mod file_ops;
mod loads;

use crate::actions::{
    Activate, ButtonTest, CancelSearch, Copy, CursorDown, CursorEnd, CursorHome, CursorUp, Delete,
    Edit, EditNewFile, FILE_MANAGER_CONTEXT, GoUp, MakeDirectory, Move, PageDown, PageUp, Reload,
    Rename, SelectAll, SwapPanels, SwitchPanel, SyncOtherPanel, ToggleHidden, ToggleSelection,
    Trash, View,
};
use crate::app_state::AppState;
use crate::panel_view::PanelView;
use crate::theme::Theme;

const PADDING: f32 = 6.0;
const DIVIDER_WIDTH: f32 = 6.0;
const STATUS_HEIGHT: f32 = 22.0;
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
    focus: FocusHandle,
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
        let subscriptions = vec![
            cx.observe_in(&commander, window, |this, commander, window, cx| {
                this.start_loads(window, cx);
                AppState::remember_panels(&commander, cx);
                this.update_title(window, cx);
                cx.notify();
            }),
            cx.observe_window_bounds(window, |_, window, cx| {
                AppState::remember_window(window.window_bounds(), cx);
            }),
        ];
        AppState::remember_window(window.window_bounds(), cx);
        AppState::remember_panels(&commander, cx);

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

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
            _subscriptions: subscriptions,
        };
        this.update_title(window, cx);
        // Startup's reads: the observer above starts them.
        this.commander.update(cx, |_, cx| cx.notify());
        this
    }

    fn update_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&window_title(self.commander.read(cx)));
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
    fn end_search(&mut self, cx: &mut Context<Self>) {
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
            .on_action(cx.listener(|this, _: &Reload, _, cx| this.execute(Command::Reload, cx)))
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
