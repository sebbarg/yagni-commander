//! The root view: two panels side by side with a draggable divider, a status
//! line, and the keyboard actions that drive the shared [`Commander`].

use gpui_kit::{
    App, Context, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, SharedString,
    Subscription, Window, div, prelude::*, px, relative,
};
use yagni_commander_core::{Command, Commander, Side};

mod commands;
mod file_ops;

use crate::actions::{
    Activate, ButtonTest, CancelSearch, Copy, CursorDown, CursorEnd, CursorHome, CursorUp, Edit,
    EditNewFile, FILE_MANAGER_CONTEXT, GoUp, MakeDirectory, Move, PageDown, PageUp, Reload, Rename,
    SelectAll, SwapPanels, SwitchPanel, SyncOtherPanel, ToggleHidden, ToggleSelection, Trash,
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
                AppState::remember_window(window.window_bounds(), cx);
            }),
        ];
        AppState::remember_window(window.window_bounds(), cx);

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
            _subscriptions: subscriptions,
        };
        this.update_title(window, cx);
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
            .on_action(cx.listener(|this, _: &CancelSearch, _, cx| {
                this.commander.update(cx, |commander, cx| {
                    if commander.search_cancel() {
                        cx.notify();
                    }
                })
            }))
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
            .on_action(cx.listener(|this, _: &Copy, window, cx| {
                this.copy_or_move(file_ops::Kind::Copy, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Move, window, cx| {
                this.copy_or_move(file_ops::Kind::Move, window, cx)
            }))
            .on_action(cx.listener(|this, _: &Trash, window, cx| this.trash(window, cx)))
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
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};

    /// A file manager window on a directory with dirs `a`, `b`, file `f` and
    /// hidden file `.dot` (not shown), using the default keymap.
    fn open(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::write(tmp.path().join("f"), b"").unwrap();
        std::fs::write(tmp.path().join(".dot"), b"").unwrap();

        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Theme::default());
            cx.set_global(AppState::default());
            cx.set_global(crate::CurrentConfig(Default::default()));
            crate::actions::bind_default_keys(cx);
        });
        let commander = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        let commander = cx.new(|_| commander);
        // Wrapped in Root like the real window, so dialogs and notifications work.
        let (_, cx) = cx.add_window_view({
            let commander = commander.clone();
            |window, cx| {
                let view = cx.new(|cx| FileManager::new(commander, None, window, cx));
                gpui_kit::base::Root::new(view, window, cx)
            }
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

    fn path(
        commander: &Entity<Commander>,
        side: Side,
        cx: &VisualTestContext,
    ) -> std::path::PathBuf {
        commander.read_with(cx, |c, _| c.panel(side).path().to_path_buf())
    }

    #[gpui_kit::test]
    fn alt_z_shows_this_directory_in_the_other_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down enter alt-z");
        assert_eq!(path(&commander, Side::Right, cx), tmp.path().join("a"));
        commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
    }

    #[gpui_kit::test]
    fn ctrl_u_swaps_panels(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down enter ctrl-u");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
        assert_eq!(path(&commander, Side::Right, cx), tmp.path().join("a"));
    }

    #[gpui_kit::test]
    fn ctrl_r_reloads(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        std::fs::write(tmp.path().join("new"), b"").unwrap();
        cx.simulate_keystrokes("ctrl-r");
        let has_new = commander.read_with(cx, |c, _| {
            c.panel(Side::Left)
                .entries()
                .iter()
                .any(|e| e.label == "new")
        });
        assert!(has_new);
    }

    fn search(commander: &Entity<Commander>, cx: &VisualTestContext) -> Option<String> {
        commander.read_with(cx, |c, _| c.search().map(str::to_owned))
    }

    #[gpui_kit::test]
    fn typing_jumps_to_matching_name_and_ignores_misses(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("b");
        assert_eq!(cursor(&commander, Side::Left, cx), 2);
        cx.simulate_keystrokes("x");
        assert_eq!(
            cursor(&commander, Side::Left, cx),
            2,
            "no name starts with bx"
        );
        assert_eq!(search(&commander, cx).as_deref(), Some("b"));
        cx.simulate_keystrokes("escape f");
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
        cx.simulate_keystrokes("home");
        assert_eq!(search(&commander, cx), None, "other keys end the search");
    }

    #[gpui_kit::test]
    fn up_and_down_step_through_search_matches(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        std::fs::create_dir(tmp.path().join("bb")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"").unwrap();
        // .., a, b, bb, b.txt, f
        cx.simulate_keystrokes("ctrl-r b");
        assert_eq!(cursor(&commander, Side::Left, cx), 2);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 4);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 2, "wraps");
        cx.simulate_keystrokes("up");
        assert_eq!(cursor(&commander, Side::Left, cx), 4, "wraps back");
        cx.simulate_keystrokes("b down");
        assert_eq!(cursor(&commander, Side::Left, cx), 3, "only bb matches bb");
        assert_eq!(search(&commander, cx).as_deref(), Some("bb"));
    }

    #[gpui_kit::test]
    fn backspace_shortens_the_search_before_going_up(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down enter");
        let inside = tmp.path().join("a");
        std::fs::write(inside.join("xy"), b"").unwrap();
        cx.simulate_keystrokes("ctrl-r x y backspace");
        assert_eq!(search(&commander, cx).as_deref(), Some("x"));
        cx.simulate_keystrokes("backspace");
        assert_eq!(search(&commander, cx), None);
        assert_eq!(path(&commander, Side::Left, cx), inside, "still here");
        cx.simulate_keystrokes("backspace");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
    }

    #[gpui_kit::test]
    fn dot_starts_a_search_for_hidden_files(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("ctrl-. . d");
        let label = commander.read_with(cx, |c, _| {
            c.panel(Side::Left).cursor_entry().unwrap().label.clone()
        });
        assert_eq!(label, ".dot");
    }

    #[gpui_kit::test]
    fn f4_without_editor_does_not_crash(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down f4");
        cx.run_until_parked();
        // The error box takes the keyboard until dismissed.
        cx.simulate_keystrokes("down escape");
        cx.run_until_parked();
        assert_eq!(cursor(&commander, Side::Left, cx), 1);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 2);
    }

    #[gpui_kit::test]
    fn f7_creates_directory_from_dialog(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("f7");
        cx.run_until_parked();
        cx.simulate_input("made/deep");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(tmp.path().join("made/deep").is_dir());
        let under_cursor = commander.read_with(cx, |c, _| {
            c.panel(Side::Left).cursor_entry().unwrap().label.clone()
        });
        assert_eq!(under_cursor, "made");
        // Focus is back on the panels.
        cx.simulate_keystrokes("home");
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
    }

    #[gpui_kit::test]
    fn f2_replaces_preselected_name_and_escape_cancels(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("notes.txt"), b"").unwrap();
        cx.simulate_keystrokes("ctrl-r n f2");
        cx.run_until_parked();
        cx.simulate_input("ideas");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(tmp.path().join("ideas.txt").exists());

        cx.simulate_keystrokes("f2");
        cx.run_until_parked();
        cx.simulate_input("other");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(tmp.path().join("ideas.txt").exists());
        assert!(!tmp.path().join("other.txt").exists());
    }

    #[gpui_kit::test]
    fn f2_onto_existing_name_keeps_both(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("down f2");
        cx.run_until_parked();
        cx.simulate_input("b");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(tmp.path().join("a").is_dir());
        assert!(tmp.path().join("b").is_dir());
    }

    #[gpui_kit::test]
    fn enter_submits_a_prompt_exactly_once(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        let view = file_manager(cx);
        let submits = std::rc::Rc::new(std::cell::Cell::new(0));
        view.update_in(cx, |this, window, cx| {
            let submits = submits.clone();
            this.prompt_name(
                commands::Prompt {
                    title: "Test",
                    error_title: "Test failed",
                    initial: "value",
                    selection: 0..0,
                },
                std::rc::Rc::new(move |_, _, _, _| {
                    submits.set(submits.get() + 1);
                    Ok(())
                }),
                window,
                cx,
            );
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(submits.get(), 1);
    }

    fn dialog_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| {
            use gpui_kit::component::WindowExt;
            window.has_active_dialog(cx)
        })
    }

    #[gpui_kit::test]
    fn errors_stay_until_dismissed_then_prompt_can_be_corrected(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("down f2");
        cx.run_until_parked();
        cx.simulate_input("b");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        // Still there long after a toast would have faded.
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(30));
        cx.run_until_parked();
        assert!(dialog_open(cx), "error box is showing");

        // Dismiss the error; the prompt is still open with focus, so the name
        // can be corrected and submitted.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(dialog_open(cx), "rename prompt is still open");
        cx.simulate_keystrokes("backspace");
        cx.simulate_input("fixed");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let names: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(!dialog_open(cx), "dialog still open; entries: {names:?}");
        assert!(tmp.path().join("fixed").is_dir());
        assert!(tmp.path().join("b").is_dir());
    }

    #[gpui_kit::test]
    fn escape_on_error_also_returns_to_the_prompt(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("down f2");
        cx.run_until_parked();
        cx.simulate_input("b");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(dialog_open(cx), "only the error box closed");
        cx.simulate_keystrokes("backspace");
        cx.simulate_input("c");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(tmp.path().join("c").is_dir());
    }

    #[gpui_kit::test]
    fn ctrl_dot_toggles_hidden_files_in_both_panels_and_remembers_it(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        let labels = |cx: &VisualTestContext, side| -> Vec<String> {
            commander.read_with(cx, |c, _| {
                c.panel(side)
                    .entries()
                    .iter()
                    .map(|e| e.label.clone())
                    .collect()
            })
        };
        let remembered = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| cx.global::<AppState>().state.show_hidden)
        };
        assert_eq!(labels(cx, Side::Left), ["..", "a", "b", "f"]);

        cx.simulate_keystrokes("ctrl-.");
        assert_eq!(labels(cx, Side::Left), ["..", "a", "b", ".dot", "f"]);
        assert_eq!(labels(cx, Side::Right), ["..", "a", "b", ".dot", "f"]);
        assert!(remembered(cx));

        cx.simulate_keystrokes("ctrl-.");
        assert_eq!(labels(cx, Side::Right), ["..", "a", "b", "f"]);
        assert!(!remembered(cx));
    }

    fn set_editor(editor: &str, cx: &mut VisualTestContext) {
        cx.update(|_, cx| {
            cx.set_global(crate::CurrentConfig(yagni_commander_core::Config {
                editor: Some(editor.to_owned()),
                ..Default::default()
            }))
        });
    }

    #[gpui_kit::test]
    fn shift_f4_creates_the_file_and_opens_the_editor(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        set_editor("true", cx); // exits at once, like a detached GUI editor
        cx.simulate_keystrokes("shift-f4");
        cx.run_until_parked();
        cx.simulate_input("todo.md");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(tmp.path().join("todo.md").is_file());
        let under_cursor = commander.read_with(cx, |c, _| {
            c.panel(Side::Left).cursor_entry().unwrap().label.clone()
        });
        assert_eq!(under_cursor, "todo.md");
    }

    #[gpui_kit::test]
    fn shift_f4_without_editor_shows_an_error_instead_of_a_prompt(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("shift-f4");
        cx.run_until_parked();
        assert!(dialog_open(cx), "error box");
        cx.simulate_input("x");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(!tmp.path().join("x").exists());
    }

    fn file_manager(cx: &mut VisualTestContext) -> Entity<FileManager> {
        cx.update(|window, cx| {
            window
                .root::<gpui_kit::base::Root>()
                .flatten()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<FileManager>()
                .unwrap()
        })
    }

    /// Lets the job's worker thread and the polling timer run until `done`.
    fn wait_until(cx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(50));
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out");
    }

    fn job_running(cx: &mut VisualTestContext) -> bool {
        let view = file_manager(cx);
        view.read_with(cx, |this, _| this.job.is_some())
    }

    /// Right panel in `a`, left cursor on `f`.
    fn open_with_target(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let (tmp, commander, cx) = open(cx);
        std::fs::write(tmp.path().join("f"), b"new").unwrap();
        cx.simulate_keystrokes("tab down enter tab end");
        (tmp, commander, cx)
    }

    #[gpui_kit::test]
    fn f5_copies_to_the_other_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open_with_target(cx);
        cx.simulate_keystrokes("space f5");
        cx.run_until_parked();
        assert!(dialog_open(cx), "destination prompt");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!dialog_open(cx));
        assert_eq!(std::fs::read(tmp.path().join("a/f")).unwrap(), b"new");
        assert!(tmp.path().join("f").exists());
        assert!(
            selected(&commander, Side::Left, cx).is_empty(),
            "copied, so deselected"
        );
        let right: Vec<_> = commander.read_with(cx, |c, _| {
            c.panel(Side::Right)
                .entries()
                .iter()
                .map(|e| e.label.clone())
                .collect()
        });
        assert_eq!(right, ["..", "f"], "reloaded");
    }

    #[gpui_kit::test]
    fn f6_moves_the_selection_to_a_typed_directory(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        cx.simulate_keystrokes("space f6");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.dispatch_action(Box::new(gpui_kit::component::input::SelectAll), cx)
        });
        cx.simulate_input("b");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(tmp.path().join("b/f").exists());
        assert!(!tmp.path().join("f").exists());
    }

    #[gpui_kit::test]
    fn conflicts_are_asked_enter_skips_and_escape_cancels(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        std::fs::write(tmp.path().join("a/f"), b"old").unwrap();
        for key in ["enter", "escape"] {
            cx.simulate_keystrokes("f5");
            cx.run_until_parked();
            cx.simulate_keystrokes("enter");
            wait_until(cx, dialog_open);
            assert!(job_running(cx), "waiting for the answer");
            cx.simulate_keystrokes(key);
            wait_until(cx, |cx| !job_running(cx));
            cx.run_until_parked();
            assert!(!dialog_open(cx), "progress closed after {key}");
            assert_eq!(std::fs::read(tmp.path().join("a/f")).unwrap(), b"old");
        }
    }

    #[gpui_kit::test]
    fn f5_to_the_same_directory_is_refused_in_the_prompt(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("end f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(dialog_open(cx), "error box");
        assert!(!job_running(cx));
        cx.simulate_keystrokes("escape escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 4);
    }

    #[gpui_kit::test]
    fn f8_and_delete_ask_before_trashing(cx: &mut TestAppContext) {
        // Never confirm here: that would use the real trash.
        let (tmp, _commander, cx) = open(cx);
        for key in ["f8", "delete"] {
            cx.simulate_keystrokes(&format!("end {key}"));
            cx.run_until_parked();
            assert!(dialog_open(cx), "{key} asks");
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(!dialog_open(cx));
            assert!(tmp.path().join("f").exists());
        }
        cx.simulate_keystrokes("home f8");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "nothing to trash on ..");
    }

    /// F5 of `f` onto an existing `a/f`, up to the conflict dialog.
    fn conflict(tmp: &tempfile::TempDir, cx: &mut VisualTestContext) {
        std::fs::write(tmp.path().join("a/f"), b"old").unwrap();
        cx.simulate_keystrokes("f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, dialog_open);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn conflict_buttons_follow_arrows_tab_space_and_enter(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        let target = tmp.path().join("a/f");
        // Buttons: Overwrite, Overwrite all, Skip (selected), Skip all, Cancel.
        for (keys, expected) in [
            ("left left enter", "new"),
            ("shift-tab shift-tab space", "new"),
            ("right right enter", "old"),
            ("tab left enter", "old"),
            (
                "left right right right right right left left left enter",
                "new",
            ),
        ] {
            conflict(&tmp, cx);
            cx.simulate_keystrokes(keys);
            wait_until(cx, |cx| !job_running(cx));
            cx.run_until_parked();
            assert!(!dialog_open(cx), "{keys}");
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                expected,
                "{keys}"
            );
        }
    }

    #[gpui_kit::test]
    fn f8_left_then_enter_cancels(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("end f8");
        cx.run_until_parked();
        cx.simulate_keystrokes("left enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(!job_running(cx));
        assert!(tmp.path().join("f").exists());
    }

    #[gpui_kit::test]
    fn prompt_arrows_edit_text_and_tab_reaches_the_buttons(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("f7");
        cx.run_until_parked();
        cx.simulate_input("ab");
        cx.simulate_keystrokes("left");
        cx.simulate_input("X");
        cx.simulate_keystrokes("tab left enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "Cancel closed it");
        assert!(!tmp.path().join("aXb").exists());

        cx.simulate_keystrokes("f7");
        cx.run_until_parked();
        cx.simulate_input("ab");
        cx.simulate_keystrokes("left");
        cx.simulate_input("X");
        cx.simulate_keystrokes("tab enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(tmp.path().join("aXb").is_dir(), "OK is preselected");
        // Focus is back on the panels.
        cx.simulate_keystrokes("home");
        assert_eq!(cursor(&commander_of(cx), Side::Left, cx), 0);
    }

    fn commander_of(cx: &mut VisualTestContext) -> Entity<Commander> {
        let view = file_manager(cx);
        view.read_with(cx, |this, _| this.commander.clone())
    }
}
