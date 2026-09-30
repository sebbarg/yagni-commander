//! The root view: two panels side by side with a draggable divider, a status
//! line, and the keyboard actions that drive the shared [`Commander`].

use gpui_kit::{
    App, Context, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, SharedString,
    Subscription, Window, div, prelude::*, px, relative,
};
use yagni_commander_core::{Command, Commander, Side};

use crate::actions::{
    Activate, CursorDown, CursorEnd, CursorHome, CursorUp, FILE_MANAGER_CONTEXT, GoUp, PageDown,
    PageUp, SelectAll, SwitchPanel, ToggleSelection,
};
use crate::panel_view::PanelView;
use crate::theme::Theme;
use crate::window_state::WindowState;

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
            cx.observe_in(&commander, window, |this, _, window, cx| {
                this.update_title(window, cx);
                cx.notify();
            }),
            cx.observe_window_bounds(window, |_, window, cx| {
                WindowState::remember(window.window_bounds(), cx);
            }),
        ];
        WindowState::remember(window.window_bounds(), cx);

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
            _subscriptions: subscriptions,
        };
        this.update_title(window, cx);
        this
    }

    fn update_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&window_title(self.commander.read(cx)));
    }

    fn execute(&mut self, command: Command, cx: &mut Context<Self>) {
        self.notice = None;
        execute(&self.commander, command, cx);
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
        let theme = Theme::get(cx);

        let divider = div()
            .id("divider")
            .w(px(DIVIDER_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .when(self.dragging_split, |d| d.bg(theme.accent))
            .hover(|d| d.bg(theme.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.dragging_split = true;
                    cx.notify();
                }),
            );

        let status = match (self.commander.read(cx).error(), &self.notice) {
            (Some(err), _) => div().text_color(theme.error).child(err.to_owned()),
            (None, Some(notice)) => div().text_color(theme.error).child(notice.clone()),
            (None, None) => div()
                .text_color(theme.text_dim)
                .child("Tab switch · ↑↓ move · Enter open · Backspace up"),
        };

        div()
            .key_context(FILE_MANAGER_CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &SwitchPanel, _, cx| this.execute(Command::SwitchPanel, cx)),
            )
            .on_action(cx.listener(|this, _: &CursorUp, _, cx| this.execute(Command::CursorUp, cx)))
            .on_action(
                cx.listener(|this, _: &CursorDown, _, cx| this.execute(Command::CursorDown, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorHome, _, cx| this.execute(Command::CursorHome, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CursorEnd, _, cx| this.execute(Command::CursorEnd, cx)),
            )
            .on_action(cx.listener(|this, _: &Activate, _, cx| this.execute(Command::Activate, cx)))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.execute(Command::GoUp, cx)))
            .on_action(cx.listener(|this, _: &ToggleSelection, _, cx| {
                this.execute(Command::ToggleSelection, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectAll, _, cx| this.execute(Command::SelectAll, cx)),
            )
            .on_action(cx.listener(|this, _: &PageUp, _, cx| this.page(false, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| this.page(true, cx)))
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
            .bg(theme.window_bg)
            .text_color(theme.text)
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
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};

    /// A file manager window on a directory with dirs `a`, `b` and file `f`,
    /// using the default keymap.
    fn open(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::write(tmp.path().join("f"), b"").unwrap();

        cx.update(|cx| {
            cx.set_global(Theme::tokyo_night());
            cx.set_global(WindowState::default());
            crate::actions::bind_default_keys(cx);
        });
        let commander = Commander::new(tmp.path(), tmp.path()).unwrap();
        let commander = cx.new(|_| commander);
        let (_, cx) = cx.add_window_view({
            let commander = commander.clone();
            |window, cx| FileManager::new(commander, None, window, cx)
        });
        (tmp, commander, cx)
    }

    fn cursor(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> usize {
        commander.read_with(cx, |c, _| c.panel(side).cursor())
    }

    #[gpui_kit::test]
    fn arrow_keys_home_and_end_move_the_cursor(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down down");
        assert_eq!(cursor(&commander, Side::Left, cx), 2);
        cx.simulate_keystrokes("up");
        assert_eq!(cursor(&commander, Side::Left, cx), 1);
        cx.simulate_keystrokes("end");
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
        cx.simulate_keystrokes("home");
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
    }

    #[gpui_kit::test]
    fn tab_switches_the_panel_that_keys_act_on(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("tab down");
        commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Right));
        assert_eq!(cursor(&commander, Side::Right, cx), 1);
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
        cx.simulate_keystrokes("tab");
        commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
    }

    #[gpui_kit::test]
    fn enter_and_backspace_navigate(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down down enter");
        commander.read_with(cx, |c, _| {
            assert_eq!(c.panel(Side::Left).path(), tmp.path().join("b"))
        });
        cx.simulate_keystrokes("backspace");
        commander.read_with(cx, |c, _| {
            assert_eq!(c.panel(Side::Left).path(), tmp.path());
            assert_eq!(c.panel(Side::Left).cursor_entry().unwrap().label, "b");
        });
    }

    #[gpui_kit::test]
    fn page_keys_move_by_more_than_one_row(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("pagedown");
        // The listing is shorter than a page, so the cursor lands on the last entry.
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
        cx.simulate_keystrokes("pageup");
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
    }

    #[gpui_kit::test]
    fn window_title_follows_the_active_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("tab down enter");
        let title = commander.read_with(cx, |c, _| window_title(c));
        assert_eq!(
            title,
            format!("{} - yagni-commander", tmp.path().join("a").display())
        );
        cx.simulate_keystrokes("tab");
        let title = commander.read_with(cx, |c, _| window_title(c));
        assert_eq!(title, format!("{} - yagni-commander", tmp.path().display()));
    }

    fn selected(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> Vec<String> {
        commander.read_with(cx, |c, _| {
            c.panel(side).selection().map(|e| e.label.clone()).collect()
        })
    }

    #[gpui_kit::test]
    fn space_selects_and_moves_down(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down space space");
        assert_eq!(selected(&commander, Side::Left, cx), ["a", "b"]);
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
        cx.simulate_keystrokes("up space");
        assert_eq!(selected(&commander, Side::Left, cx), ["a"]);
    }

    #[gpui_kit::test]
    fn ctrl_a_selects_all_in_active_panel(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("tab ctrl-a");
        assert_eq!(selected(&commander, Side::Right, cx), ["a", "b", "f"]);
        assert!(selected(&commander, Side::Left, cx).is_empty());
    }
}
