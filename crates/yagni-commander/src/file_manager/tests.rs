use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};
use yagni_commander_core::watch::Report;

/// The globals and keymap a file manager window needs.
fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        // Animations run on wall-clock time, not the test clock: a dialog
        // sliding in would move between a test's frames, and clicks would
        // miss under load. Reduced motion settles them on the first frame.
        cx.set_reduce_motion(true);
        crate::fonts::register(cx);
        cx.set_global(Theme::default());
        cx.set_global(AppState::default());
        cx.set_global(crate::config_state::CurrentConfig::default());
        crate::actions::bind_default_keys(cx);
    });
}

/// A file manager window on `commander`, wrapped in Root like the real
/// window so dialogs and notifications work. Reads run on threads.
fn window_on(commander: Entity<Commander>, cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| FileManager::new(commander, None, window, cx));
        gpui_kit::base::Root::new(view, window, cx)
    });
    // No real watchers (see `FileManager::new`): tests report changes with
    // `changed`. Enter on a file must never start a real application.
    file_manager(cx).update(cx, |this, _| {
        this.opener = "true".into();
        this.terminal = Some("true".into());
        this.temp_dir = None;
    });
    cx
}

/// A file manager window on a directory with dirs `a`, `b`, file `f` and
/// hidden file `.dot` (not shown), using the default keymap. Directory reads
/// run inline, so keys take effect at once.
fn open(cx: &mut TestAppContext) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("a")).unwrap();
    std::fs::create_dir(tmp.path().join("b")).unwrap();
    std::fs::write(tmp.path().join("f"), b"").unwrap();
    std::fs::write(tmp.path().join(".dot"), b"").unwrap();

    setup(cx);
    let commander = Commander::new(tmp.path(), tmp.path(), false).unwrap();
    let commander = cx.new(|_| commander);
    let cx = window_on(commander.clone(), cx);
    file_manager(cx).update(cx, |this, _| this.load = None);
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
fn shift_tab_also_switches_panels(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("shift-tab");
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Right));
    cx.simulate_keystrokes("shift-tab");
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
}

fn tab_state(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> (usize, usize) {
    commander.read_with(cx, |c, _| (c.tabs(side).count(), c.tabs(side).index()))
}

#[gpui_kit::test]
fn ctrl_t_opens_a_tab_on_the_same_folder_and_ctrl_w_closes_it(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-w");
    assert_eq!(
        tab_state(&commander, Side::Left, cx),
        (1, 0),
        "the last tab stays"
    );
    cx.simulate_keystrokes("down ctrl-t");
    assert_eq!(tab_state(&commander, Side::Left, cx), (2, 1));
    assert_eq!(path(&commander, Side::Left, cx), tmp.path());
    assert_eq!(cursor(&commander, Side::Left, cx), 1);
    cx.simulate_keystrokes("ctrl-w");
    assert_eq!(tab_state(&commander, Side::Left, cx), (1, 0));
}

#[gpui_kit::test]
fn ctrl_tab_and_ctrl_shift_tab_cycle_the_active_sides_tabs(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-t down enter"); // the second tab enters "a"
    cx.simulate_keystrokes("ctrl-tab");
    assert_eq!(
        path(&commander, Side::Left, cx),
        tmp.path(),
        "wrapped to the first"
    );
    cx.simulate_keystrokes("ctrl-shift-tab");
    assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
    assert_eq!(tab_state(&commander, Side::Right, cx), (1, 0));
}

#[gpui_kit::test]
fn a_background_tab_shows_changes_when_it_comes_to_the_front(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-t ctrl-tab");
    std::fs::write(tmp.path().join("new"), b"").unwrap();
    cx.simulate_keystrokes("ctrl-tab");
    cx.run_until_parked();
    assert!(labels(&commander, Side::Left, cx).contains(&"new".to_string()));
}

#[gpui_kit::test]
fn the_watcher_follows_the_tab_in_front(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    // The right side shows `b`, so each watched folder is one side's.
    cx.simulate_keystrokes("tab down down enter tab");
    cx.simulate_keystrokes("ctrl-t down enter");
    let watches = |cx: &mut VisualTestContext, dir: std::path::PathBuf| {
        file_manager(cx).read_with(cx, |this, _| this.watched.contains(&Some(dir)))
    };
    assert!(watches(cx, tmp.path().join("a")));
    assert!(!watches(cx, tmp.path().to_path_buf()));
    cx.simulate_keystrokes("ctrl-tab");
    assert!(watches(cx, tmp.path().to_path_buf()));
    assert!(watches(cx, tmp.path().join("b")));
}

#[gpui_kit::test]
fn a_tab_stuck_loading_can_be_left_and_closed(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-t");
    enter_held_a(&tmp, cx);
    assert!(loading(&commander, cx));
    cx.simulate_keystrokes("ctrl-t");
    assert_eq!(
        tab_state(&commander, Side::Left, cx),
        (2, 1),
        "no copy of a loading tab"
    );
    cx.simulate_keystrokes("ctrl-tab");
    assert!(!loading(&commander, cx));
    cx.simulate_keystrokes("ctrl-tab ctrl-w");
    assert_eq!(tab_state(&commander, Side::Left, cx), (1, 0));
    assert!(!loading(&commander, cx));
    release(&tmp);
    wait_until(cx, |cx| {
        file_manager(cx).read_with(cx, |this, _| this.loads.is_empty())
    });
    assert_eq!(
        path(&commander, Side::Left, cx),
        tmp.path(),
        "the late result is dropped"
    );
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

fn path(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> std::path::PathBuf {
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
                width: super::commands::PROMPT_WIDTH,
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
    let remembered =
        |cx: &mut VisualTestContext| cx.update(|_, cx| cx.global::<AppState>().state.show_hidden);
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
        cx.set_global(crate::config_state::CurrentConfig {
            config: yagni_commander_core::Config {
                editor: Some(editor.to_owned()),
                ..Default::default()
            },
            ..Default::default()
        })
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
fn conflicts_are_asked_escape_cancels_and_enter_overwrites(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open_with_target(cx);
    std::fs::write(tmp.path().join("a/f"), b"old").unwrap();
    for (key, expected) in [("escape", "old"), ("enter", "new")] {
        cx.simulate_keystrokes("f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, dialog_open);
        assert!(job_running(cx), "waiting for the answer");
        cx.simulate_keystrokes(key);
        wait_until(cx, |cx| !job_running(cx));
        cx.run_until_parked();
        assert!(!dialog_open(cx), "progress closed after {key}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a/f")).unwrap(),
            expected,
            "{key}"
        );
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
fn f5_on_one_file_can_rename_the_copy(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open_with_target(cx);
    cx.simulate_keystrokes("f5");
    cx.run_until_parked();
    // The name is preselected: typing replaces it.
    cx.simulate_input("g");
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert_eq!(std::fs::read(tmp.path().join("a/g")).unwrap(), b"new");
    assert!(!tmp.path().join("a/f").exists());
}

#[gpui_kit::test]
fn f6_on_one_file_into_new_folders(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open_with_target(cx);
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    cx.simulate_input("x/y/g");
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert_eq!(std::fs::read(tmp.path().join("a/x/y/g")).unwrap(), b"new");
    assert!(!tmp.path().join("f").exists());
}

#[gpui_kit::test]
fn f5_on_one_folder_copies_it_under_a_new_name(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    std::fs::write(tmp.path().join("b/inner"), b"i").unwrap();
    // Left on b, right panel in a.
    cx.simulate_keystrokes("tab down enter tab down down f5");
    cx.run_until_parked();
    cx.simulate_input("copied");
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert_eq!(
        std::fs::read(tmp.path().join("a/copied/inner")).unwrap(),
        b"i"
    );
}

#[gpui_kit::test]
fn f5_on_many_keeps_names(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open_with_target(cx);
    std::fs::write(tmp.path().join("g"), b"g").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    // Select f and g (the last two entries).
    cx.simulate_keystrokes("end space up space f5");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert!(tmp.path().join("a/f").exists());
    assert!(tmp.path().join("a/g").exists());
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
    // Buttons: Overwrite (selected), Overwrite all, Skip, Skip all, Cancel.
    for (keys, expected) in [
        ("enter", "new"),
        ("left enter", "new"),
        ("right right enter", "old"),
        ("tab tab space", "old"),
        ("right right right right enter", "old"),
        (
            "right right right right right left left left left enter",
            "new",
        ),
        ("tab tab shift-tab shift-tab space", "new"),
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

// Mouse. Elements are found by their `debug_selector` (see panel_view.rs).

fn bounds(
    cx: &mut VisualTestContext,
    selector: String,
) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    cx.run_until_parked();
    cx.debug_bounds(selector.leak())
}

fn center(cx: &mut VisualTestContext, selector: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    bounds(cx, selector.to_owned())
        .unwrap_or_else(|| panic!("{selector} not drawn"))
        .center()
}

fn click(cx: &mut VisualTestContext, selector: &str, click_count: usize) {
    let position = center(cx, selector);
    click_at(cx, position, click_count);
}

fn click_at(
    cx: &mut VisualTestContext,
    position: gpui_kit::Point<gpui_kit::Pixels>,
    click_count: usize,
) {
    let modifiers = gpui_kit::Modifiers::default();
    let button = gpui_kit::MouseButton::Left;
    cx.simulate_event(gpui_kit::MouseDownEvent {
        position,
        modifiers,
        button,
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(gpui_kit::MouseUpEvent {
        position,
        modifiers,
        button,
        click_count,
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn clicking_a_row_moves_the_cursor_and_activates_that_panel(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    click(cx, "row-right-2", 1);
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Right));
    assert_eq!(cursor(&commander, Side::Right, cx), 2);
    assert_eq!(cursor(&commander, Side::Left, cx), 0);
    // Keys now act on the clicked panel.
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&commander, Side::Right, cx), 3);
}

#[gpui_kit::test]
fn both_sides_draw_a_tab_header_even_with_one_tab(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    assert!(bounds(cx, "tab-left-0".into()).is_some());
    assert!(bounds(cx, "tab-right-0".into()).is_some());
    assert!(bounds(cx, "tab-left-1".into()).is_none());
    cx.simulate_keystrokes("ctrl-t");
    assert!(bounds(cx, "tab-left-1".into()).is_some());
}

#[gpui_kit::test]
fn the_tab_header_sits_above_the_path_header(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let tab = bounds(cx, "tab-left-0".into()).unwrap();
    let header = bounds(cx, "header-left-Name".into()).unwrap();
    assert!(tab.bottom() <= header.top());
}

#[gpui_kit::test]
fn the_first_tab_lines_up_with_the_panels_left_edge(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    for side in ["left", "right"] {
        let tab = bounds(cx, format!("tab-{side}-0")).unwrap();
        // Rows sit inside the panel's 1 px border.
        let row = bounds(cx, format!("row-{side}-0")).unwrap();
        let gap = f32::from(row.left()) - f32::from(tab.left());
        assert!((0.0..=1.0).contains(&gap), "{side}: {gap}");
    }
}

#[gpui_kit::test]
fn the_path_header_starts_with_a_folder_glyph_while_icons_are_on(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    assert!(bounds(cx, "pwd-left".into()).is_some());
    assert!(bounds(cx, "pwd-right".into()).is_some());
    let pwd = bounds(cx, "pwd-left".into()).unwrap();
    let tab = bounds(cx, "tab-left-0".into()).unwrap();
    let names = bounds(cx, "header-left-Name".into()).unwrap();
    assert!(
        pwd.top() >= tab.bottom() && pwd.bottom() <= names.top(),
        "in the path header"
    );
    commander.update(cx, |c, cx| {
        c.set_icons(false);
        cx.notify();
    });
    assert!(bounds(cx, "pwd-left".into()).is_none());
}

#[gpui_kit::test]
fn clicking_a_tab_brings_it_and_its_side_to_the_front(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("ctrl-t down enter tab"); // left: [root, a]; right active
    click(cx, "tab-left-0", 1);
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
    assert_eq!(tab_state(&commander, Side::Left, cx), (2, 0));
    assert_eq!(path(&commander, Side::Left, cx), tmp.path());
}

#[gpui_kit::test]
fn tabs_shrink_evenly_to_fit(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let one = bounds(cx, "tab-left-0".into()).unwrap();
    // Twelve tabs: more than fit at full width in a test window, few enough
    // that each still has room for its padding.
    for _ in 0..11 {
        cx.simulate_keystrokes("ctrl-t");
    }
    let first = bounds(cx, "tab-left-0".into()).unwrap();
    let last = bounds(cx, "tab-left-11".into()).unwrap();
    let rows = bounds(cx, "row-left-0".into()).unwrap();
    assert!(
        last.right() <= rows.right() + gpui_kit::px(1.0),
        "nothing overflows"
    );
    assert!(first.size.width < one.size.width, "they shrank");
    let (a, b) = (f32::from(first.size.width), f32::from(last.size.width));
    assert!((a - b).abs() < 1.0, "evenly");
}

#[gpui_kit::test]
fn double_click_opens_a_directory_and_goes_up_on_parent(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    click(cx, "row-left-1", 2);
    assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
    click(cx, "row-left-0", 2);
    assert_eq!(path(&commander, Side::Left, cx), tmp.path());
}

#[gpui_kit::test]
fn clicking_a_header_sorts_and_clicking_again_reverses(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let sort = |cx: &VisualTestContext| commander.read_with(cx, |c, _| c.panel(Side::Right).sort());
    click(cx, "header-right-Size", 1);
    let first = sort(cx);
    assert_eq!(first.key, yagni_commander_core::SortKey::Size);
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Right));
    click(cx, "header-right-Size", 1);
    assert_eq!(sort(cx).descending, !first.descending);
    click(cx, "header-right-Name", 1);
    assert_eq!(sort(cx).key, yagni_commander_core::SortKey::Name);
}

#[gpui_kit::test]
fn wheel_scrolls_the_list_without_moving_the_cursor(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    for i in 0..200 {
        std::fs::write(tmp.path().join(format!("file{i:03}")), b"").unwrap();
    }
    cx.simulate_keystrokes("ctrl-r");
    assert!(bounds(cx, "row-left-0".into()).is_some());
    let position = center(cx, "row-left-3");
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position,
        delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, -60.0)),
        ..Default::default()
    });
    assert!(
        bounds(cx, "row-left-0".into()).is_none(),
        "first rows scrolled away"
    );
    assert_eq!(cursor(&commander, Side::Left, cx), 0);
}

#[gpui_kit::test]
fn a_reload_shifting_the_cursor_keeps_a_wheel_scrolled_view(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    for i in 0..200 {
        std::fs::write(tmp.path().join(format!("file{i:03}")), b"").unwrap();
    }
    cx.simulate_keystrokes("ctrl-r end");
    assert!(
        bounds(cx, "row-left-0".into()).is_none(),
        "the cursor at the end"
    );
    let position = center(cx, "row-left-190");
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position,
        delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0.0, 400.0)),
        ..Default::default()
    });
    assert!(
        bounds(cx, "row-left-0".into()).is_some(),
        "scrolled to the top"
    );
    // Entries before the cursor's: its index changes, its entry doesn't.
    std::fs::write(tmp.path().join("aaa"), b"").unwrap();
    changed(cx, Side::Left);
    assert_eq!(
        commander.read_with(cx, |c, _| c
            .panel(Side::Left)
            .cursor_entry()
            .unwrap()
            .label
            .clone()),
        "file199"
    );
    assert!(bounds(cx, "row-left-0".into()).is_some(), "the view stays");
    // A cursor in view is followed.
    cx.simulate_keystrokes("home");
    std::fs::write(tmp.path().join("aab"), b"").unwrap();
    changed(cx, Side::Left);
    assert!(bounds(cx, "row-left-0".into()).is_some());
}

#[gpui_kit::test]
fn dragging_the_divider_resizes_the_panels(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let before = bounds(cx, "row-left-0".into()).unwrap().size.width;
    let start = center(cx, "divider");
    let target = gpui_kit::point(start.x / 2.0, start.y);
    let modifiers = gpui_kit::Modifiers::default();
    let left = gpui_kit::MouseButton::Left;
    cx.simulate_mouse_down(start, left, modifiers);
    cx.simulate_mouse_move(target, left, modifiers);
    cx.simulate_mouse_up(target, left, modifiers);
    let after = bounds(cx, "row-left-0".into()).unwrap().size.width;
    assert!(after < before * 0.7, "{before:?} -> {after:?}");
    // Released: further moves change nothing.
    cx.simulate_mouse_move(start, None, modifiers);
    assert_eq!(bounds(cx, "row-left-0".into()).unwrap().size.width, after);
}

// Jobs with a fake trash: deletes, but first waits while a file named `hold`
// exists next to the entry, so a test can keep a job running.

fn held_trash(path: &std::path::Path) -> Result<(), String> {
    let hold = path.parent().unwrap().join("hold");
    let start = std::time::Instant::now();
    while hold.exists() && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    std::fs::remove_dir_all(path)
        .or_else(|_| std::fs::remove_file(path))
        .map_err(|e| e.to_string())
}

fn use_fake_trash(cx: &mut VisualTestContext) {
    let view = file_manager(cx);
    view.update(cx, |this, _| this.trash = held_trash);
}

fn progress_open(cx: &mut VisualTestContext) -> bool {
    let view = file_manager(cx);
    view.read_with(cx, |this, _| {
        this.job.as_ref().is_some_and(|j| j.progress_open())
    })
}

#[gpui_kit::test]
fn f8_confirmed_trashes_the_selection(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    use_fake_trash(cx);
    cx.simulate_keystrokes("down space space f8");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert!(!dialog_open(cx));
    assert!(!tmp.path().join("a").exists() && !tmp.path().join("b").exists());
    assert!(tmp.path().join("f").exists());
    let labels: Vec<_> = commander.read_with(cx, |c, _| {
        c.panel(Side::Left)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect()
    });
    assert_eq!(labels, ["..", "f"], "reloaded");
}

#[gpui_kit::test]
fn slow_job_shows_progress_and_cancel_stops_it(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    use_fake_trash(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    cx.simulate_keystrokes("ctrl-a f8");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, progress_open);
    assert!(dialog_open(cx) && job_running(cx));
    // The progress dialog's only button, Cancel, is selected.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let cancelling =
        file_manager(cx).read_with(cx, |this, cx| this.job.as_ref().unwrap().cancelling(cx));
    assert!(cancelling);
    assert!(job_running(cx), "waits for the worker to stop");
    std::fs::remove_file(tmp.path().join("hold")).unwrap();
    wait_until(cx, |cx| !job_running(cx));
    assert!(!dialog_open(cx), "progress closed");
    // The first entry was being trashed when Cancel came; the rest were left.
    let left = ["a", "b", "f"]
        .iter()
        .filter(|n| tmp.path().join(n).exists())
        .count();
    assert_eq!(left, 2);
}

#[gpui_kit::test]
fn escape_on_the_progress_dialog_cancels_too(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    use_fake_trash(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    cx.simulate_keystrokes("ctrl-a f8");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, progress_open);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(dialog_open(cx), "stays until the worker stops");
    std::fs::remove_file(tmp.path().join("hold")).unwrap();
    wait_until(cx, |cx| !job_running(cx));
    assert!(!dialog_open(cx));
    assert!(tmp.path().join("f").exists());
}

#[gpui_kit::test]
fn a_second_job_is_refused_while_one_runs(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    use_fake_trash(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    cx.simulate_keystrokes("down f8");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // Before the progress dialog opens, the panels still take keys.
    cx.simulate_keystrokes("down f8");
    cx.run_until_parked();
    assert!(dialog_open(cx), "error box");
    cx.simulate_keystrokes("enter");
    std::fs::remove_file(tmp.path().join("hold")).unwrap();
    wait_until(cx, |cx| !job_running(cx));
    cx.run_until_parked();
    assert!(!tmp.path().join("a").exists());
    assert!(tmp.path().join("b").exists(), "the refused job never ran");
}

#[cfg(unix)]
#[gpui_kit::test]
fn failures_are_summarised_after_the_job(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open_with_target(cx);
    // A socket can't be copied.
    let _socket = std::os::unix::net::UnixListener::bind(tmp.path().join("sock")).unwrap();
    cx.simulate_keystrokes("ctrl-r ctrl-a f5");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    cx.run_until_parked();
    assert!(dialog_open(cx), "error summary");
    assert!(tmp.path().join("a/f").exists(), "the rest was copied");
    let selected = selected(
        &file_manager(cx).read_with(cx, |this, _| this.commander.clone()),
        Side::Left,
        cx,
    );
    assert!(!selected.is_empty(), "selection kept after a failed copy");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
}

// Startup and odds and ends.

#[gpui_kit::test]
fn a_startup_notice_shows_until_the_next_command(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let view = file_manager(cx);
    view.update(cx, |this, _| {
        this.notice = Some("Config ignored: bad".into())
    });
    cx.simulate_keystrokes("down");
    assert!(view.read_with(cx, |this, _| this.notice.is_none()));
}

#[gpui_kit::test]
fn window_geometry_is_recorded_for_the_state_file(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let window = cx.update(|_, cx| cx.global::<AppState>().state.window);
    assert!(window.is_some());
}

#[gpui_kit::test]
fn f4_with_an_editor_shows_no_error(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    set_editor("true", cx);
    cx.simulate_keystrokes("end f4");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
}

#[gpui_kit::test]
fn reload_of_a_vanished_directory_shows_the_error(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("down enter");
    std::fs::remove_dir(tmp.path().join("a")).unwrap();
    cx.simulate_keystrokes("ctrl-r");
    assert!(commander.read_with(cx, |c, _| c.error().is_some()));
}

#[gpui_kit::test]
fn the_search_box_is_drawn_only_in_the_active_panel_while_searching(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    assert!(bounds(cx, "search-left".into()).is_none());
    cx.simulate_keystrokes("b");
    assert!(bounds(cx, "search-left".into()).is_some());
    assert!(bounds(cx, "search-right".into()).is_none());
    cx.simulate_keystrokes("escape");
    assert!(bounds(cx, "search-left".into()).is_none());
}

#[gpui_kit::test]
fn commands_that_open_a_dialog_close_the_search_box(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    set_editor("true", cx);
    for key in ["f2", "f4", "f5", "f6", "f7", "f8", "shift-f8", "shift-f4"] {
        cx.simulate_keystrokes("f");
        assert_eq!(search(&commander, cx).as_deref(), Some("f"), "{key}");
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        assert_eq!(search(&commander, cx), None, "{key} ends the search");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "{key}");
    }
}

#[gpui_kit::test]
fn shift_f8_deletes_permanently_after_confirmation(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    std::fs::write(tmp.path().join("a/inside"), b"").unwrap();
    // If this went to the trash, the fake would block on `hold` and time out.
    use_fake_trash(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    cx.simulate_keystrokes("down shift-f8");
    cx.run_until_parked();
    assert!(dialog_open(cx));
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    assert!(!dialog_open(cx));
    assert!(!tmp.path().join("a").exists());
    assert_eq!(cursor(&commander, Side::Left, cx), 1, "stays on the row");
}

#[gpui_kit::test]
fn shift_delete_asks_and_left_enter_cancels(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("end shift-delete");
    cx.run_until_parked();
    assert!(dialog_open(cx));
    cx.simulate_keystrokes("left enter");
    cx.run_until_parked();
    assert!(!dialog_open(cx) && !job_running(cx));
    assert!(tmp.path().join("f").exists());
}

#[gpui_kit::test]
fn background_jobs_write_to_the_operation_log(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open_with_target(cx);
    let logs = tempfile::tempdir().unwrap();
    let log = yagni_commander_core::oplog::OperationLog::open(logs.path()).unwrap();
    commander.update(cx, |c, _| c.set_log(Some(std::sync::Arc::new(log))));
    cx.simulate_keystrokes("f5");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_until(cx, |cx| !job_running(cx));
    let file = std::fs::read_dir(logs.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let text = std::fs::read_to_string(file.path()).unwrap();
    let copied = format!(
        "copy copied {} -> {}",
        tmp.path().join("f").display(),
        tmp.path().join("a/f").display()
    );
    assert!(text.contains(&copied), "{text}");
    assert!(text.contains("copy finished: 0 failed, 0 skipped"));
}

#[gpui_kit::test]
fn shift_f4_creates_missing_folders_and_selects_the_first(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    set_editor("true", cx);
    cx.simulate_keystrokes("shift-f4");
    cx.run_until_parked();
    cx.simulate_input("notes/2026/todo.md");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
    assert!(tmp.path().join("notes/2026/todo.md").is_file());
    let under_cursor = commander.read_with(cx, |c, _| {
        c.panel(Side::Left).cursor_entry().unwrap().label.clone()
    });
    assert_eq!(under_cursor, "notes");
}

fn viewers(cx: &mut VisualTestContext) -> usize {
    let windows = cx.windows();
    windows
        .into_iter()
        .filter(|w| crate::viewer_view::tests::viewer_in(*w, cx).is_some())
        .count()
}

#[gpui_kit::test]
fn f3_opens_a_viewer_window_per_file(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("end f3"); // "f"
    cx.run_until_parked();
    assert_eq!(viewers(cx), 1);
    cx.simulate_keystrokes("f3");
    cx.run_until_parked();
    assert_eq!(viewers(cx), 2);
}

#[gpui_kit::test]
fn f3_on_a_directory_or_parent_does_nothing(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("f3 down f3"); // ".." then "a"
    cx.run_until_parked();
    assert_eq!(viewers(cx), 0);
    assert!(!dialog_open(cx));
}

#[gpui_kit::test]
fn f3_on_a_fifo_shows_an_error(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    std::process::Command::new("mkfifo")
        .arg(tmp.path().join("pipe"))
        .status()
        .unwrap();
    cx.simulate_keystrokes("ctrl-r end f3"); // "pipe" sorts after "f"
    cx.run_until_parked();
    assert_eq!(viewers(cx), 0);
    assert!(dialog_open(cx));
    let _ = commander;
}

#[gpui_kit::test]
fn panel_tabs_and_active_side_are_remembered(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    let state = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let s = &cx.global::<AppState>().state;
            (
                s.left_tabs.clone(),
                s.left_tab,
                s.right_tabs.clone(),
                s.active,
            )
        })
    };
    let root = tmp.path().to_path_buf();
    assert_eq!(
        state(cx),
        (vec![root.clone()], 0, vec![root.clone()], Some(Side::Left))
    );
    cx.simulate_keystrokes("ctrl-t down enter tab");
    cx.run_until_parked();
    assert_eq!(
        state(cx),
        (
            vec![root.clone(), root.join("a")],
            1,
            vec![root],
            Some(Side::Right)
        )
    );
}

// Loads that wait while a file named `hold` exists in the folder being read,
// so a test can keep a panel loading.

fn held_load(
    request: yagni_commander_core::LoadRequest,
) -> std::sync::mpsc::Receiver<std::io::Result<yagni_commander_core::Listing>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let hold = request.path.join("hold");
        let start = std::time::Instant::now();
        while hold.exists() && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let _ = tx.send(yagni_commander_core::read_listing(&request));
    });
    rx
}

fn use_held_loads(cx: &mut VisualTestContext) {
    file_manager(cx).update(cx, |this, _| this.load = Some(held_load));
}

/// Enters "a" while `a/hold` keeps the load pending.
fn enter_held_a(tmp: &tempfile::TempDir, cx: &mut VisualTestContext) {
    use_held_loads(cx);
    std::fs::write(tmp.path().join("a/hold"), b"").unwrap();
    cx.simulate_keystrokes("down enter");
}

fn release(tmp: &tempfile::TempDir) {
    let _ = std::fs::remove_file(tmp.path().join("a/hold"));
}

fn loading(commander: &Entity<Commander>, cx: &VisualTestContext) -> bool {
    commander.read_with(cx, |c, _| c.panel(Side::Left).loading().is_some())
}

/// The watcher reports a change in `side`'s folder.
fn changed(cx: &mut VisualTestContext, side: Side) {
    let dir = commander_of(cx).read_with(cx, |c, _| c.panel(side).real_dir().to_path_buf());
    report(cx, Report::Changed(dir));
}

fn report(cx: &mut VisualTestContext, report: Report) {
    file_manager(cx).update(cx, |this, _| this.changes.unbounded_send(report).unwrap());
    cx.run_until_parked();
}

fn labels(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> Vec<String> {
    commander.read_with(cx, |c, _| {
        c.panel(side)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect()
    })
}

fn refreshing(commander: &Entity<Commander>, cx: &VisualTestContext) -> bool {
    commander.read_with(cx, |c, _| c.panel(Side::Left).is_refreshing())
}

#[gpui_kit::test]
fn a_change_outside_reloads_the_panel(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("tab down enter tab"); // right in `a`
    std::fs::write(tmp.path().join("new"), b"").unwrap();
    changed(cx, Side::Left);
    assert!(labels(&commander, Side::Left, cx).contains(&"new".to_owned()));
    assert!(
        !labels(&commander, Side::Right, cx).contains(&"new".to_owned()),
        "only that side"
    );
    assert!(!loading(&commander, cx));
}

#[gpui_kit::test]
fn keys_and_dialogs_work_during_a_quiet_reload(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    use_held_loads(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    changed(cx, Side::Left);
    assert!(refreshing(&commander, cx));
    // Long enough for the "Loading..." indicator of a normal read.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert!(!loading(&commander, cx), "no indicator, nothing blocked");
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&commander, Side::Left, cx), 1);
    cx.simulate_keystrokes("f7");
    cx.run_until_parked();
    assert!(dialog_open(cx), "F7 is not blocked");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    std::fs::remove_file(tmp.path().join("hold")).unwrap();
    wait_until(cx, |cx| !refreshing(&commander, cx));
    let label = commander.read_with(cx, |c, _| {
        c.panel(Side::Left).cursor_entry().unwrap().label.clone()
    });
    assert_eq!(label, "a", "the cursor stayed where it was moved");
}

#[gpui_kit::test]
fn a_watch_start_rereads_a_folder_changed_since_its_listing(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("tab down enter tab"); // right in `a`
    let a = tmp.path().join("a");
    std::fs::write(a.join("new"), b"").unwrap();
    let modified = yagni_commander_core::dir_modified(&a);
    report(cx, Report::Started(a, modified));
    assert!(labels(&commander, Side::Right, cx).contains(&"new".to_owned()));
}

#[gpui_kit::test]
fn escape_stops_a_hung_quiet_read(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("tab down enter tab"); // right in `a`
    use_held_loads(cx);
    std::fs::write(tmp.path().join("hold"), b"").unwrap();
    changed(cx, Side::Left);
    assert!(refreshing(&commander, cx));
    cx.simulate_keystrokes("escape");
    assert!(!refreshing(&commander, cx));
    wait_until(cx, |cx| {
        file_manager(cx).read_with(cx, |this, _| this.loads.is_empty())
    });
    std::fs::remove_file(tmp.path().join("hold")).unwrap();
}

#[gpui_kit::test]
fn after_a_slow_quiet_read_the_next_one_waits(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("tab down enter tab"); // right in `a`
    use_held_loads(cx);
    let hold = tmp.path().join("hold");
    std::fs::write(&hold, b"").unwrap();
    changed(cx, Side::Left);
    for _ in 0..10 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
    }
    std::fs::remove_file(&hold).unwrap();
    wait_until(cx, |cx| !refreshing(&commander, cx));
    // The next one is held back about as long (500 ms on the test clock).
    std::fs::write(tmp.path().join("new"), b"").unwrap();
    changed(cx, Side::Left);
    for _ in 0..4 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(refreshing(&commander, cx), "still waiting");
    assert!(!labels(&commander, Side::Left, cx).contains(&"new".to_owned()));
    wait_until(cx, |cx| !refreshing(&commander, cx));
    assert!(labels(&commander, Side::Left, cx).contains(&"new".to_owned()));
}

#[gpui_kit::test]
fn a_restored_tab_is_watched_only_once_read(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    std::fs::create_dir(&a).unwrap();
    setup(cx);
    let commander = Commander::start_tabs(
        yagni_commander_core::StartTabs {
            dirs: vec![tmp.path().to_path_buf(), a.clone()],
            active: 0,
        },
        yagni_commander_core::StartTabs::one(tmp.path().to_path_buf()),
        false,
        tmp.path().to_path_buf(),
    );
    let commander = cx.new(|_| commander);
    let cx = window_on(commander.clone(), cx);
    use_held_loads(cx);
    let loads_done = |cx: &mut VisualTestContext| {
        file_manager(cx).read_with(cx, |this, _| this.loads.is_empty())
    };
    wait_until(cx, loads_done);
    let watches = |cx: &mut VisualTestContext| {
        file_manager(cx).read_with(cx, |this, _| this.watched.contains(&Some(a.clone())))
    };
    std::fs::write(a.join("hold"), b"").unwrap();
    cx.simulate_keystrokes("ctrl-tab"); // its first read, held
    assert!(loading(&commander, cx));
    assert!(!watches(cx), "not before its read");
    std::fs::remove_file(a.join("hold")).unwrap();
    wait_until(cx, loads_done);
    assert!(watches(cx));
}

#[gpui_kit::test]
fn the_watchers_follow_the_panels(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    let watched =
        |cx: &mut VisualTestContext| file_manager(cx).read_with(cx, |this, _| this.watched.clone());
    let root = Some(tmp.path().to_path_buf());
    let a = Some(tmp.path().join("a"));
    // One folder on both sides is watched once.
    assert_eq!(watched(cx), [root.clone(), None]);
    cx.simulate_keystrokes("down enter");
    assert_eq!(watched(cx), [root.clone(), a.clone()]);
    // Ctrl-U moves no watch, so a change seen just before isn't lost.
    cx.simulate_keystrokes("ctrl-u");
    assert_eq!(watched(cx), [root.clone(), a.clone()]);
    // Ctrl-R watches both again (a symlinked folder may point elsewhere):
    // crossed watchers are assigned afresh, in side order.
    let crossed = [a.clone(), root.clone()];
    file_manager(cx).update(cx, |this, _| this.watched = crossed);
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    assert_eq!(watched(cx), [root, a]);
}

#[gpui_kit::test]
fn a_prompt_does_not_act_in_the_folder_a_vanished_one_left_for(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    set_editor("true", cx);
    let a = tmp.path().join("a");
    cx.simulate_keystrokes("down enter"); // into a
    std::fs::write(a.join("b"), b"").unwrap(); // what a misdirected F2 would hit
    for key in ["f7", "shift-f4", "f2"] {
        std::fs::create_dir(a.join("sub")).unwrap();
        std::fs::write(a.join("sub/b"), b"").unwrap();
        changed(cx, Side::Left);
        cx.simulate_keystrokes("home down enter end"); // into a/sub, onto b
        assert_eq!(path(&commander, Side::Left, cx), a.join("sub"), "{key}");
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        assert!(dialog_open(cx), "{key}");
        std::fs::remove_dir_all(a.join("sub")).unwrap();
        changed(cx, Side::Left);
        assert_eq!(path(&commander, Side::Left, cx), a, "{key}: moved up");
        cx.simulate_input(if key == "f2" { "y" } else { "x" });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!a.join("x").exists(), "{key} acted in the parent");
        assert!(!a.join("y").exists(), "{key} acted in the parent");
        assert!(dialog_open(cx), "{key}: the error shows");
        cx.simulate_keystrokes("enter escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "{key}");
    }
}

#[gpui_kit::test]
fn the_quick_search_survives_a_quiet_reload(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("b");
    std::fs::write(tmp.path().join("new"), b"").unwrap();
    changed(cx, Side::Left);
    commander.read_with(cx, |c, _| assert_eq!(c.search(), Some("b")));
}

#[gpui_kit::test]
fn a_slow_folder_keeps_the_old_listing_until_it_is_read(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    assert_eq!(path(&commander, Side::Left, cx), tmp.path());
    assert!(loading(&commander, cx));
    cx.simulate_keystrokes("down");
    assert_eq!(
        cursor(&commander, Side::Left, cx),
        1,
        "keys for the loading panel wait"
    );
    release(&tmp);
    wait_until(cx, |cx| {
        path(&commander, Side::Left, cx) == tmp.path().join("a")
    });
    assert!(!loading(&commander, cx));
}

#[gpui_kit::test]
fn escape_cancels_a_load(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    cx.simulate_keystrokes("escape");
    assert!(!loading(&commander, cx));
    release(&tmp);
    for _ in 0..5 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        path(&commander, Side::Left, cx),
        tmp.path(),
        "the late result is dropped"
    );
}

#[gpui_kit::test]
fn tab_and_the_other_panel_work_while_one_loads(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    cx.simulate_keystrokes("tab down");
    commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Right));
    assert_eq!(cursor(&commander, Side::Right, cx), 1);
    release(&tmp);
    wait_until(cx, |cx| {
        path(&commander, Side::Left, cx) == tmp.path().join("a")
    });
}

#[gpui_kit::test]
fn dialogs_wait_while_the_panel_loads(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    for key in ["f2", "f5", "f6", "f7", "f8", "shift-f8", "f3"] {
        cx.simulate_keystrokes(key);
        assert!(!dialog_open(cx), "{key}");
    }
    assert_eq!(viewers(cx), 0);
    release(&tmp);
}

fn dead_load(
    request: yagni_commander_core::LoadRequest,
) -> std::sync::mpsc::Receiver<std::io::Result<yagni_commander_core::Listing>> {
    drop(request);
    std::sync::mpsc::channel().1 // sender dropped: like a thread that never started
}

#[gpui_kit::test]
fn a_loader_that_dies_reports_an_error(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    file_manager(cx).update(cx, |this, _| this.load = Some(dead_load));
    cx.simulate_keystrokes("down enter");
    wait_until(cx, |cx| !loading(&commander, cx));
    assert_eq!(path(&commander, Side::Left, cx), tmp.path());
    assert!(commander.read_with(cx, |c, _| c.error().is_some()));
}

#[gpui_kit::test]
fn the_loading_indicator_shows_after_a_moment(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    assert!(
        bounds(cx, "loading-left".into()).is_none(),
        "not right away"
    );
    wait_until(cx, |cx| bounds(cx, "loading-left".into()).is_some());
    release(&tmp);
    wait_until(cx, |cx| {
        path(&commander, Side::Left, cx) == tmp.path().join("a")
    });
    assert!(bounds(cx, "loading-left".into()).is_none());
}

#[test]
fn loading_text_names_the_folder_and_the_count() {
    assert_eq!(
        crate::panel_view::loading_text(std::path::Path::new("/mnt/nas"), 12),
        "Loading /mnt/nas... 12 entries"
    );
}

#[gpui_kit::test]
fn startup_reads_both_panels_in_the_background(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("kept")).unwrap();
    setup(cx);
    let commander = Commander::start(
        tmp.path().join("kept"),
        tmp.path().join("gone"),
        false,
        tmp.path().to_path_buf(),
    );
    let commander = cx.new(|_| commander);
    let cx = window_on(commander.clone(), cx);
    wait_until(cx, |cx| {
        commander.read_with(cx, |c, _| {
            c.panel(Side::Left).is_loaded() && c.panel(Side::Right).is_loaded()
        })
    });
    assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("kept"));
    assert_eq!(
        path(&commander, Side::Right, cx),
        tmp.path(),
        "fell back to the parent"
    );
}

#[gpui_kit::test]
fn a_long_load_does_not_redraw_without_news(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    enter_held_a(&tmp, cx);
    wait_until(cx, |cx| bounds(cx, "loading-left".into()).is_some());
    let notified = std::rc::Rc::new(std::cell::Cell::new(0));
    let _watch = cx.update(|_, cx| {
        let notified = notified.clone();
        cx.observe(&commander, move |_, _| notified.set(notified.get() + 1))
    });
    // Half a second with no new entries (the read waits on `hold`).
    for _ in 0..50 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(10));
        cx.run_until_parked();
    }
    assert!(notified.get() <= 1, "{} redraws", notified.get());
    release(&tmp);
    wait_until(cx, |cx| {
        path(&commander, Side::Left, cx) == tmp.path().join("a")
    });
}

#[gpui_kit::test]
fn copy_and_move_wait_while_the_other_panel_loads(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    use_held_loads(cx);
    std::fs::write(tmp.path().join("a/hold"), b"").unwrap();
    cx.simulate_keystrokes("tab down enter tab"); // right loads "a"; left active
    assert!(commander.read_with(cx, |c, _| c.panel(Side::Right).loading().is_some()));
    for key in ["f5", "f6"] {
        cx.simulate_keystrokes(key);
        assert!(!dialog_open(cx), "{key}");
    }
    release(&tmp);
}

// Menu commands.

fn sort_of(
    commander: &Entity<Commander>,
    side: Side,
    cx: &VisualTestContext,
) -> yagni_commander_core::Sort {
    commander.read_with(cx, |c, _| c.panel(side).sort())
}

#[gpui_kit::test]
fn sort_actions_sort_the_active_panel_and_repeat_reverses(cx: &mut TestAppContext) {
    use yagni_commander_core::SortKey;
    let (_tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("tab");
    cx.dispatch_action(crate::actions::SortBySize);
    let sort = sort_of(&commander, Side::Right, cx);
    assert_eq!(sort.key, SortKey::Size);
    assert!(sort.descending);
    assert_eq!(sort_of(&commander, Side::Left, cx).key, SortKey::Name);
    cx.dispatch_action(crate::actions::SortBySize);
    assert!(!sort_of(&commander, Side::Right, cx).descending);
    let actions: [(Box<dyn gpui_kit::Action>, SortKey); 4] = [
        (Box::new(crate::actions::SortByModified), SortKey::Modified),
        (Box::new(crate::actions::SortByOwner), SortKey::Owner),
        (
            Box::new(crate::actions::SortByPermissions),
            SortKey::Permissions,
        ),
        (Box::new(crate::actions::SortByName), SortKey::Name),
    ];
    for (action, key) in actions {
        cx.update(|window, cx| window.dispatch_action(action, cx));
        assert_eq!(sort_of(&commander, Side::Right, cx).key, key);
    }
}

#[gpui_kit::test]
fn about_shows_a_dialog_and_ok_returns_to_the_panels(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    cx.dispatch_action(crate::actions::About);
    cx.run_until_parked();
    assert!(dialog_open(cx));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&commander, Side::Left, cx), 1);
}

#[test]
fn about_text_has_name_and_version() {
    let text = super::commands::about_text();
    assert!(text.contains("yagni-commander"));
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
}

#[gpui_kit::test]
fn menu_labels_show_the_primary_keys(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let view = file_manager(cx);
    let focus = view.read_with(cx, |this, _| this.focus.clone());
    let key = |action: &dyn gpui_kit::Action, cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .highest_precedence_binding_for_action_in(action, &focus)
                .map(|b| b.keystrokes()[0].inner().unparse())
        })
    };
    assert_eq!(key(&crate::actions::Trash, cx).as_deref(), Some("f8"));
    assert_eq!(
        key(&crate::actions::Delete, cx).as_deref(),
        Some("shift-f8")
    );
    let quit = if cfg!(target_os = "macos") {
        "cmd-q"
    } else {
        "alt-f4"
    };
    assert_eq!(key(&crate::actions::Quit, cx).as_deref(), Some(quit));
    // The native macOS menu shows an action's first binding instead
    // (gpui-pre-macos `platform.rs`, zed issue 23621).
    let first = |action: &dyn gpui_kit::Action, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            cx.key_bindings()
                .borrow()
                .bindings_for_action(action)
                .next()
                .map(|b| b.keystrokes()[0].inner().unparse())
        })
    };
    assert_eq!(first(&crate::actions::Trash, cx).as_deref(), Some("f8"));
    assert_eq!(
        first(&crate::actions::Delete, cx).as_deref(),
        Some("shift-f8")
    );
    assert_eq!(first(&crate::actions::Quit, cx).as_deref(), Some(quit));
    for (action, keys) in [
        (&crate::actions::Pack as &dyn gpui_kit::Action, "alt-f5"),
        (&crate::actions::Extract, "alt-f6"),
    ] {
        assert_eq!(key(action, cx).as_deref(), Some(keys));
        assert_eq!(first(action, cx).as_deref(), Some(keys));
    }
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    assert_eq!(key(&crate::actions::CopyPath, cx).as_deref(), Some(copy));
    assert_eq!(first(&crate::actions::CopyPath, cx).as_deref(), Some(copy));
    for (action, keys) in [
        (
            &crate::actions::DirectoryHotlist as &dyn gpui_kit::Action,
            "ctrl-d",
        ),
        (&crate::actions::NewTab as &dyn gpui_kit::Action, "ctrl-t"),
        (&crate::actions::CloseTab, "ctrl-w"),
        (&crate::actions::NextTab, "ctrl-tab"),
        (&crate::actions::PrevTab, "ctrl-shift-tab"),
    ] {
        assert_eq!(key(action, cx).as_deref(), Some(keys));
        assert_eq!(first(action, cx).as_deref(), Some(keys));
    }
}

fn native_check(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|_, cx| {
        cx.get_menus()
            .unwrap()
            .iter()
            .flat_map(|m| m.items.clone())
            .any(|item| {
                matches!(item, gpui_kit::OwnedMenuItem::Action { name, checked: true, .. }
                    if name.as_str() == label)
            })
    })
}

#[gpui_kit::test]
fn menu_checks_follow_ctrl_dot_sorting_and_the_active_panel(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    assert!(!native_check(cx, "Hidden files"));
    assert!(native_check(cx, "Sort by name"));
    cx.simulate_keystrokes("ctrl-.");
    assert!(native_check(cx, "Hidden files"));
    cx.dispatch_action(crate::actions::SortBySize);
    assert!(native_check(cx, "Sort by size"));
    assert!(!native_check(cx, "Sort by name"));
    // The right panel still sorts by name.
    cx.simulate_keystrokes("tab");
    assert!(native_check(cx, "Sort by name"));
}

/// A zip at `path`: names ending in "/" are folders, the rest files with
/// the given contents.
fn make_zip(path: &std::path::Path, entries: &[(&str, &str)]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for (name, contents) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, options).unwrap();
        } else {
            zip.start_file(*name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
    }
    zip.finish().unwrap();
}

/// The open message box's title and text (copied with Ctrl-C).
fn box_text(cx: &mut VisualTestContext) -> String {
    cx.simulate_keystrokes("ctrl-c");
    clipboard_text(cx).unwrap_or_default()
}

fn clipboard_text(cx: &mut VisualTestContext) -> Option<String> {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .map(|text| text.to_string())
}

#[gpui_kit::test]
fn ctrl_c_and_ctrl_ins_copy_the_full_path_under_the_cursor(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    let path = |name: &str| tmp.path().join(name).display().to_string();
    // On "..", the panel's own folder.
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx), Some(tmp.path().display().to_string()));
    cx.simulate_keystrokes("down ctrl-c");
    assert_eq!(clipboard_text(cx), Some(path("a")));
    cx.simulate_keystrokes("end ctrl-insert");
    assert_eq!(clipboard_text(cx), Some(path("f")));
    // Only the cursor entry, never the selection.
    cx.simulate_keystrokes("home down space ctrl-c");
    assert_eq!(clipboard_text(cx), Some(path("b")));
    if cfg!(target_os = "macos") {
        cx.simulate_keystrokes("end cmd-c");
        assert_eq!(clipboard_text(cx), Some(path("f")));
    }
}

#[gpui_kit::test]
fn ctrl_c_ends_the_quick_search_and_copies_its_match(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.simulate_keystrokes("f ctrl-c");
    assert_eq!(search(&commander, cx), None);
    assert_eq!(
        clipboard_text(cx),
        Some(tmp.path().join("f").display().to_string())
    );
}

#[gpui_kit::test]
fn ctrl_c_in_a_text_field_copies_its_text_not_the_path(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("down f2");
    cx.run_until_parked();
    // F2 preselects the name.
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("a"));
    cx.simulate_keystrokes("end ctrl-a ctrl-insert");
    assert_eq!(clipboard_text(cx).as_deref(), Some("a"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
    assert!(tmp.path().join("a").is_dir());
}

#[gpui_kit::test]
fn ctrl_c_does_nothing_while_the_panel_loads(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()));
    enter_held_a(&tmp, cx);
    assert!(loading(&commander, cx));
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("before"));
    release(&tmp);
}

#[gpui_kit::test]
fn text_fields_take_the_classic_clipboard_keys(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("f7");
    cx.run_until_parked();
    cx.simulate_input("abc");
    // Shift-Del cuts, Shift-Ins pastes.
    cx.simulate_keystrokes("ctrl-a shift-delete");
    let clipboard = |cx: &mut VisualTestContext| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .map(|text| text.to_string())
    };
    assert_eq!(clipboard(cx).as_deref(), Some("abc"));
    cx.simulate_keystrokes("shift-insert shift-insert");
    // Ctrl-Ins copies.
    cx.simulate_keystrokes("ctrl-a ctrl-insert end");
    assert_eq!(clipboard(cx).as_deref(), Some("abcabc"));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(tmp.path().join("abcabc").is_dir());
}

// Settings.

use yagni_commander_core::Setting;

/// Points the app at a config file in `dir` holding `text`, loaded as at startup.
fn use_config(dir: &std::path::Path, text: &str, cx: &mut VisualTestContext) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    let state = crate::config_state::CurrentConfig::load(Some(path.clone()));
    cx.update(|_, cx| cx.set_global(state));
    path
}

fn config(cx: &mut VisualTestContext) -> yagni_commander_core::Config {
    cx.update(|_, cx| {
        cx.global::<crate::config_state::CurrentConfig>()
            .config
            .clone()
    })
}

fn config_problem(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| {
        cx.global::<crate::config_state::CurrentConfig>()
            .problem
            .clone()
    })
}

fn use_log_dir(dir: &std::path::Path, cx: &mut VisualTestContext) {
    let view = file_manager(cx);
    view.update(cx, |this, _| this.log_dir = Some(dir.to_owned()));
}

fn change_setting(setting: Setting, cx: &mut VisualTestContext) {
    let view = file_manager(cx);
    cx.update(|window, cx| view.update(cx, |this, cx| this.change_setting(setting, window, cx)));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn changing_a_setting_applies_it_and_saves_only_that_key(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    std::fs::write(tmp.path().join("Zed"), b"").unwrap();
    std::fs::write(tmp.path().join("apple"), b"").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "# mine\ncase_sensitive_sort = false\n", cx);
    let names = |cx: &mut VisualTestContext| {
        commander.read_with(cx, |c, _| {
            c.panel(Side::Left)
                .entries()
                .iter()
                .map(|e| e.label.clone())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(names(cx), ["..", "a", "b", "apple", "f", "Zed"]);
    change_setting(Setting::CaseSensitiveSort(true), cx);
    assert_eq!(names(cx), ["..", "a", "b", "Zed", "apple", "f"]);
    assert!(config(cx).case_sensitive_sort);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\ncase_sensitive_sort = true\n"
    );
}

#[gpui_kit::test]
fn turning_logging_on_and_off_takes_effect_at_once(cx: &mut TestAppContext) {
    let (tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    use_log_dir(logs.path(), cx);
    change_setting(Setting::Log(true), cx);
    assert!(commander.read_with(cx, |c, _| c.log().is_some()));
    cx.simulate_keystrokes("f7");
    cx.simulate_input("logged");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(tmp.path().join("logged").is_dir());
    assert_eq!(std::fs::read_dir(logs.path()).unwrap().count(), 1);
    change_setting(Setting::Log(false), cx);
    assert!(commander.read_with(cx, |c, _| c.log().is_none()));
}

#[gpui_kit::test]
fn ctrl_r_retries_a_log_that_failed_to_start(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let not_a_folder = logs.path().join("file");
    std::fs::write(&not_a_folder, b"").unwrap();
    use_config(cfg.path(), "", cx);
    use_log_dir(&not_a_folder, cx);
    change_setting(Setting::Log(true), cx);
    assert!(commander.read_with(cx, |c, _| c.log().is_none()));
    use_log_dir(logs.path(), cx);
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    assert!(commander.read_with(cx, |c, _| c.log().is_some()));
}

#[gpui_kit::test]
fn the_editor_setting_is_used_by_f4_at_once(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    change_setting(Setting::editor("true"), cx);
    // Without an editor F4 shows an error box (f4_without_editor_does_not_crash).
    cx.simulate_keystrokes("down f4");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
}

#[gpui_kit::test]
fn a_broken_config_is_never_written(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "log = maybe\n", cx);
    assert!(config_problem(cx).is_some());
    change_setting(Setting::Log(true), cx);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "log = maybe\n");
    assert!(!config(cx).log);
}

#[gpui_kit::test]
fn ctrl_r_rereads_the_config(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    use_log_dir(logs.path(), cx);
    std::fs::write(
        &path,
        "editor = \"zed\"\ncase_sensitive_sort = true\nlog = true\n",
    )
    .unwrap();
    cx.simulate_keystrokes("ctrl-r");
    let now = config(cx);
    assert_eq!(now.editor.as_deref(), Some("zed"));
    assert!(now.case_sensitive_sort);
    assert!(commander.read_with(cx, |c, _| c.panel(Side::Left).sort().case_sensitive));
    assert!(commander.read_with(cx, |c, _| c.log().is_some()));
}

#[gpui_kit::test]
fn ctrl_r_on_a_broken_config_keeps_the_settings_and_says_why(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "editor = \"zed\"\n", cx);
    std::fs::write(&path, "editor = \n").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    assert_eq!(config(cx).editor.as_deref(), Some("zed"));
    let view = file_manager(cx);
    let notice = view.read_with(cx, |this, _| this.notice.clone());
    assert!(notice.unwrap().starts_with("Config ignored: "));
    assert!(config_problem(cx).is_some());
    // Fixed and re-read: the problem is gone.
    std::fs::write(&path, "editor = \"vim\"\n").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    assert_eq!(config(cx).editor.as_deref(), Some("vim"));
    assert!(config_problem(cx).is_none());
}

// The settings dialog.

/// Test windows start inactive; a real one is active when the user types.
/// gpui reports focus changes (blur, focus-out) for the active window only.
fn activate(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
}

/// A full key press, down and up. `simulate_keystrokes` sends only key-down,
/// and gpui turns Space or Enter on a focused button or switch into a click
/// on key-up.
fn press(cx: &mut VisualTestContext, key: &str) {
    let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
    cx.simulate_event(gpui_kit::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    cx.run_until_parked();
}

fn settings_open(cx: &mut VisualTestContext) -> bool {
    cx.run_until_parked();
    cx.debug_bounds("settings-editor").is_some()
}

#[gpui_kit::test]
fn settings_show_where_the_logs_are(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let logs = tempfile::tempdir().unwrap();
    use_log_dir(logs.path(), cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    assert!(settings_open(cx));
    assert!(cx.debug_bounds("settings-log-dir").is_some());
    let view = file_manager(cx);
    let shown = view.read_with(cx, |this, cx| {
        this.settings.as_ref().unwrap().read(cx).log_dir_text()
    });
    let path = logs.path().display().to_string();
    assert_eq!(
        shown.as_deref(),
        Some(format!("Log files are stored in {path} (click to copy)").as_str())
    );
    // A click copies the path alone and says so.
    click(cx, "settings-log-dir", 1);
    cx.run_until_parked();
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied, Some(path.clone()));
    let shown = view.read_with(cx, |this, cx| {
        this.settings.as_ref().unwrap().read(cx).log_dir_text()
    });
    assert_eq!(
        shown.as_deref(),
        Some(format!("Log files are stored in {path} (copied)").as_str())
    );
    // The dialog stays open.
    assert!(settings_open(cx));
}

#[gpui_kit::test]
fn ctrl_comma_opens_settings_with_the_current_values(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "editor = \"vim\"\nlog_keep_days = 30\n", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    assert!(settings_open(cx));
    let view = file_manager(cx);
    let (editor, days) = view.read_with(cx, |this, cx| {
        let s = this.settings.as_ref().unwrap().read(cx);
        (s.editor_text(cx), s.days_text(cx))
    });
    assert_eq!((editor.as_str(), days.as_str()), ("vim", "30"));
    cx.simulate_keystrokes("escape");
    assert!(!settings_open(cx));
}

#[gpui_kit::test]
fn typing_an_editor_saves_it_on_close(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "# keep me\n", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.simulate_input("code --wait");
    cx.simulate_keystrokes("enter");
    assert!(!settings_open(cx));
    assert_eq!(config(cx).editor.as_deref(), Some("code --wait"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# keep me\n"), "{text}");
    assert!(text.contains("editor = \"code --wait\""), "{text}");
}

#[gpui_kit::test]
fn escape_also_keeps_what_was_typed(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.simulate_input("zed");
    cx.simulate_keystrokes("escape");
    assert!(!settings_open(cx));
    assert_eq!(config(cx).editor.as_deref(), Some("zed"));
}

#[gpui_kit::test]
fn keys_reach_the_panels_after_settings_close(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-, escape");
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    assert_eq!(cursor(&commander, Side::Left, cx), 1);
}

#[gpui_kit::test]
fn tab_reaches_the_switches_and_space_toggles_them(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    use_log_dir(logs.path(), cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab"); // sort switch
    press(cx, "space");
    assert!(config(cx).case_sensitive_sort);
    cx.simulate_keystrokes("tab"); // log switch
    press(cx, "space");
    assert!(config(cx).log);
    assert!(commander.read_with(cx, |c, _| c.log().is_some()));
    assert!(settings_open(cx));
}

#[gpui_kit::test]
fn enter_on_a_switch_closes_without_toggling(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    press(cx, "enter");
    assert!(!settings_open(cx));
    assert!(!config(cx).case_sensitive_sort);
}

#[gpui_kit::test]
fn clicking_a_switch_toggles_it(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    click(cx, "settings-sort", 1);
    assert!(config(cx).case_sensitive_sort);
}

fn name_width(cx: &mut VisualTestContext) -> f32 {
    f32::from(cx.debug_bounds("header-left-Name").unwrap().size.width)
}

#[gpui_kit::test]
fn switches_hide_and_show_the_optional_columns(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    for (switch, column, key) in [
        ("settings-modified", "Modified", "show_modified"),
        ("settings-owner", "Owner", "show_owner"),
        ("settings-permissions", "Permissions", "show_permissions"),
    ] {
        let before = name_width(cx);
        click(cx, switch, 1);
        cx.run_until_parked();
        for side in ["left", "right"] {
            let header: &'static str = format!("header-{side}-{column}").leak();
            assert!(cx.debug_bounds(header).is_none(), "{header} hidden");
        }
        assert!(name_width(cx) > before, "Name takes the {column} width");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("{key} = false")), "{text}");
    }
    assert!(cx.debug_bounds("header-left-Size").is_some(), "Size stays");
    click(cx, "settings-owner", 1);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("header-left-Owner").is_some(),
        "shown again"
    );
}

#[gpui_kit::test]
fn hiding_the_sort_column_sorts_by_name(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    click(cx, "header-left-Owner", 1);
    let key =
        |cx: &VisualTestContext| commander.read_with(cx, |c, _| c.panel(Side::Left).sort().key);
    assert_eq!(key(cx), yagni_commander_core::SortKey::Owner);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    click(cx, "settings-owner", 1);
    cx.run_until_parked();
    assert_eq!(key(cx), yagni_commander_core::SortKey::Name);
    cx.dispatch_action(crate::actions::SortByOwner);
    assert_eq!(
        key(cx),
        yagni_commander_core::SortKey::Name,
        "its action does nothing"
    );
}

#[gpui_kit::test]
fn ctrl_r_applies_hand_edited_columns(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    std::fs::write(&path, "show_permissions = false\n").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    assert!(cx.debug_bounds("header-left-Permissions").is_none());
    assert!(cx.debug_bounds("header-left-Owner").is_some());
}

#[gpui_kit::test]
fn invalid_days_show_an_error_and_are_not_saved(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "log_keep_days = 30\n", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab tab tab"); // editor -> sort -> log -> days
    cx.simulate_keystrokes("ctrl-a");
    cx.simulate_input("0");
    cx.simulate_keystrokes("tab"); // leaving the field checks it
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-days-error").is_some());
    assert_eq!(config(cx).log_keep_days, 30);
    cx.simulate_keystrokes("shift-tab ctrl-a");
    cx.simulate_input("14");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-days-error").is_none());
    assert_eq!(config(cx).log_keep_days, 14);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("log_keep_days = 14")
    );
}

#[gpui_kit::test]
fn the_close_button_closes(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    // editor -> sort -> log -> days -> icons -> modified -> owner -> permissions -> Close
    cx.simulate_keystrokes("tab tab tab tab tab tab tab tab");
    press(cx, "space");
    assert!(!settings_open(cx));
}

#[gpui_kit::test]
fn a_broken_config_disables_the_dialog_and_says_why(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "log = maybe\n", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-problem").is_some());
    click(cx, "settings-sort", 1);
    assert!(!config(cx).case_sensitive_sort);
}

#[gpui_kit::test]
fn a_failed_save_shows_an_error_and_still_applies(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    std::fs::write(&path, "log = maybe\n").unwrap(); // broken since startup
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    click(cx, "settings-sort", 1);
    cx.run_until_parked();
    assert!(config(cx).case_sensitive_sort);
    // The error box is on top of the settings dialog; dismiss it.
    cx.simulate_keystrokes("enter");
    assert!(settings_open(cx));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "log = maybe\n");
}

#[gpui_kit::test]
fn the_menu_opens_settings(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    cx.dispatch_action(crate::actions::OpenSettings);
    assert!(settings_open(cx));
}

#[gpui_kit::test]
fn a_save_failing_on_close_shows_its_error_after_closing(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    std::fs::write(&path, "log = maybe\n").unwrap(); // broken since startup
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.simulate_input("zed");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!settings_open(cx));
    assert!(dialog_open(cx), "the error box shows");
    assert_eq!(config(cx).editor.as_deref(), Some("zed"));
}

#[gpui_kit::test]
fn closing_settings_does_not_undo_a_hand_edit(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "editor = \"vim\"\nlog_keep_days = 30\n", cx);
    std::fs::write(&path, "editor = \"helix\"\nlog_keep_days = 9\n").unwrap();
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    click(cx, "settings-sort", 1); // saves, and reads the file's other keys
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("editor = \"helix\""), "{text}");
    assert!(text.contains("log_keep_days = 9"), "{text}");
    assert_eq!(config(cx).editor.as_deref(), Some("helix"));
}

#[gpui_kit::test]
fn enter_with_invalid_days_keeps_the_dialog_open(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab tab tab ctrl-a");
    cx.simulate_input("0");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(settings_open(cx));
    assert!(cx.debug_bounds("settings-days-error").is_some());
    // Escape still closes, dropping the invalid value.
    cx.simulate_keystrokes("escape");
    assert!(!settings_open(cx));
    assert_eq!(config(cx).log_keep_days, 7);
}

// The menu bar: Linux only (macOS has the native one, no F10 or lone Alt).
#[gpui_kit::test]
fn a_button_row_rings_its_button_only_while_it_has_focus(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("f7");
    cx.run_until_parked();
    assert!(dialog_open(cx));
    assert!(
        bounds(cx, "focus-ring-button".into()).is_none(),
        "the field has focus"
    );
    cx.simulate_keystrokes("tab");
    assert!(bounds(cx, "focus-ring-button".into()).is_some());
    cx.simulate_keystrokes("escape");
}

#[cfg(not(target_os = "macos"))]
mod menu_bar {
    use super::*;

    fn menu_open(cx: &mut VisualTestContext) -> Option<usize> {
        let view = file_manager(cx);
        // Draw a frame, as the app would: focus changes are reported then.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.read_with(cx, |this, cx| {
            this.menu_bar.as_ref().unwrap().read(cx).open_index()
        })
    }

    #[gpui_kit::test]
    fn f10_opens_the_first_menu_and_enter_runs_an_item(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("down"); // left cursor on row 1, right on row 0
        cx.simulate_keystrokes("f10");
        assert_eq!(menu_open(cx), Some(0));
        cx.simulate_keystrokes("right");
        assert_eq!(menu_open(cx), Some(1));
        // Commands > Swap panels: Down selects the first item, Down again the second.
        cx.simulate_keystrokes("down down enter");
        assert_eq!(menu_open(cx), None);
        // Both panels show the same folder, so the swap shows in the cursors.
        assert_eq!(cursor(&commander, Side::Right, cx), 1);
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
    }

    #[gpui_kit::test]
    fn alt_f_opens_the_files_menu(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("b alt-f");
        assert_eq!(menu_open(cx), Some(0));
        assert_eq!(search(&commander, cx), None, "Alt-F ends the quick search");
        // Again: Files stays open (no toggle).
        cx.simulate_keystrokes("alt-f");
        assert_eq!(menu_open(cx), Some(0));
        // From another menu: back to Files.
        cx.simulate_keystrokes("right alt-f");
        assert_eq!(menu_open(cx), Some(0));
        cx.simulate_keystrokes("escape");
        assert_eq!(menu_open(cx), None);
        // Keys reach the panel again: from "b" (the search's match) to "f".
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 3);
    }

    #[gpui_kit::test]
    fn left_and_right_wrap_between_menus(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("f10 left");
        assert_eq!(menu_open(cx), Some(3));
        cx.simulate_keystrokes("right");
        assert_eq!(menu_open(cx), Some(0));
    }

    #[gpui_kit::test]
    fn escape_closes_the_menu_and_keys_reach_the_panel_again(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("f10 down escape");
        assert_eq!(menu_open(cx), None);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 1);
        cx.simulate_keystrokes("f10 f10");
        assert_eq!(menu_open(cx), None);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 2);
    }

    #[gpui_kit::test]
    fn menu_rename_opens_the_prompt_with_focus_in_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("down"); // on "a"
        // Files > Rename is the 7th item.
        cx.simulate_keystrokes("f10 down down down down down down down enter");
        cx.run_until_parked();
        assert!(dialog_open(cx));
        cx.simulate_input("z");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(tmp.path().join("z").is_dir());
    }

    #[gpui_kit::test]
    fn menu_hidden_files_toggles_like_ctrl_dot(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        // Show is the 3rd menu, Hidden files its first item.
        cx.simulate_keystrokes("f10 right right down enter");
        assert!(commander.read_with(cx, |c, _| c.shows_hidden()));
    }

    #[gpui_kit::test]
    fn clicking_a_title_opens_and_closes_its_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        click(cx, "menu-Commands", 1);
        assert_eq!(menu_open(cx), Some(1));
        click(cx, "menu-Commands", 1);
        assert_eq!(menu_open(cx), None);
    }

    #[gpui_kit::test]
    fn clicking_a_title_ends_the_quick_search(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("b");
        assert!(search(&commander, cx).is_some());
        click(cx, "menu-Files", 1);
        assert_eq!(menu_open(cx), Some(0));
        assert_eq!(search(&commander, cx), None);
    }

    #[gpui_kit::test]
    fn alt_f4_quits_while_a_menu_is_open(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        let quit = std::rc::Rc::new(std::cell::Cell::new(false));
        cx.update(|_, cx| {
            let quit = quit.clone();
            cx.on_action(move |_: &crate::actions::Quit, _| quit.set(true));
        });
        cx.simulate_keystrokes("f10");
        assert_eq!(menu_open(cx), Some(0));
        cx.simulate_keystrokes("alt-f4");
        assert!(quit.get());
    }

    #[gpui_kit::test]
    fn hovering_another_title_switches_an_open_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        click(cx, "menu-Files", 1);
        let position = center(cx, "menu-Show");
        cx.simulate_mouse_move(position, None, gpui_kit::Modifiers::default());
        assert_eq!(menu_open(cx), Some(2));
    }

    #[gpui_kit::test]
    fn clicking_an_item_runs_it(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        click(cx, "menu-Show", 1);
        // The first item (Hidden files) sits at the top of the popup.
        let popup = bounds(cx, "menu-popup".into()).expect("popup drawn");
        let at = gpui_kit::point(popup.center().x, popup.top() + gpui_kit::px(16.0));
        click_at(cx, at, 1);
        assert!(commander.read_with(cx, |c, _| c.shows_hidden()));
        assert_eq!(menu_open(cx), None);
    }

    #[gpui_kit::test]
    fn clicking_the_panels_closes_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("f10");
        click(cx, "row-left-1", 1);
        assert_eq!(menu_open(cx), None);
    }

    // Lone Alt and keys while a menu is open.

    fn alt_tap(cx: &mut VisualTestContext) {
        cx.simulate_modifiers_change(gpui_kit::Modifiers::alt());
        cx.simulate_modifiers_change(gpui_kit::Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_lone_alt_opens_and_closes_the_menu(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        alt_tap(cx);
        assert_eq!(menu_open(cx), Some(0));
        alt_tap(cx);
        assert_eq!(menu_open(cx), None);
        cx.simulate_keystrokes("down");
        assert_eq!(cursor(&commander, Side::Left, cx), 1);
    }

    #[gpui_kit::test]
    fn alt_z_does_not_open_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_modifiers_change(gpui_kit::Modifiers::alt());
        cx.simulate_keystrokes("alt-z");
        cx.simulate_modifiers_change(gpui_kit::Modifiers::none());
        assert_eq!(menu_open(cx), None);
    }

    #[gpui_kit::test]
    fn alt_click_does_not_open_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_modifiers_change(gpui_kit::Modifiers::alt());
        click(cx, "row-left-1", 1);
        cx.simulate_modifiers_change(gpui_kit::Modifiers::none());
        assert_eq!(menu_open(cx), None);
    }

    #[gpui_kit::test]
    fn alt_released_after_a_window_switch_does_not_open_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        activate(cx);
        cx.simulate_modifiers_change(gpui_kit::Modifiers::alt());
        cx.deactivate_window();
        activate(cx);
        cx.simulate_modifiers_change(gpui_kit::Modifiers::none());
        assert_eq!(menu_open(cx), None);
        // The next lone Alt works again.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        alt_tap(cx);
        assert_eq!(menu_open(cx), Some(0));
    }

    #[gpui_kit::test]
    fn deactivating_the_window_closes_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        activate(cx);
        cx.simulate_keystrokes("f10");
        cx.deactivate_window();
        assert_eq!(menu_open(cx), None);
    }

    #[gpui_kit::test]
    fn keys_other_than_navigation_are_ignored_while_the_menu_is_open(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("f10");
        cx.simulate_keystrokes("f7");
        cx.simulate_input("x");
        cx.simulate_keystrokes("tab ctrl-a space");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(menu_open(cx), Some(0));
        assert_eq!(search(&commander, cx), None);
        assert!(selected(&commander, Side::Left, cx).is_empty());
        commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
        assert!(!tmp.path().join("x").exists());
        cx.simulate_keystrokes("escape down");
        assert_eq!(cursor(&commander, Side::Left, cx), 1);
    }

    #[gpui_kit::test]
    fn a_lone_alt_in_a_dialog_does_nothing(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("f7");
        cx.run_until_parked();
        alt_tap(cx);
        assert_eq!(menu_open(cx), None);
        assert!(dialog_open(cx));
    }

    #[gpui_kit::test]
    fn alt_held_while_switching_into_the_window_does_not_open_the_menu(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        activate(cx);
        cx.deactivate_window();
        // Wayland reports the held Alt right after the window gets focus.
        activate(cx);
        alt_tap(cx);
        assert_eq!(menu_open(cx), None);
        // A lone Alt a moment later works.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        alt_tap(cx);
        assert_eq!(menu_open(cx), Some(0));
    }

    #[gpui_kit::test]
    fn a_dialog_opening_closes_the_menu(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = open(cx);
        activate(cx); // gpui reports focus changes for the active window only
        cx.simulate_keystrokes("f10");
        // E.g. a file operation's error summary arriving while the menu is open.
        cx.dispatch_action(crate::actions::About);
        cx.run_until_parked();
        assert!(dialog_open(cx));
        assert_eq!(menu_open(cx), None);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        // Typing reaches the panels again.
        cx.simulate_input("f");
        assert_eq!(search(&commander, cx).as_deref(), Some("f"));
    }
}

#[gpui_kit::test]
fn rows_show_icons_unless_turned_off(cx: &mut TestAppContext) {
    let (_tmp, commander, cx) = open(cx);
    // Rows: "..", a, b, f.
    for ix in 0..4 {
        let selector: &'static str = format!("icon-left-{ix}").leak();
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
    commander.update(cx, |c, cx| {
        c.set_icons(false);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("icon-left-1").is_none());
    assert!(cx.debug_bounds("row-left-1").is_some());
}

#[gpui_kit::test]
fn ctrl_r_applies_a_hand_edited_icons_key(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "", cx);
    assert!(cx.debug_bounds("icon-left-1").is_some());
    std::fs::write(&path, "icons = false\n").unwrap();
    cx.simulate_keystrokes("ctrl-r");
    cx.run_until_parked();
    assert!(cx.debug_bounds("icon-left-1").is_none());
}

#[gpui_kit::test]
fn the_icons_switch_turns_icons_off_at_once_and_saves(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    let path = use_config(cfg.path(), "# mine\nlog = false\n", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    click(cx, "settings-icons", 1);
    cx.run_until_parked();
    assert!(cx.debug_bounds("icon-left-1").is_none());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# mine"), "{text}");
    assert!(text.lines().any(|l| l == "icons = false"), "{text}");
    click(cx, "settings-icons", 1);
    cx.run_until_parked();
    assert!(cx.debug_bounds("icon-left-1").is_some());
}

mod hotlist {
    use super::*;

    /// A config with entries &Alpha -> a and &Beta -> b (in the test
    /// folder), plus `extra` entries as (name, path).
    fn use_hotlist(
        tmp: &tempfile::TempDir,
        cfg: &tempfile::TempDir,
        extra: &[(&str, &str)],
        cx: &mut VisualTestContext,
    ) -> std::path::PathBuf {
        let mut text = String::new();
        let a = tmp.path().join("a").display().to_string();
        let b = tmp.path().join("b").display().to_string();
        for (name, path) in [("&Alpha", a.as_str()), ("&Beta", b.as_str())]
            .into_iter()
            .chain(extra.iter().copied())
        {
            text += &format!("[[hotlist]]\nname = \"{name}\"\npath = \"{path}\"\n\n");
        }
        use_config(cfg.path(), &text, cx)
    }

    pub(super) fn hotlist_open(cx: &mut VisualTestContext) -> bool {
        cx.run_until_parked();
        file_manager(cx).read_with(cx, |this, _| this.hotlist.is_some())
    }

    fn highlight(cx: &mut VisualTestContext) -> usize {
        file_manager(cx).read_with(cx, |this, cx| {
            this.hotlist.as_ref().unwrap().popup.read(cx).highlight()
        })
    }

    #[gpui_kit::test]
    fn ctrl_d_and_a_letter_go_to_that_folder(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("ctrl-d");
        assert!(hotlist_open(cx));
        assert!(bounds(cx, "hotlist-row-0".into()).is_some());
        cx.simulate_keystrokes("b");
        assert!(!hotlist_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
        // Upper case works too, and keys reach the panel again.
        cx.simulate_keystrokes("backspace ctrl-d shift-a");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
    }

    #[gpui_kit::test]
    fn arrows_wrap_enter_picks_and_escape_closes(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("ctrl-d");
        assert_eq!(highlight(cx), 0);
        // Rows: Alpha, Beta, Add current folder, Configure...
        cx.simulate_keystrokes("up");
        assert_eq!(highlight(cx), 3);
        cx.simulate_keystrokes("down down");
        assert_eq!(highlight(cx), 1);
        cx.simulate_keystrokes("escape");
        assert!(!hotlist_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
        cx.simulate_keystrokes("down");
        assert_eq!(
            cursor(&commander, Side::Left, cx),
            1,
            "keys reach the panel"
        );
        cx.simulate_keystrokes("ctrl-d down enter");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    #[gpui_kit::test]
    fn a_click_picks_a_row(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("ctrl-d");
        click(cx, "hotlist-row-1", 1);
        assert!(!hotlist_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    #[gpui_kit::test]
    fn other_keys_are_ignored_while_it_is_open(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("ctrl-d f7 tab space ctrl-a x");
        assert!(hotlist_open(cx));
        assert!(!dialog_open(cx), "F7 did nothing");
        commander.read_with(cx, |c, _| assert_eq!(c.active(), Side::Left));
        assert!(selected(&commander, Side::Left, cx).is_empty());
        assert_eq!(search(&commander, cx), None, "x started no search");
    }

    #[gpui_kit::test]
    fn the_first_of_two_entries_with_a_letter_wins(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let f = tmp.path().display().to_string();
        use_hotlist(&tmp, &cfg, &[("&alpha too", f.as_str())], cx);
        cx.simulate_keystrokes("ctrl-d a");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
    }

    #[gpui_kit::test]
    fn the_right_panel_gets_its_own_popup_and_navigation(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("tab ctrl-d");
        let side = file_manager(cx).read_with(cx, |this, _| this.hotlist.as_ref().unwrap().side);
        assert_eq!(side, Side::Right);
        let row = bounds(cx, "hotlist-row-0".into()).unwrap();
        let divider = bounds(cx, "divider".into()).unwrap();
        assert!(
            row.origin.x > divider.origin.x,
            "drawn over the right panel"
        );
        cx.simulate_keystrokes("a");
        assert_eq!(path(&commander, Side::Right, cx), tmp.path().join("a"));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
    }

    #[gpui_kit::test]
    fn a_missing_folder_shows_its_error_and_the_panel_stays(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("gone").display().to_string();
        use_hotlist(&tmp, &cfg, &[("&Gone", gone.as_str())], cx);
        cx.simulate_keystrokes("ctrl-d g");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
        assert!(commander.read_with(cx, |c, _| c.error().is_some()));
    }

    #[gpui_kit::test]
    fn ctrl_d_ends_the_quick_search(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("f ctrl-d");
        assert_eq!(search(&commander, cx), None);
        assert!(hotlist_open(cx));
    }

    #[gpui_kit::test]
    fn ctrl_d_does_nothing_while_the_panel_loads(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        enter_held_a(&tmp, cx);
        assert!(loading(&commander, cx));
        cx.simulate_keystrokes("ctrl-d");
        assert!(!hotlist_open(cx));
        release(&tmp);
    }

    #[gpui_kit::test]
    fn focus_leaving_closes_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        activate(cx);
        cx.simulate_keystrokes("ctrl-d");
        assert!(hotlist_open(cx));
        click(cx, "row-right-1", 1);
        assert!(!hotlist_open(cx));
    }

    #[gpui_kit::test]
    fn deactivating_the_window_closes_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        activate(cx);
        cx.simulate_keystrokes("ctrl-d");
        assert!(hotlist_open(cx));
        cx.deactivate_window();
        assert!(!hotlist_open(cx));
    }

    #[gpui_kit::test]
    fn the_quit_key_still_quits_while_it_is_open(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        let quit = std::rc::Rc::new(std::cell::Cell::new(false));
        cx.update(|_, cx| {
            let quit = quit.clone();
            cx.on_action(move |_: &crate::actions::Quit, _| quit.set(true));
        });
        cx.simulate_keystrokes("ctrl-d");
        assert!(hotlist_open(cx));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-q"
        } else {
            "alt-f4"
        });
        assert!(quit.get());
    }

    #[cfg(target_os = "linux")]
    #[gpui_kit::test]
    fn the_menu_bar_opens_it_from_the_keyboard(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        activate(cx);
        // Commands > Directory hotlist, the fourth item.
        cx.simulate_keystrokes("f10 right down down down down enter");
        assert!(hotlist_open(cx), "the closing menu leaves it open");
        cx.simulate_keystrokes("b");
        assert!(!hotlist_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    #[gpui_kit::test]
    fn a_failed_save_shows_why_and_keeps_the_entry_for_the_session(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        let before = std::fs::read_to_string(&file).unwrap();
        // The atomic write puts a temporary file next to the config.
        std::fs::set_permissions(cfg.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        cx.simulate_keystrokes("ctrl-d up up enter");
        cx.simulate_input("&Here");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        std::fs::set_permissions(cfg.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            box_text(cx).starts_with("Hotlist not saved\n"),
            "{}",
            box_text(cx)
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        assert_eq!(config(cx).hotlist.len(), 3, "used until the app quits");
    }

    #[gpui_kit::test]
    fn the_commands_menu_opens_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        cx.dispatch_action(crate::actions::DirectoryHotlist);
        assert!(hotlist_open(cx));
    }

    fn saved_hotlist(cfg: &std::path::Path) -> Vec<yagni_commander_core::config::HotlistEntry> {
        let text = std::fs::read_to_string(cfg).unwrap();
        toml::from_str::<yagni_commander_core::Config>(&text)
            .unwrap()
            .hotlist
    }

    #[gpui_kit::test]
    fn add_current_folder_is_refused_inside_an_archive(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = super::archive_browsing::inside_zip(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        cx.simulate_keystrokes("ctrl-d up up enter");
        cx.run_until_parked();
        assert_eq!(
            box_text(cx),
            format!(
                "Inside an archive\n{}",
                yagni_commander_core::archive::IN_ARCHIVE
            )
        );
        assert_eq!(saved_hotlist(&file).len(), 2, "nothing added");
    }

    #[gpui_kit::test]
    fn add_current_folder_asks_for_a_name_and_saves(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        // Rows: Alpha, Beta, Add, Configure. Up twice from Alpha is Add.
        cx.simulate_keystrokes("ctrl-d up up enter");
        assert!(dialog_open(cx), "name prompt");
        // The folder's name is preselected: typing replaces it.
        cx.simulate_input("&Here");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        let saved = saved_hotlist(&file);
        assert_eq!(saved.len(), 3);
        assert_eq!(saved[2].name, "&Here");
        assert_eq!(saved[2].path, tmp.path().display().to_string());
        assert_eq!(config(cx).hotlist, saved, "applied at once");
        // The new entry works right away.
        cx.simulate_keystrokes("down enter ctrl-d h");
        assert_eq!(
            path(&commander_of(cx), Side::Left, cx),
            tmp.path(),
            "back via its letter"
        );
    }

    #[gpui_kit::test]
    fn the_name_prompt_starts_with_the_folder_name(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_config(cfg.path(), "", cx);
        // Empty hotlist: rows are Add, Configure; Enter on Add, Enter again
        // accepts the suggested name.
        cx.simulate_keystrokes("ctrl-d enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let saved = saved_hotlist(&file);
        let name = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(saved[0].name, name);
    }

    #[gpui_kit::test]
    fn escape_in_the_name_prompt_adds_nothing(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        let before = std::fs::read_to_string(&file).unwrap();
        cx.simulate_keystrokes("ctrl-d up up enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    }

    #[gpui_kit::test]
    fn a_broken_config_refuses_add_but_entries_still_work(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_config(cfg.path(), "log = maybe\n", cx);
        // The app keeps entries it had before the file broke.
        cx.update(|_, cx| {
            let current = cx.global_mut::<crate::config_state::CurrentConfig>();
            current.config.hotlist = vec![yagni_commander_core::config::HotlistEntry {
                name: "&Alpha".into(),
                path: tmp.path().join("a").display().to_string(),
            }];
        });
        cx.simulate_keystrokes("ctrl-d down enter"); // Add
        cx.run_until_parked();
        assert!(dialog_open(cx), "error box, not the prompt");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "log = maybe\n");
        cx.simulate_keystrokes("ctrl-d a");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
    }

    #[gpui_kit::test]
    fn ctrl_r_picks_up_a_hotlist_edited_on_disk(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_config(cfg.path(), "", cx);
        let b = tmp.path().join("b").display().to_string();
        std::fs::write(
            &file,
            format!("[[hotlist]]\nname = \"&Beta\"\npath = \"{b}\"\n"),
        )
        .unwrap();
        cx.simulate_keystrokes("ctrl-r ctrl-d b");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    fn dialog_entries(cx: &mut VisualTestContext) -> Vec<(String, String)> {
        file_manager(cx).read_with(cx, |this, cx| {
            this.hotlist_dialog
                .as_ref()
                .unwrap()
                .read(cx)
                .entries()
                .iter()
                .map(|e| (e.name.clone(), e.path.clone()))
                .collect()
        })
    }

    fn dialog_cursor(cx: &mut VisualTestContext) -> usize {
        file_manager(cx).read_with(cx, |this, cx| {
            this.hotlist_dialog.as_ref().unwrap().read(cx).cursor()
        })
    }

    fn names(cx: &mut VisualTestContext) -> Vec<String> {
        dialog_entries(cx)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Opens Configure from the popup (Up from the first row wraps to it).
    fn configure(cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("ctrl-d up enter");
        cx.run_until_parked();
        assert!(dialog_open(cx));
    }

    #[gpui_kit::test]
    fn configure_lists_the_entries_with_the_cursor_on_the_first(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        configure(cx);
        assert_eq!(names(cx), ["&Alpha", "&Beta"]);
        assert_eq!(dialog_cursor(cx), 0);
        assert!(bounds(cx, "hotlist-entry-1".into()).is_some());
    }

    #[gpui_kit::test]
    fn list_keys_move_reorder_and_remove(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let c = tmp.path().display().to_string();
        use_hotlist(&tmp, &cfg, &[("&Gamma", c.as_str())], cx);
        configure(cx);
        cx.simulate_keystrokes("down");
        assert_eq!(dialog_cursor(cx), 1);
        cx.simulate_keystrokes("alt-up");
        assert_eq!(names(cx), ["&Beta", "&Alpha", "&Gamma"]);
        assert_eq!(dialog_cursor(cx), 0, "the cursor follows the entry");
        cx.simulate_keystrokes("alt-up"); // already first: nothing
        assert_eq!(names(cx), ["&Beta", "&Alpha", "&Gamma"]);
        cx.simulate_keystrokes("alt-down alt-down");
        assert_eq!(names(cx), ["&Alpha", "&Gamma", "&Beta"]);
        assert_eq!(dialog_cursor(cx), 2);
        cx.simulate_keystrokes("delete");
        assert_eq!(names(cx), ["&Alpha", "&Gamma"]);
        assert_eq!(dialog_cursor(cx), 1, "the cursor stays on the last row");
        click(cx, "hotlist-entry-0", 1);
        assert_eq!(dialog_cursor(cx), 0);
    }

    #[gpui_kit::test]
    fn ok_saves_and_cancel_does_not(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        let before = std::fs::read_to_string(&file).unwrap();
        configure(cx);
        cx.simulate_keystrokes("delete escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            before,
            "Escape cancels"
        );
        configure(cx);
        cx.simulate_keystrokes("delete enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        let saved: Vec<_> = saved_hotlist(&file).into_iter().map(|e| e.name).collect();
        assert_eq!(saved, ["&Beta"]);
        assert_eq!(config(cx).hotlist.len(), 1);
    }

    #[gpui_kit::test]
    fn typing_edits_the_entry_under_the_cursor(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        configure(cx);
        // List -> Name: select all, type.
        cx.simulate_keystrokes("down tab ctrl-a");
        cx.simulate_input("&Bin");
        assert_eq!(names(cx), ["&Alpha", "&Bin"], "the list follows the typing");
        // Name -> Path.
        cx.simulate_keystrokes("tab ctrl-a");
        cx.simulate_input("~/bin");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let saved = saved_hotlist(&file);
        assert_eq!(saved[1].name, "&Bin");
        assert_eq!(saved[1].path, "~/bin");
    }

    #[gpui_kit::test]
    fn moving_the_cursor_shows_that_entry_in_the_fields(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        configure(cx);
        cx.simulate_keystrokes("down");
        let fields = file_manager(cx).read_with(cx, |this, cx| {
            this.hotlist_dialog
                .as_ref()
                .unwrap()
                .read(cx)
                .field_texts(cx)
        });
        assert_eq!(fields.0, "&Beta");
        assert_eq!(fields.1, tmp.path().join("b").display().to_string());
    }

    #[gpui_kit::test]
    fn a_bad_path_keeps_the_dialog_open_on_that_entry(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        let before = std::fs::read_to_string(&file).unwrap();
        configure(cx);
        cx.simulate_keystrokes("down tab tab ctrl-a");
        cx.simulate_input("relative/path");
        cx.simulate_keystrokes("up"); // in the field: no effect on the list
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        // The error box is on top of the still-open Configure dialog.
        assert!(file_manager(cx).read_with(cx, |this, _| this.hotlist_dialog.is_some()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
        cx.simulate_keystrokes("enter"); // dismiss the error
        cx.run_until_parked();
        assert!(dialog_open(cx), "Configure stays");
        assert_eq!(dialog_cursor(cx), 1);
    }

    #[gpui_kit::test]
    fn the_buttons_add_remove_and_move(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        configure(cx);
        // Buttons: Add(0) Remove(1) Move up(2) Move down(3) Cancel(4) OK(5).
        // The row remembers the last button pressed, so go to the first
        // one, then right `ix` times. The row is reached with Shift-Tab
        // from the list (focus cycles backwards past the dialog's start).
        let button = |cx: &mut VisualTestContext, ix: usize| {
            cx.simulate_keystrokes("shift-tab");
            for _ in 0..5 {
                cx.simulate_keystrokes("left");
            }
            for _ in 0..ix {
                cx.simulate_keystrokes("right");
            }
            press(cx, "enter");
        };
        button(cx, 0); // Add current folder
        assert_eq!(names(cx).len(), 3);
        assert_eq!(dialog_cursor(cx), 2);
        let folder = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            dialog_entries(cx)[2],
            (folder.clone(), tmp.path().display().to_string())
        );
        // Add put focus in Name; a click on the list puts it back there.
        click(cx, "hotlist-entry-2", 1);
        button(cx, 2); // Move up: [Alpha, folder, Beta]
        assert_eq!(dialog_cursor(cx), 1);
        click(cx, "hotlist-entry-1", 1);
        button(cx, 3); // Move down: [Alpha, Beta, folder]
        assert_eq!(dialog_cursor(cx), 2);
        click(cx, "hotlist-entry-0", 1);
        button(cx, 1); // Remove Alpha
        assert_eq!(names(cx), ["&Beta".to_owned(), folder]);
        click(cx, "hotlist-entry-0", 1);
        button(cx, 5); // OK
        assert!(!dialog_open(cx));
        assert_eq!(saved_hotlist(&file).len(), 2);
    }

    #[gpui_kit::test]
    fn an_empty_list_says_so_and_add_fills_it(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_config(cfg.path(), "", cx);
        cx.simulate_keystrokes("ctrl-d down enter"); // rows: Add, Configure
        cx.run_until_parked();
        assert!(bounds(cx, "hotlist-empty".into()).is_some());
        cx.simulate_keystrokes("delete alt-up"); // nothing to act on: no panic
        assert!(names(cx).is_empty());
    }

    #[gpui_kit::test]
    fn a_broken_config_refuses_configure(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_config(cfg.path(), "log = maybe\n", cx);
        cx.simulate_keystrokes("ctrl-d down enter"); // rows: Add, Configure
        cx.run_until_parked();
        assert!(dialog_open(cx), "error box");
        assert!(file_manager(cx).read_with(cx, |this, _| this.hotlist_dialog.is_none()));
    }

    #[gpui_kit::test]
    fn a_dialog_taking_focus_closes_the_popup_and_keeps_the_focus(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        use_hotlist(&tmp, &cfg, &[], cx);
        activate(cx);
        cx.simulate_keystrokes("ctrl-d");
        assert!(hotlist_open(cx));
        // E.g. a file operation's progress dialog opening on its own.
        cx.update(|window, cx| {
            super::commands::show_error("Some error", "details", None, window, cx)
        });
        cx.run_until_parked();
        assert!(!hotlist_open(cx));
        let view = file_manager(cx);
        let panels_focused = cx.update(|window, cx| view.read(cx).focus.is_focused(window));
        assert!(!panels_focused, "focus stays in the dialog");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "Enter reached the dialog's button");
    }

    #[gpui_kit::test]
    fn paths_are_trimmed_on_ok(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let cfg = tempfile::tempdir().unwrap();
        let file = use_hotlist(&tmp, &cfg, &[], cx);
        configure(cx);
        cx.simulate_keystrokes("down tab tab ctrl-a");
        cx.simulate_input(" ~/bin ");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx), "a padded path is accepted");
        assert_eq!(saved_hotlist(&file)[1].path, "~/bin");
    }
}

mod open_file {
    use super::*;

    /// A stand-in for xdg-open that writes its argument to `out`.
    fn use_recording_opener(
        dir: &std::path::Path,
        cx: &mut VisualTestContext,
    ) -> std::path::PathBuf {
        let out = dir.join("opened");
        let opener = dir.join("opener");
        std::fs::write(
            &opener,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // A thread of another test may fork while the script is still open
        // for writing; its exec then fails with "text file busy" until that
        // child has exec'd. Run it once successfully before the app does.
        for _ in 0..200 {
            match std::process::Command::new(&opener).arg("warm-up").status() {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                _ => break,
            }
        }
        std::fs::remove_file(&out).unwrap();
        use_opener(opener.to_str().unwrap(), cx);
        out
    }

    fn use_opener(opener: &str, cx: &mut VisualTestContext) {
        file_manager(cx).update(cx, |this, _| this.opener = opener.into());
    }

    fn opened(out: &std::path::Path, cx: &mut VisualTestContext) -> String {
        wait_until(cx, |_| {
            std::fs::read_to_string(out).is_ok_and(|s| s.ends_with('\n'))
        });
        std::fs::read_to_string(out).unwrap().trim_end().to_owned()
    }

    #[gpui_kit::test]
    fn enter_on_a_file_hands_it_to_the_opener(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let scratch = tempfile::tempdir().unwrap();
        let out = use_recording_opener(scratch.path(), cx);
        cx.simulate_keystrokes("end enter"); // on "f"
        assert_eq!(opened(&out, cx), tmp.path().join("f").display().to_string());
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_double_click_on_a_file_opens_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let scratch = tempfile::tempdir().unwrap();
        let out = use_recording_opener(scratch.path(), cx);
        click(cx, "row-left-3", 2);
        assert_eq!(opened(&out, cx), tmp.path().join("f").display().to_string());
    }

    #[gpui_kit::test]
    fn enter_on_a_folder_still_enters_it(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        let scratch = tempfile::tempdir().unwrap();
        let out = use_recording_opener(scratch.path(), cx);
        cx.simulate_keystrokes("down enter");
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
        cx.run_until_parked();
        assert!(!out.exists());
    }

    #[gpui_kit::test]
    fn an_opener_that_fails_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        use_opener("false", cx); // like xdg-open with no application for the type
        cx.simulate_keystrokes("end enter");
        wait_until(cx, dialog_open);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_missing_opener_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        use_opener("no-such-opener-xyz", cx);
        cx.simulate_keystrokes("end enter");
        wait_until(cx, dialog_open);
    }

    /// An executable file in the test folder, ready to exec, listed.
    fn executable(
        tmp: &tempfile::TempDir,
        name: &str,
        text: &str,
        cx: &mut VisualTestContext,
    ) -> std::path::PathBuf {
        let path = tmp.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // See use_recording_opener: wait out "text file busy".
        for _ in 0..200 {
            match std::process::Command::new(&path)
                .arg("warm-up")
                .current_dir(tmp.path())
                .status()
            {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                _ => break,
            }
        }
        cx.simulate_keystrokes("ctrl-r");
        path
    }

    #[gpui_kit::test]
    fn enter_on_a_script_runs_it_in_the_panels_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let scratch = tempfile::tempdir().unwrap();
        let opened = use_recording_opener(scratch.path(), cx);
        executable(
            &tmp,
            "run.sh",
            "#!/bin/sh\n[ \"$1\" = warm-up ] || pwd > ran\n",
            cx,
        );
        cx.simulate_keystrokes("end enter"); // on run.sh
        let ran = tmp.path().join("ran");
        wait_until(cx, |_| {
            std::fs::read_to_string(&ran).is_ok_and(|s| s.ends_with('\n'))
        });
        let pwd = std::fs::read_to_string(&ran).unwrap();
        assert_eq!(
            std::path::Path::new(pwd.trim_end()).canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
        assert!(!opened.exists(), "not handed to the opener");
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn an_executable_document_goes_to_the_opener(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let scratch = tempfile::tempdir().unwrap();
        let out = use_recording_opener(scratch.path(), cx);
        // Like any file on an NTFS or SMB mount: executable, not a program.
        executable(&tmp, "report.pdf", "%PDF-1.7\n", cx);
        cx.simulate_keystrokes("end enter");
        assert_eq!(
            opened(&out, cx),
            tmp.path().join("report.pdf").display().to_string()
        );
    }

    #[gpui_kit::test]
    fn a_program_that_cannot_start_shows_an_error(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        executable(&tmp, "x-broken", "#!/no/such/interpreter\n", cx);
        cx.simulate_keystrokes("end enter");
        wait_until(cx, dialog_open);
    }

    #[gpui_kit::test]
    fn a_successful_open_shows_nothing_later(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        // window_on's opener is `true`.
        cx.simulate_keystrokes("end enter");
        for _ in 0..20 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(50));
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!dialog_open(cx));
    }
}

mod copy_message {
    use super::*;

    fn show(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            super::commands::show_error(
                "Cannot open file",
                "No application could open it.",
                None,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(dialog_open(cx));
    }

    #[gpui_kit::test]
    fn ctrl_c_copies_an_error_box_and_leaves_it_open(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        show(cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(
            clipboard_text(cx).as_deref(),
            Some("Cannot open file\nNo application could open it.")
        );
        assert!(dialog_open(cx), "copying doesn't close it");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn ctrl_ins_copies_it_too(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        show(cx);
        cx.simulate_keystrokes("ctrl-insert");
        assert_eq!(
            clipboard_text(cx).as_deref(),
            Some("Cannot open file\nNo application could open it.")
        );
        if cfg!(target_os = "macos") {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("x".into()));
            cx.simulate_keystrokes("cmd-c");
            assert_eq!(
                clipboard_text(cx).as_deref(),
                Some("Cannot open file\nNo application could open it.")
            );
        }
    }

    #[gpui_kit::test]
    fn a_real_error_box_copies_its_text(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        // Shift-F4 without an editor shows an error box.
        cx.simulate_keystrokes("shift-f4");
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(
            clipboard_text(cx).as_deref(),
            Some("Cannot open editor\nNo editor configured: set `editor` in config.toml.")
        );
    }

    #[gpui_kit::test]
    fn ctrl_c_in_a_confirm_dialog_copies_nothing(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        use_fake_trash(cx);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()));
        cx.simulate_keystrokes("down f8");
        cx.run_until_parked();
        assert!(dialog_open(cx));
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard_text(cx).as_deref(), Some("before"));
        assert!(dialog_open(cx));
        cx.simulate_keystrokes("escape");
    }
}

/// gpui animations run on wall-clock time: a dialog sliding in moves
/// between frames, and under a loaded test run a click aimed at one frame's
/// position lands somewhere else. Tests use reduced motion (`setup`).
#[gpui_kit::test]
fn dialogs_appear_in_place_so_clicks_land(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    let cfg = tempfile::tempdir().unwrap();
    use_config(cfg.path(), "", cx);
    activate(cx);
    cx.simulate_keystrokes("ctrl-,");
    cx.run_until_parked();
    let first = center(cx, "settings-sort");
    // Longer than a loaded machine's gap between two frames.
    std::thread::sleep(std::time::Duration::from_millis(120));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert_eq!(center(cx, "settings-sort"), first, "the dialog moved");
    click_at(cx, first, 1);
    assert!(config(cx).case_sensitive_sort);
}

mod archives {
    use super::*;

    fn zip_names(path: &std::path::Path) -> Vec<String> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_owned())
            .collect();
        names.sort();
        names
    }

    fn is_link_entry(path: &std::path::Path, name: &str) -> bool {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        zip.by_name(name).unwrap().is_symlink()
    }

    fn link_prompt_open(cx: &mut VisualTestContext) -> bool {
        cx.run_until_parked();
        cx.debug_bounds("link-prompt").is_some()
    }

    #[gpui_kit::test]
    fn alt_f5_packs_the_entry_under_the_cursor_into_the_other_panel(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx); // left cursor on "f"
        cx.simulate_keystrokes("alt-f5");
        cx.run_until_parked();
        assert!(dialog_open(cx));
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!dialog_open(cx));
        assert_eq!(zip_names(&tmp.path().join("a/f.zip")), ["f"]);
    }

    #[gpui_kit::test]
    fn the_typed_name_gets_zip_added(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        cx.simulate_keystrokes("alt-f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("bundle");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(
            tmp.path().join("bundle.zip").is_file(),
            "relative to the active panel"
        );
    }

    #[gpui_kit::test]
    fn an_existing_zip_asks_before_overwriting(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        let zip = tmp.path().join("a/f.zip");
        std::fs::write(&zip, "old").unwrap();
        cx.simulate_keystrokes("alt-f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter"); // the prompt
        cx.run_until_parked();
        assert!(dialog_open(cx), "the confirm box");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(std::fs::read_to_string(&zip).unwrap(), "old");
        cx.simulate_keystrokes("alt-f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter"); // Overwrite (preselected)
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(zip_names(&zip), ["f"]);
    }

    /// `b` holds `link -> ../f`; packs `b` and waits for the link prompt.
    fn pack_b_with_a_link(tmp: &tempfile::TempDir, cx: &mut VisualTestContext) {
        let _ = std::fs::remove_file(tmp.path().join("a/b.zip"));
        let link = tmp.path().join("b/link");
        if link.symlink_metadata().is_err() {
            std::os::unix::fs::symlink("../f", &link).unwrap();
        }
        cx.simulate_keystrokes("home down down alt-f5"); // on "b"
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, link_prompt_open);
    }

    #[gpui_kit::test]
    fn the_link_prompt_buttons_follow_store_leave_out_and_cancel(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        let zip = tmp.path().join("a/b.zip");

        pack_b_with_a_link(&tmp, cx);
        cx.simulate_keystrokes("enter"); // Follow
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(zip_names(&zip), ["b/", "b/link"]);
        assert!(!is_link_entry(&zip, "b/link"), "followed: a file");

        pack_b_with_a_link(&tmp, cx);
        cx.simulate_keystrokes("right enter"); // Store as link
        wait_until(cx, |cx| !job_running(cx));
        assert!(is_link_entry(&zip, "b/link"));

        pack_b_with_a_link(&tmp, cx);
        cx.simulate_keystrokes("right right enter"); // Leave out
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(zip_names(&zip), ["b/"]);
        assert!(dialog_open(cx), "the left-out summary");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        pack_b_with_a_link(&tmp, cx);
        cx.simulate_keystrokes("escape"); // Cancel
        wait_until(cx, |cx| !job_running(cx));
        assert!(!zip.exists());
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn same_for_the_remaining_links_asks_once(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        std::os::unix::fs::symlink("../f", tmp.path().join("b/l1")).unwrap();
        std::os::unix::fs::symlink("../f", tmp.path().join("b/l2")).unwrap();
        cx.simulate_keystrokes("home down down alt-f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, link_prompt_open);
        click(cx, "link-all", 1);
        // Back to the buttons (Follow is still highlighted), then press it.
        cx.simulate_keystrokes("tab");
        press(cx, "enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!link_prompt_open(cx), "asked once");
        assert_eq!(
            zip_names(&tmp.path().join("a/b.zip")),
            ["b/", "b/l1", "b/l2"]
        );
    }

    #[gpui_kit::test]
    fn alt_f5_ends_the_quick_search_and_waits_for_a_loading_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        cx.simulate_keystrokes("f alt-f5");
        cx.run_until_parked();
        assert_eq!(search(&commander, cx), None);
        assert!(dialog_open(cx));
        cx.simulate_keystrokes("escape home");
        cx.run_until_parked();
        enter_held_a(&tmp, cx);
        assert!(loading(&commander, cx));
        cx.simulate_keystrokes("alt-f5");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        release(&tmp);
    }

    /// Extracts the left panel's last entry (`pkg.zip`) with `key`.
    fn extract_last_with(key: &str, cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("ctrl-r end");
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        assert!(dialog_open(cx), "the folder prompt");
    }

    #[gpui_kit::test]
    fn alt_f6_extracts_into_the_other_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open_with_target(cx);
        make_zip(
            &tmp.path().join("pkg.zip"),
            &[("pkg/", ""), ("pkg/x.txt", "x")],
        );
        extract_last_with("alt-f6", cx);
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a/pkg/x.txt")).unwrap(),
            "x"
        );
        assert!(
            labels(&commander, Side::Right, cx).contains(&"pkg".to_owned()),
            "reloaded"
        );
    }

    #[gpui_kit::test]
    fn alt_f9_extracts_too(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        make_zip(&tmp.path().join("pkg.zip"), &[("x.txt", "x")]);
        extract_last_with("alt-f9", cx);
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a/pkg/x.txt")).unwrap(),
            "x"
        );
    }

    #[gpui_kit::test]
    fn a_typed_relative_folder_is_used_and_created(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        make_zip(&tmp.path().join("pkg.zip"), &[("pkg/x.txt", "x")]);
        extract_last_with("alt-f6", cx);
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input("out/here");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/here/pkg/x.txt")).unwrap(),
            "x"
        );
    }

    #[gpui_kit::test]
    fn no_archive_under_the_cursor_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open_with_target(cx); // on "f"
        cx.simulate_keystrokes("alt-f6");
        cx.run_until_parked();
        assert!(dialog_open(cx), "the error box");
        assert!(!job_running(cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn an_existing_file_asks_and_enter_overwrites(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        make_zip(&tmp.path().join("pkg.zip"), &[("pkg/x.txt", "new")]);
        std::fs::create_dir_all(tmp.path().join("a/pkg")).unwrap();
        std::fs::write(tmp.path().join("a/pkg/x.txt"), "old").unwrap();
        extract_last_with("alt-f6", cx);
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| cx.debug_bounds("conflict-new").is_some());
        cx.simulate_keystrokes("enter"); // Overwrite
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a/pkg/x.txt")).unwrap(),
            "new"
        );
    }

    #[gpui_kit::test]
    fn several_selected_archives_extract_each(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        make_zip(&tmp.path().join("one.zip"), &[("1.txt", "1")]);
        make_zip(&tmp.path().join("two.zip"), &[("2.txt", "2")]);
        // Rows: .., a, b, f, one.zip, two.zip
        cx.simulate_keystrokes("ctrl-r end space up space alt-f6");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(tmp.path().join("a/one/1.txt").is_file());
        assert!(tmp.path().join("a/two/2.txt").is_file());
    }

    #[gpui_kit::test]
    fn alt_f6_waits_for_a_loading_panel(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        enter_held_a(&tmp, cx);
        cx.simulate_keystrokes("alt-f6");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        release(&tmp);
    }

    /// A zip with `pkg/x.txt` ("x") under the AES password "pw", rewritten
    /// until the wrong password "nope" fails the quick check (AES lets a
    /// wrong password through 1 time in 65536).
    fn make_locked_zip(path: &std::path::Path) {
        use std::io::Write;
        for _ in 0..1000 {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .with_aes_encryption(zip::AesMode::Aes256, "pw");
            zip.start_file("pkg/x.txt", options).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
            let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
            if archive.by_index_decrypt(0, b"nope").is_err() {
                return;
            }
        }
        panic!("no zip rejecting \"nope\"");
    }

    fn password_prompt_open(cx: &mut VisualTestContext) -> bool {
        cx.run_until_parked();
        cx.debug_bounds("password-prompt").is_some()
    }

    fn wrong_shown(cx: &mut VisualTestContext) -> bool {
        cx.debug_bounds("password-wrong").is_some()
    }

    /// Alt-F6 on `pkg.zip`, OK on the folder prompt, then waits for the
    /// password prompt.
    fn extract_locked(tmp: &tempfile::TempDir, cx: &mut VisualTestContext) {
        make_locked_zip(&tmp.path().join("pkg.zip"));
        extract_last_with("alt-f6", cx);
        cx.simulate_keystrokes("enter");
        wait_until(cx, password_prompt_open);
    }

    fn extracted(tmp: &tempfile::TempDir) -> Option<String> {
        std::fs::read_to_string(tmp.path().join("a/pkg/x.txt")).ok()
    }

    #[gpui_kit::test]
    fn a_password_zip_asks_and_enter_extracts(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        extract_locked(&tmp, cx);
        assert!(!wrong_shown(cx));
        let masked = cx.update(|window, cx| {
            use gpui_kit::component::WindowExt;
            use gpui_kit::component::input::AnyInputState;
            match window.focused_input(cx) {
                Some(AnyInputState::Input(state)) => state.read(cx).presentation().is_masked(),
                _ => false,
            }
        });
        assert!(masked, "the focused field is masked");
        cx.simulate_input("pw");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(extracted(&tmp).as_deref(), Some("x"));
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_wrong_password_asks_again_and_the_ok_button_answers_once(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        extract_locked(&tmp, cx);
        cx.simulate_input("nope");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| password_prompt_open(cx) && wrong_shown(cx));
        cx.simulate_input("pw");
        // The button row: OK is highlighted.
        cx.simulate_keystrokes("tab");
        press(cx, "enter");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(extracted(&tmp).as_deref(), Some("x"));
        assert!(!dialog_open(cx), "no stale answer, no error box");
    }

    /// `pkg.zip` with `pkg/x.txt` under AES "pw" and `pkg/y.txt` under
    /// AES "other", each rewritten until "pw" fails `y`'s quick check.
    fn make_two_password_zip(path: &std::path::Path) {
        use std::io::Write;
        for _ in 0..1000 {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            for (name, password) in [("pkg/x.txt", "pw"), ("pkg/y.txt", "other")] {
                let options = zip::write::SimpleFileOptions::default()
                    .with_aes_encryption(zip::AesMode::Aes256, password);
                zip.start_file(name, options).unwrap();
                zip.write_all(b"x").unwrap();
            }
            zip.finish().unwrap();
            let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
            if archive.by_index_decrypt(1, b"pw").is_err() {
                return;
            }
        }
        panic!("no zip rejecting \"pw\" for y");
    }

    #[gpui_kit::test]
    fn the_ok_button_answers_exactly_once(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        make_two_password_zip(&tmp.path().join("pkg.zip"));
        extract_last_with("alt-f6", cx);
        cx.simulate_keystrokes("enter");
        wait_until(cx, password_prompt_open);
        cx.simulate_input("pw");
        cx.simulate_keystrokes("tab");
        press(cx, "enter"); // OK: x.txt extracts, y.txt asks
        wait_until(cx, password_prompt_open);
        // A second "pw" left in the channel would have answered this
        // question at once and brought up "Wrong password.".
        assert!(!wrong_shown(cx), "asked fresh, not answered by a stale OK");
        cx.simulate_input("other");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(tmp.path().join("a/pkg/y.txt").is_file());
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn skip_archive_extracts_nothing_and_shows_no_error(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        extract_locked(&tmp, cx);
        cx.simulate_keystrokes("tab right");
        press(cx, "enter"); // Skip archive
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(extracted(&tmp), None);
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn escape_cancels_the_job(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open_with_target(cx);
        extract_locked(&tmp, cx);
        cx.simulate_keystrokes("escape");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(extracted(&tmp), None);
        assert!(
            !dialog_open(cx),
            "both the prompt and the progress dialog closed"
        );
    }
}

mod compare {
    use super::*;

    /// Runs Compare by content and returns the box's text (copied with
    /// Ctrl-C), then closes the box.
    fn compare(cx: &mut VisualTestContext) -> String {
        cx.dispatch_action(crate::actions::CompareContents);
        wait_until(cx, |cx| !job_running(cx));
        assert!(dialog_open(cx), "a result or error box");
        cx.simulate_keystrokes("ctrl-c");
        let text = clipboard_text(cx).unwrap();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        text
    }

    /// Adds files `g` and `h` next to `f` (listing: .., a, b, f, g, h).
    fn with_files(tmp: &tempfile::TempDir, f: &str, g: &str, cx: &mut VisualTestContext) {
        std::fs::write(tmp.path().join("f"), f).unwrap();
        std::fs::write(tmp.path().join("g"), g).unwrap();
        std::fs::write(tmp.path().join("h"), "").unwrap();
        cx.simulate_keystrokes("ctrl-r");
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn two_selected_files_in_one_panel(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        with_files(&tmp, "same", "same", cx);
        cx.simulate_keystrokes("end up up space space");
        assert_eq!(compare(cx), "Compare\nThe files are identical.");
        assert_eq!(selected(&commander, Side::Left, cx), ["f", "g"], "kept");
        std::fs::write(tmp.path().join("g"), "diff").unwrap();
        assert_eq!(compare(cx), "Compare\nThe files differ.");
    }

    #[gpui_kit::test]
    fn the_cursor_entries_of_both_panels(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("a/x.txt"), "1").unwrap();
        std::fs::write(tmp.path().join("a/only.txt"), "").unwrap();
        std::fs::write(tmp.path().join("b/x.txt"), "2").unwrap();
        std::fs::create_dir(tmp.path().join("b/sub")).unwrap();
        // Left on a, right (active) on b.
        cx.simulate_keystrokes("down tab down down");
        assert_eq!(
            compare(cx),
            "Compare\nThe folders differ:\nonly in left: only.txt\nonly in right: sub/\ndifferent: x.txt"
        );
        std::fs::write(tmp.path().join("b/x.txt"), "1").unwrap();
        std::fs::write(tmp.path().join("b/only.txt"), "").unwrap();
        std::fs::remove_dir(tmp.path().join("b/sub")).unwrap();
        assert_eq!(compare(cx), "Compare\nThe folders are identical (2 files).");
    }

    #[gpui_kit::test]
    fn one_selected_entry_per_panel_beats_the_cursor(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        with_files(&tmp, "1", "1", cx);
        // Left: f selected, cursor moves on to g. Right: cursor on h.
        cx.simulate_keystrokes("end up up space tab end");
        assert_eq!(compare(cx), "Compare\nThe files differ.", "f vs h");
        cx.simulate_keystrokes("up space");
        assert_eq!(compare(cx), "Compare\nThe files are identical.", "f vs g");
    }

    #[gpui_kit::test]
    fn the_names_label_a_pair_from_one_panel(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("b/new"), "").unwrap();
        cx.simulate_keystrokes("down space space");
        assert_eq!(compare(cx), "Compare\nThe folders differ:\nonly in b: new");
    }

    #[gpui_kit::test]
    fn wrong_picks_are_refused(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        with_files(&tmp, "", "", cx);
        let select = "Cannot compare\nSelect two files or two folders to compare.";
        // Both cursors on "..".
        assert_eq!(compare(cx), select);
        // A file and a folder.
        cx.simulate_keystrokes("down tab end");
        assert_eq!(
            compare(cx),
            "Cannot compare\nCannot compare a file with a folder."
        );
        // Three selected in the active panel.
        cx.simulate_keystrokes("end up up space space space");
        assert_eq!(compare(cx), select);
    }

    #[gpui_kit::test]
    fn compare_waits_while_a_panel_loads(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        enter_held_a(&tmp, cx);
        cx.dispatch_action(crate::actions::CompareContents);
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(!job_running(cx));
        release(&tmp);
    }

    #[gpui_kit::test]
    fn the_files_menu_offers_it(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        let labels = cx.update(|_, cx| {
            let state = crate::menus::MenuState::of(_commander.read(cx));
            crate::menus::menus(state, false)
                .remove(0)
                .entries
                .into_iter()
                .filter_map(|entry| match entry {
                    crate::menus::MenuEntry::Item { label, action, .. } => {
                        Some((label, action.name()))
                    }
                    crate::menus::MenuEntry::Separator => None,
                })
                .collect::<Vec<_>>()
        });
        assert!(labels.contains(&("Compare by content", "yagni_commander::CompareContents")));
    }
}

mod archive_browsing {
    use super::*;
    use yagni_commander_core::archive::IN_ARCHIVE;

    /// The left panel inside `pkg.zip` (src/lib/a.rs, src/main.rs,
    /// README), the right panel on `a`.
    pub(super) fn inside_zip(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let (tmp, commander, cx) = open_with_target(cx);
        make_zip(
            &tmp.path().join("pkg.zip"),
            &[("src/lib/a.rs", "a"), ("src/main.rs", "m"), ("README", "r")],
        );
        cx.simulate_keystrokes("ctrl-r end enter");
        cx.run_until_parked();
        assert!(commander.read_with(cx, |c, _| c.panel(Side::Left).in_archive()));
        (tmp, commander, cx)
    }

    /// The error box's text, then closes it.
    fn refusal(cx: &mut VisualTestContext) -> String {
        assert!(dialog_open(cx), "an error box");
        let text = box_text(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        text
    }

    #[gpui_kit::test]
    fn enter_opens_a_zip_and_backspace_leaves_it(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = inside_zip(cx);
        assert_eq!(labels(&commander, Side::Left, cx), ["..", "src", "README"]);
        cx.simulate_keystrokes("down enter");
        assert_eq!(
            path(&commander, Side::Left, cx),
            tmp.path().join("pkg.zip/src")
        );
        cx.simulate_keystrokes("backspace backspace");
        cx.run_until_parked();
        assert_eq!(path(&commander, Side::Left, cx), tmp.path());
        assert!(!commander.read_with(cx, |c, _| c.panel(Side::Left).in_archive()));
    }

    #[gpui_kit::test]
    fn writing_keys_are_refused_inside(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        for key in [
            "f2",
            "f4",
            "f6",
            "f7",
            "f8",
            "delete",
            "shift-f8",
            "shift-delete",
            "shift-f4",
            "alt-f5",
            "alt-f6",
            "alt-f9",
        ] {
            cx.simulate_keystrokes("end");
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
            assert_eq!(
                refusal(cx),
                format!("Inside an archive\n{IN_ARCHIVE}"),
                "{key}"
            );
        }
        assert!(!job_running(cx));
        assert!(tmp.path().join("f").exists());
        assert!(tmp.path().join("pkg.zip").is_file());
    }

    #[gpui_kit::test]
    fn compare_is_refused_when_an_entry_is_inside(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        // The right panel's cursor on a real file, the left on README.
        std::fs::write(tmp.path().join("a/x"), "r").unwrap();
        cx.simulate_keystrokes("tab ctrl-r end tab end");
        cx.run_until_parked();
        cx.dispatch_action(crate::actions::CompareContents);
        cx.run_until_parked();
        assert_eq!(refusal(cx), format!("Inside an archive\n{IN_ARCHIVE}"));
        assert!(!job_running(cx));
    }

    #[gpui_kit::test]
    fn f5_copies_selected_entries_out(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = inside_zip(cx);
        cx.simulate_keystrokes("home down enter"); // into src
        cx.simulate_keystrokes("ctrl-a f5");
        cx.run_until_parked();
        assert!(dialog_open(cx), "destination prompt");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!dialog_open(cx), "no error");
        let read = |p: &str| std::fs::read_to_string(tmp.path().join(p)).unwrap();
        assert_eq!(read("a/lib/a.rs"), "a");
        assert_eq!(read("a/main.rs"), "m");
        assert!(selected(&commander, Side::Left, cx).is_empty(), "copied");
        assert!(commander.read_with(cx, |c, _| c.panel(Side::Left).in_archive()));
        assert!(
            labels(&commander, Side::Right, cx).contains(&"lib".to_owned()),
            "reloaded"
        );
    }

    #[gpui_kit::test]
    fn f5_extracts_several_entries_into_the_archives_own_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        cx.simulate_keystrokes("tab backspace tab"); // right panel on the zip's folder
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-a f5 enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!dialog_open(cx), "no error box");
        let readme = std::fs::read_to_string(tmp.path().join("README")).unwrap();
        assert_eq!(readme, "r");
        assert!(tmp.path().join("src/main.rs").exists());
    }

    #[gpui_kit::test]
    fn f5_on_one_entry_can_rename_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        cx.simulate_keystrokes("end f5"); // README
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input(&tmp.path().join("a/read.txt").display().to_string());
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        let copy = std::fs::read_to_string(tmp.path().join("a/read.txt")).unwrap();
        assert_eq!(copy, "r");
    }

    #[gpui_kit::test]
    fn f5_on_one_folder_preselects_its_whole_name(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        cx.simulate_keystrokes("home down f5"); // src
        cx.run_until_parked();
        cx.simulate_input("v1.0");
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(tmp.path().join("a/v1.0/lib/a.rs").exists());
    }

    #[gpui_kit::test]
    fn copies_into_an_archive_are_refused(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        // The right panel into the zip too, the left back to the folder.
        cx.simulate_keystrokes("alt-z backspace");
        cx.run_until_parked();
        cx.simulate_keystrokes("end f5"); // `pkg.zip` itself as the source
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(box_text(cx), "Cannot copy\nCan't copy into an archive");
        cx.simulate_keystrokes("escape escape");
        cx.run_until_parked();
        assert!(!job_running(cx));
        assert!(!dialog_open(cx));
        assert!(tmp.path().join("pkg.zip").is_file());
    }

    /// Points the F3 temp folder at a fresh folder.
    fn use_temp_dir(cx: &mut VisualTestContext) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("viewer-tmp");
        file_manager(cx).update(cx, |this, _| this.temp_dir = Some(dir));
        temp
    }

    fn copies(temp: &tempfile::TempDir) -> usize {
        std::fs::read_dir(temp.path().join("viewer-tmp"))
            .map(|d| d.count())
            .unwrap_or(0)
    }

    #[gpui_kit::test]
    fn f3_views_an_entry_and_closing_deletes_its_copy(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = inside_zip(cx);
        let temp = use_temp_dir(cx);
        cx.simulate_keystrokes("end f3"); // README
        wait_until(cx, |cx| !job_running(cx));
        assert!(!dialog_open(cx), "no error");
        assert_eq!(viewers(cx), 1);
        assert_eq!(copies(&temp), 1, "one private copy");
        let window = cx
            .windows()
            .into_iter()
            .find(|w| crate::viewer_view::tests::viewer_in(*w, cx).is_some())
            .unwrap();
        let mut viewer_cx = VisualTestContext::from_window(window, cx);
        viewer_cx.simulate_keystrokes("escape");
        viewer_cx.run_until_parked();
        cx.run_until_parked();
        assert_eq!(viewers(cx), 0);
        assert_eq!(copies(&temp), 0, "deleted with the window");
    }

    #[gpui_kit::test]
    fn f3_on_a_folder_does_nothing_and_on_a_link_explains(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open_with_target(cx);
        let temp = use_temp_dir(cx);
        let file = std::fs::File::create(tmp.path().join("links.tar")).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder.append_link(&mut header, "zlink", "target").unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, "dir/", std::io::empty())
            .unwrap();
        builder.finish().unwrap();
        drop(builder);
        cx.simulate_keystrokes("ctrl-r");
        cx.run_until_parked();
        let ix = labels(&commander, Side::Left, cx)
            .iter()
            .position(|l| l == "links.tar")
            .unwrap();
        commander.update(cx, |c, _| {
            c.execute(Command::CursorTo(Side::Left, ix));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(commander.read_with(cx, |c, _| c.panel(Side::Left).in_archive()));

        cx.simulate_keystrokes("home down f3"); // "dir" (folders first)
        cx.run_until_parked();
        assert!(!dialog_open(cx) && !job_running(cx), "a folder: nothing");
        cx.simulate_keystrokes("end f3"); // "zlink"
        cx.run_until_parked();
        assert_eq!(
            box_text(cx),
            "Cannot view file\nOnly files can be viewed inside an archive."
        );
        assert_eq!(viewers(cx), 0);
        assert_eq!(copies(&temp), 0);
    }

    #[gpui_kit::test]
    fn f3_inside_an_archive_writes_nothing_to_the_operation_log(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = inside_zip(cx);
        let _temp = use_temp_dir(cx);
        let logs = tempfile::tempdir().unwrap();
        let log = yagni_commander_core::oplog::OperationLog::open(logs.path()).unwrap();
        commander.update(cx, |c, _| c.set_log(Some(std::sync::Arc::new(log))));
        cx.simulate_keystrokes("end f3");
        wait_until(cx, |cx| !job_running(cx));
        assert_eq!(viewers(cx), 1);
        let text: String = std::fs::read_dir(logs.path())
            .unwrap()
            .map(|f| std::fs::read_to_string(f.unwrap().path()).unwrap())
            .collect();
        assert!(!text.contains("viewer-tmp"), "{text}");
    }

    #[gpui_kit::test]
    fn f3_without_a_temp_folder_says_so(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = inside_zip(cx);
        file_manager(cx).update(cx, |this, _| this.temp_dir = None);
        cx.simulate_keystrokes("end f3");
        cx.run_until_parked();
        assert_eq!(
            box_text(cx),
            "Cannot view file\nno folder for temporary files"
        );
    }

    #[gpui_kit::test]
    fn the_watcher_and_state_use_the_archives_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = inside_zip(cx);
        let watched = file_manager(cx).read_with(cx, |this, _| this.watched[0].clone());
        assert_eq!(watched.as_deref(), Some(tmp.path()));
        let saved = cx.update(|_, cx| cx.global::<AppState>().state.left_tabs.clone());
        assert_eq!(saved, [tmp.path().to_path_buf()]);
    }

    #[gpui_kit::test]
    fn the_window_title_shows_the_path_inside(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = inside_zip(cx);
        let title = commander.read_with(cx, |c, _| super::super::window_title(c));
        assert_eq!(
            title,
            format!("{} - yagni-commander", tmp.path().join("pkg.zip").display())
        );
    }
}

mod properties {
    use super::*;

    fn done(cx: &mut VisualTestContext) -> bool {
        dialog_open(cx)
            && file_manager(cx).read_with(cx, |this, _| this.info.as_ref().is_none_or(|i| i.done()))
    }

    /// Opens Properties, waits for its details and count, returns its text.
    fn properties(cx: &mut VisualTestContext) -> String {
        cx.simulate_keystrokes("alt-enter");
        wait_until(cx, done);
        box_text(cx)
    }

    fn value<'a>(text: &'a str, label: &str) -> Option<&'a str> {
        text.lines()
            .find_map(|l| l.strip_prefix(label)?.strip_prefix(": "))
    }

    #[gpui_kit::test]
    fn alt_enter_on_a_file(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("f"), b"12345").unwrap();
        cx.simulate_keystrokes("end");
        let text = properties(cx);
        assert!(text.starts_with("Properties\n"), "{text}");
        assert_eq!(value(&text, "Name"), Some("f"));
        assert_eq!(
            value(&text, "Folder"),
            Some(tmp.path().display().to_string().as_str())
        );
        assert_eq!(value(&text, "Type"), Some("File"));
        assert_eq!(value(&text, "Size"), Some("5 bytes (5 B)"));
        assert!(value(&text, "Contains").is_none());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert!(file_manager(cx).read_with(cx, |this, _| this.info.is_none()));
    }

    #[gpui_kit::test]
    fn alt_enter_on_a_folder_counts_it(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("a/x"), b"123").unwrap();
        std::fs::create_dir(tmp.path().join("a/sub")).unwrap();
        cx.simulate_keystrokes("home down"); // a
        let text = properties(cx);
        assert_eq!(value(&text, "Type"), Some("Folder"));
        assert_eq!(value(&text, "Contains"), Some("1 file, 1 folder"));
        assert_eq!(value(&text, "Size"), Some("3 bytes (3 B)"));
        cx.simulate_keystrokes("enter"); // OK
        cx.run_until_parked();
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_symlink_to_a_folder_counts_its_target(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        std::fs::write(tmp.path().join("a/x"), b"123").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("a"), tmp.path().join("link")).unwrap();
        cx.simulate_keystrokes("ctrl-r");
        cx.run_until_parked();
        cx.simulate_keystrokes("l");
        cx.simulate_keystrokes("escape");
        let text = properties(cx);
        assert_eq!(value(&text, "Name"), Some("link"));
        assert!(value(&text, "Type").is_some_and(|t| t.starts_with("Symbolic link to ")));
        assert_eq!(value(&text, "Contains"), Some("1 file, 0 folders"));
        assert_eq!(value(&text, "Size"), Some("3 bytes (3 B)"));
    }

    #[gpui_kit::test]
    fn on_dot_dot_it_shows_the_current_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("home");
        let text = properties(cx);
        let name = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(value(&text, "Name"), Some(name.as_str()));
        let parent = tmp.path().parent().unwrap().display().to_string();
        assert_eq!(value(&text, "Folder"), Some(parent.as_str()));
        assert_eq!(
            value(&text, "Contains"),
            Some("2 files, 2 folders"),
            "f, .dot, a, b"
        );
    }

    #[gpui_kit::test]
    fn inside_an_archive_from_the_index(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = super::archive_browsing::inside_zip(cx);
        cx.simulate_keystrokes("home down"); // src
        let text = properties(cx);
        assert_eq!(value(&text, "Contains"), Some("2 files, 1 folder"));
        assert!(value(&text, "Accessed").is_none());
        let zip = tmp.path().join("pkg.zip").display().to_string();
        assert_eq!(value(&text, "Folder"), Some(zip.as_str()));
        cx.simulate_keystrokes("escape home");
        cx.run_until_parked();
        let root = properties(cx); // ".." at the archive's root
        assert_eq!(value(&root, "Name"), Some("pkg.zip"));
        assert_eq!(value(&root, "Type"), Some("Folder"));
        assert_eq!(value(&root, "Contains"), Some("3 files, 2 folders"));
    }

    #[gpui_kit::test]
    fn the_menu_item_opens_it_too(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("end");
        cx.dispatch_action(crate::actions::ShowProperties);
        wait_until(cx, done);
        assert!(box_text(cx).contains("Name: f"));
    }

    #[gpui_kit::test]
    fn a_closed_box_stops_its_count_and_ignores_its_old_results(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        cx.simulate_keystrokes("home down alt-enter");
        let cancel = file_manager(cx).read_with(cx, |this, _| this.info.as_ref().unwrap().cancel());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed), "stopped");
        assert!(file_manager(cx).read_with(cx, |this, _| this.info.is_none()));
        std::fs::write(tmp.path().join("f"), b"1").unwrap();
        cx.simulate_keystrokes("end");
        let text = properties(cx);
        assert_eq!(value(&text, "Name"), Some("f"), "the new box's own lines");
        assert_eq!(value(&text, "Size"), Some("1 byte (1 B)"));
    }

    #[gpui_kit::test]
    fn ignored_while_loading(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = open(cx);
        enter_held_a(&tmp, cx);
        assert!(loading(&commander, cx));
        cx.simulate_keystrokes("alt-enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        release(&tmp);
    }
}

mod results_panel {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use yagni_commander_core::find::{self, Query, masks::Masks};

    /// `open`'s folder with `src/main.rs` and `src/sub/lib.rs`, the left
    /// panel showing the `*.rs` results.
    fn fed(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let (tmp, commander, cx) = open(cx);
        std::fs::create_dir_all(tmp.path().join("src/sub")).unwrap();
        std::fs::write(tmp.path().join("src/main.rs"), b"x").unwrap();
        std::fs::write(tmp.path().join("src/sub/lib.rs"), b"y").unwrap();
        let query = Query {
            root: tmp.path().into(),
            masks: Masks::parse("*.rs"),
            skip: Vec::new(),
            text: None,
        };
        let mut found = Vec::new();
        find::search(
            &query,
            &AtomicBool::new(false),
            &find::Progress::default(),
            &mut |f| found.push(f),
        );
        let results =
            find::Results::new(tmp.path().into(), "*.rs".into(), std::sync::Arc::new(found));
        commander.update(cx, |c, cx| {
            // Fed from the right: the left side shows them, active.
            c.set_active(Side::Right);
            c.feed(results);
            cx.notify();
        });
        cx.run_until_parked();
        (tmp, commander, cx)
    }

    fn labels(commander: &Entity<Commander>, side: Side, cx: &VisualTestContext) -> Vec<String> {
        commander.read_with(cx, |c, _| {
            c.panel(side)
                .entries()
                .iter()
                .map(|e| e.label.clone())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn f2_f7_and_shift_f4_are_refused(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = fed(cx);
        for key in ["down f2", "f7", "shift-f4"] {
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
            assert!(dialog_open(cx), "{key}");
            let text = box_text(cx);
            assert!(text.contains(find::IN_RESULTS), "{key}: {text}");
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(!dialog_open(cx), "{key}");
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 5);
    }

    #[gpui_kit::test]
    fn f5_on_one_result_targets_its_file_name(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = fed(cx);
        // The right panel into `a`; the left cursor on src/main.rs.
        cx.simulate_keystrokes("tab down enter tab down");
        cx.simulate_keystrokes("f5");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(tmp.path().join("a/main.rs").is_file());
        assert_eq!(
            labels(&commander, Side::Left, cx),
            ["..", "src/main.rs", "src/sub/lib.rs"],
            "still the results after the reload"
        );
    }

    #[gpui_kit::test]
    fn f8_drops_the_trashed_result(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = fed(cx);
        use_fake_trash(cx);
        cx.simulate_keystrokes("down f8");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        wait_until(cx, |cx| !job_running(cx));
        assert!(!tmp.path().join("src/main.rs").exists());
        assert_eq!(labels(&commander, Side::Left, cx), ["..", "src/sub/lib.rs"]);
    }

    #[gpui_kit::test]
    fn the_header_and_tab_say_results(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = fed(cx);
        commander.read_with(cx, |c, _| {
            let panel = c.panel(Side::Left);
            assert_eq!(
                crate::panel_view::header_parts(panel),
                (
                    Some("Results: *.rs in ".to_owned()),
                    tmp.path().display().to_string()
                )
            );
            assert_eq!(crate::panel_view::tab_label(panel), "Results");
            assert_eq!(
                crate::panel_view::header_parts(c.panel(Side::Right)),
                (None, tmp.path().display().to_string())
            );
        });
    }

    #[gpui_kit::test]
    fn a_narrow_header_shortens_the_path_not_the_results_prefix(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = fed(cx);
        let prefix = bounds(cx, "results-prefix-left".into()).unwrap().size.width;
        // The divider far left: the left header gets very narrow.
        let start = center(cx, "divider");
        let target = gpui_kit::point(gpui_kit::px(40.0), start.y);
        let left = gpui_kit::MouseButton::Left;
        let modifiers = gpui_kit::Modifiers::default();
        cx.simulate_mouse_down(start, left, modifiers);
        cx.simulate_mouse_move(target, left, modifiers);
        cx.simulate_mouse_up(target, left, modifiers);
        assert_eq!(
            bounds(cx, "results-prefix-left".into()).unwrap().size.width,
            prefix
        );
    }
}

mod find_files {
    use super::*;
    use crate::find_dialog::{Field, FindDialog, Status, status_text};
    use yagni_commander_core::find::SearchSummary;

    /// `open`'s folder plus `a/x.rs` and `b/y.txt` (containing "needle").
    fn opened(
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, Entity<Commander>, &mut VisualTestContext) {
        let (tmp, commander, cx) = open(cx);
        std::fs::write(tmp.path().join("a/x.rs"), b"fn main").unwrap();
        std::fs::write(tmp.path().join("b/y.txt"), b"a needle here").unwrap();
        activate(cx);
        (tmp, commander, cx)
    }

    fn dialog(cx: &mut VisualTestContext) -> Entity<FindDialog> {
        file_manager(cx).read_with(cx, |this, _| this.find.clone().expect("find dialog"))
    }

    fn running(cx: &mut VisualTestContext) -> bool {
        let view = dialog(cx);
        view.read_with(cx, |v, _| v.running())
    }

    fn results(cx: &mut VisualTestContext) -> Vec<String> {
        let view = dialog(cx);
        view.read_with(cx, |v, _| v.result_labels())
    }

    fn set(cx: &mut VisualTestContext, field: Field, text: &str) {
        let view = dialog(cx);
        view.update_in(cx, |v, window, cx| v.set_field(field, text, window, cx));
    }

    /// Alt-F7, the masks typed, Enter; waits for the search to end.
    fn search(cx: &mut VisualTestContext, masks: &str) {
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Masks, masks);
        run_search(cx);
    }

    fn run_search(cx: &mut VisualTestContext) {
        let view = dialog(cx);
        view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        wait_until(cx, |cx| !running(cx));
    }

    #[gpui_kit::test]
    fn alt_f7_and_the_menu_action_open_the_dialog(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        assert!(dialog_open(cx));
        let view = dialog(cx);
        let search_in = view.read_with(cx, |v, cx| v.field(Field::SearchIn, cx));
        assert_eq!(search_in, tmp.path().display().to_string());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        cx.dispatch_action(crate::actions::FindFiles);
        cx.run_until_parked();
        assert!(dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_name_search_lists_results_and_enter_goes_to_the_file(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = opened(cx);
        search(cx, "*.rs");
        assert_eq!(results(cx), ["a/x.rs"]);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("a"));
        let at = commander.read_with(cx, |c, _| {
            c.panel(Side::Left).cursor_entry().unwrap().label.clone()
        });
        assert_eq!(at, "x.rs");
        // The panel has the keys again.
        cx.simulate_keystrokes("home");
        assert_eq!(cursor(&commander, Side::Left, cx), 0);
    }

    #[gpui_kit::test]
    fn the_results_list_takes_arrow_keys(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = opened(cx);
        search(cx, "*.rs *.txt");
        assert_eq!(results(cx), ["a/x.rs", "b/y.txt"]);
        cx.simulate_keystrokes("down down up end home pagedown enter");
        cx.run_until_parked();
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    fn drawn(cx: &mut VisualTestContext, selector: &str) -> bool {
        bounds(cx, selector.to_owned()).is_some()
    }

    #[gpui_kit::test]
    fn focus_shows_on_one_control_at_a_time(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        search(cx, "*.rs *.txt");
        // The list has focus after a search: a bright cursor, no rings.
        assert!(drawn(cx, "find-cursor-focused"));
        assert!(!drawn(cx, "focus-ring-button"));
        // The buttons: a ring on the selected one, the cursor muted.
        cx.simulate_keystrokes("tab");
        assert!(drawn(cx, "focus-ring-button"));
        assert!(!drawn(cx, "find-cursor-focused"));
        assert!(drawn(cx, "find-cursor"), "still shown, muted");
        cx.simulate_keystrokes("shift-tab");
        assert!(drawn(cx, "find-cursor-focused"));
        assert!(!drawn(cx, "focus-ring-button"));
        // Back again: the last option box, which Space toggles.
        cx.simulate_keystrokes("shift-tab");
        assert!(drawn(cx, "focus-ring-find-not"));
        assert!(!drawn(cx, "find-cursor-focused"));
        let view = dialog(cx);
        assert!(!view.read_with(cx, |v, _| v.options()[4]));
        cx.simulate_keystrokes("space");
        assert!(view.read_with(cx, |v, _| v.options()[4]));
        cx.simulate_keystrokes("space");
        assert!(!view.read_with(cx, |v, _| v.options()[4]));
        // A text field: its own ring, none of ours.
        view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
        assert!(!drawn(cx, "focus-ring-find-not"));
        assert!(!drawn(cx, "focus-ring-button"));
        assert!(!drawn(cx, "find-cursor-focused"));
    }

    #[gpui_kit::test]
    fn the_grip_sits_square_in_the_dialogs_corner(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        let dialog = bounds(cx, "dialog-0".into()).unwrap();
        let grip = bounds(cx, "find-grip-dots".into()).unwrap();
        let right = dialog.right() - grip.right();
        let bottom = dialog.bottom() - grip.bottom();
        assert_eq!(right, bottom, "the same inset from both edges");
        assert!(
            right >= gpui_kit::px(4.0) && right <= gpui_kit::px(8.0),
            "{right:?}"
        );
    }

    #[gpui_kit::test]
    fn the_dialog_resizes_from_its_corner_but_not_below_its_size(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        search(cx, "*.rs");
        let list = |cx: &mut VisualTestContext| bounds(cx, "find-list".into()).unwrap().size;
        let before = list(cx);
        let grip = center(cx, "find-grip");
        let drag = |cx: &mut VisualTestContext, dx: f32, dy: f32| {
            let left = gpui_kit::MouseButton::Left;
            let modifiers = gpui_kit::Modifiers::default();
            let to = gpui_kit::point(grip.x + gpui_kit::px(dx), grip.y + gpui_kit::px(dy));
            cx.simulate_mouse_down(grip, left, modifiers);
            cx.simulate_mouse_move(to, left, modifiers);
            cx.simulate_mouse_up(to, left, modifiers);
            cx.run_until_parked();
        };
        drag(cx, 50.0, 80.0);
        let bigger = list(cx);
        // Centered: the corner follows the mouse, so the width grows twice.
        assert_eq!(bigger.width, before.width + gpui_kit::px(100.0));
        assert_eq!(bigger.height, before.height + gpui_kit::px(80.0));
        // Released: moving on changes nothing.
        cx.simulate_mouse_move(grip, None, gpui_kit::Modifiers::default());
        assert_eq!(list(cx), bigger);
        // Never smaller than it opened.
        let grip_now = center(cx, "find-grip");
        let left = gpui_kit::MouseButton::Left;
        let modifiers = gpui_kit::Modifiers::default();
        let far = gpui_kit::point(gpui_kit::px(1.0), gpui_kit::px(1.0));
        cx.simulate_mouse_down(grip_now, left, modifiers);
        cx.simulate_mouse_move(far, left, modifiers);
        cx.simulate_mouse_up(far, left, modifiers);
        assert_eq!(list(cx), before);
        // The size stays for the next Alt-F7.
        drag(cx, 50.0, 80.0);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        assert_eq!(list(cx), bigger);
    }

    #[gpui_kit::test]
    fn double_click_goes_to_the_file(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = opened(cx);
        search(cx, "y.txt");
        click(cx, "find-result-0", 2);
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(path(&commander, Side::Left, cx), tmp.path().join("b"));
    }

    #[gpui_kit::test]
    fn a_content_search_and_its_options(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Text, "NEEDLE");
        run_search(cx);
        assert_eq!(results(cx), ["b/y.txt"]);
        click(cx, "find-case", 1);
        run_search(cx);
        assert!(results(cx).is_empty(), "case-sensitive");
        click(cx, "find-case", 1);
        click(cx, "find-not", 1);
        set(cx, Field::Masks, "*.rs *.txt");
        run_search(cx);
        assert_eq!(results(cx), ["a/x.rs"], "not containing");
        click(cx, "find-not", 1);
        click(cx, "find-regex", 1);
        click(cx, "find-words", 1);
        set(cx, Field::Text, "ne+dle|fn");
        run_search(cx);
        assert_eq!(results(cx), ["a/x.rs", "b/y.txt"]);
    }

    /// `opened`'s folder plus `.git/g.rs`, `node_modules/n.rs`, `bin/b.rs`.
    fn with_tooling(tmp: &tempfile::TempDir) {
        for dir in [".git", "node_modules", "bin"] {
            std::fs::create_dir(tmp.path().join(dir)).unwrap();
            std::fs::write(tmp.path().join(dir).join("x.rs"), b"").unwrap();
        }
    }

    #[gpui_kit::test]
    fn git_and_node_modules_are_skipped_by_default(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        with_tooling(&tmp);
        search(cx, "*.rs");
        assert_eq!(results(cx), ["a/x.rs", "bin/x.rs"]);
        let status = dialog(cx).read_with(cx, |v, _| v.status());
        assert_eq!(status, "2 found, 2 folders skipped");
        assert_eq!(
            dialog(cx).read_with(cx, |v, _| v.skip_text()),
            ".git, node_modules"
        );
    }

    #[gpui_kit::test]
    fn the_skip_popup_toggles_folders_and_remembers_them(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        with_tooling(&tmp);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        click(cx, "find-skip", 1);
        assert!(drawn(cx, "find-skip-popup"));
        // .git, node_modules, bin: Space turns bin on, then .git off.
        cx.simulate_keystrokes("down down space up up space");
        // Enter closes the popup, not the dialog, and searches nothing.
        cx.simulate_keystrokes("enter");
        assert!(!drawn(cx, "find-skip-popup"));
        assert!(dialog_open(cx));
        assert!(!running(cx) && results(cx).is_empty());
        assert_eq!(
            dialog(cx).read_with(cx, |v, _| v.skip_text()),
            "node_modules, bin"
        );
        set(cx, Field::Masks, "*.rs");
        run_search(cx);
        assert_eq!(results(cx), [".git/x.rs", "a/x.rs"]);
        // Saved in the state file's data, for the next start.
        let saved = cx.update(|_, cx| cx.global::<AppState>().state.find_skip.clone());
        assert_eq!(
            saved,
            Some(vec!["node_modules".to_owned(), "bin".to_owned()])
        );
        // Escape closes the popup only, too.
        click(cx, "find-skip", 1);
        cx.simulate_keystrokes("escape");
        assert!(!drawn(cx, "find-skip-popup"));
        assert!(dialog_open(cx));
    }

    #[gpui_kit::test]
    fn a_click_on_a_skip_popup_row_toggles_it(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        click(cx, "find-skip", 1);
        click(cx, "find-skip-row-3", 1); // obj
        assert_eq!(
            dialog(cx).read_with(cx, |v, _| v.skip_text()),
            ".git, node_modules, obj"
        );
    }

    #[gpui_kit::test]
    fn the_saved_skip_list_is_used_on_the_next_start(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        with_tooling(&tmp);
        cx.update(|_, cx| {
            cx.global_mut::<AppState>().state.find_skip = Some(vec!["bin".to_owned()]);
        });
        search(cx, "*.rs");
        assert_eq!(results(cx), [".git/x.rs", "a/x.rs", "node_modules/x.rs"]);
    }

    #[test]
    fn skip_texts() {
        use crate::find_dialog::skip_text;
        let on = |names: &[&str]| -> Vec<bool> {
            yagni_commander_core::find::SKIP_FOLDERS
                .iter()
                .map(|f| names.contains(f))
                .collect()
        };
        assert_eq!(skip_text(&on(&[])), "none");
        assert_eq!(skip_text(&on(&[".git", "bin", "obj"])), ".git, bin, obj");
        assert_eq!(
            skip_text(&on(&[".git", "node_modules", "bin", "obj", "target"])),
            ".git, node_modules +3"
        );
    }

    #[gpui_kit::test]
    fn names_and_text_each_have_a_case_box(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        std::fs::write(tmp.path().join("README"), b"").unwrap();
        std::fs::write(tmp.path().join("readme.md"), b"").unwrap();
        search(cx, "README");
        assert_eq!(results(cx), ["README", "readme.md"]);
        // The text box leaves names alone.
        click(cx, "find-case", 1);
        run_search(cx);
        assert_eq!(results(cx), ["README", "readme.md"]);
        click(cx, "find-case", 1);
        click(cx, "find-case-names", 1);
        run_search(cx);
        assert_eq!(results(cx), ["README"]);
    }

    #[gpui_kit::test]
    fn case_sensitive_text_keeps_names_case_insensitive(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        std::fs::write(tmp.path().join("UserService.cs"), b"var client = 1;").unwrap();
        std::fs::write(tmp.path().join("other-service.txt"), b"Client only").unwrap();
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Text, "client");
        click(cx, "find-case", 1);
        set(cx, Field::Masks, "*service*");
        run_search(cx);
        assert_eq!(results(cx), ["UserService.cs"]);
    }

    #[gpui_kit::test]
    fn the_dialog_remembers_fields_and_results_and_resets_search_in(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        search(cx, "*.rs");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        cx.simulate_keystrokes("down enter"); // into a
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        let view = dialog(cx);
        let (masks, search_in) = view.read_with(cx, |v, cx| {
            (v.field(Field::Masks, cx), v.field(Field::SearchIn, cx))
        });
        assert_eq!(masks, "*.rs");
        assert_eq!(search_in, tmp.path().join("a").display().to_string());
        assert_eq!(results(cx), ["a/x.rs"]);
    }

    #[gpui_kit::test]
    fn escape_stops_a_search_then_closes(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        let view = dialog_with_hold(cx);
        set(cx, Field::Masks, "*");
        view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(running(cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(dialog_open(cx), "the first Escape stops");
        assert!(!running(cx));
        let status = view.read_with(cx, |v, _| v.status());
        assert!(status.ends_with(", stopped"), "{status}");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(cx));
    }

    /// Opens the dialog with its next search held before the walk.
    fn dialog_with_hold(cx: &mut VisualTestContext) -> Entity<FindDialog> {
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        let view = dialog(cx);
        view.update(cx, |v, _| v.hold = true);
        view
    }

    #[gpui_kit::test]
    fn a_closed_search_never_adds_to_the_next_one(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        let view = dialog_with_hold(cx);
        set(cx, Field::Masks, "*.txt");
        view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let first = view.read_with(cx, |v, _| v.held().unwrap());
        // Close stops it; a new search starts while the first is held.
        click(cx, "button-Close", 1);
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        search(cx, "*.rs");
        first.store(false, std::sync::atomic::Ordering::Relaxed);
        for _ in 0..5 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(200));
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(results(cx), ["a/x.rs"]);
    }

    #[gpui_kit::test]
    fn feed_to_panel_fills_the_other_side_and_activates_it(cx: &mut TestAppContext) {
        let (tmp, commander, cx) = opened(cx);
        search(cx, "*.rs *.txt");
        click(cx, "button-Feed to panel", 1);
        cx.run_until_parked();
        assert!(!dialog_open(cx));
        assert_eq!(commander.read_with(cx, |c, _| c.active()), Side::Right);
        assert!(!commander.read_with(cx, |c, _| c.panel(Side::Left).in_results()));
        let (in_results, labels) = commander.read_with(cx, |c, _| {
            let panel = c.panel(Side::Right);
            (
                panel.in_results(),
                panel
                    .entries()
                    .iter()
                    .map(|e| e.label.clone())
                    .collect::<Vec<_>>(),
            )
        });
        assert!(in_results);
        assert_eq!(labels, ["..", "a/x.rs", "b/y.txt"]);
        assert_eq!(path(&commander, Side::Right, cx), tmp.path());
        // The keys go to the results.
        cx.simulate_keystrokes("end");
        assert_eq!(cursor(&commander, Side::Right, cx), 2);
    }

    #[gpui_kit::test]
    fn feed_shares_the_dialogs_results_instead_of_copying_them(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = opened(cx);
        search(cx, "*.rs *.txt");
        let view = dialog(cx);
        let found = view.read_with(cx, |v, _| v.found_entries());
        click(cx, "button-Feed to panel", 1);
        cx.run_until_parked();
        let shared = commander.read_with(cx, |c, _| {
            std::sync::Arc::ptr_eq(&c.panel(c.active()).results().unwrap().entries, &found)
        });
        assert!(shared);
    }

    #[gpui_kit::test]
    fn alt_l_feeds_to_the_panel_from_anywhere_in_the_dialog(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = opened(cx);
        let fed = |cx: &mut VisualTestContext| {
            commander.read_with(cx, |c, _| c.panel(c.active()).in_results())
        };
        // From the results list (focused after a search).
        search(cx, "*.rs");
        cx.simulate_keystrokes("alt-l");
        cx.run_until_parked();
        assert!(!dialog_open(cx) && fed(cx), "from the list");
        // From a text field: nothing typed into it.
        cx.simulate_keystrokes("backspace");
        search(cx, "*.rs");
        let view = dialog(cx);
        view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
        cx.simulate_keystrokes("alt-l");
        cx.run_until_parked();
        assert!(!dialog_open(cx) && fed(cx), "from a field");
        assert_eq!(
            view.read_with(cx, |v, cx| v.field(Field::Masks, cx)),
            "*.rs"
        );
        // From the buttons.
        cx.simulate_keystrokes("backspace");
        search(cx, "*.rs");
        cx.simulate_keystrokes("tab alt-l");
        cx.run_until_parked();
        assert!(!dialog_open(cx) && fed(cx), "from the buttons");
    }

    #[gpui_kit::test]
    fn feed_does_nothing_without_results(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = opened(cx);
        search(cx, "*.none");
        click(cx, "button-Feed to panel", 1);
        cx.run_until_parked();
        assert!(dialog_open(cx));
        assert!(!commander.read_with(cx, |c, _| c.panel(Side::Left).in_results()));
    }

    #[gpui_kit::test]
    fn f3_on_a_result_opens_a_viewer_and_the_dialog_stays(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        search(cx, "*.txt");
        cx.simulate_keystrokes("f3");
        cx.run_until_parked();
        assert_eq!(viewers(cx), 1);
        assert!(dialog_open(cx));
    }

    /// The match the viewer opened by F3 on a result shows, once its
    /// search ends.
    fn viewer_match(cx: &mut VisualTestContext) -> Option<std::ops::Range<u64>> {
        let viewer = cx
            .windows()
            .into_iter()
            .find_map(|w| crate::viewer_view::tests::viewer_in(w, cx))
            .expect("a viewer");
        wait_until(cx, |cx| !viewer.read_with(cx, |v, _| v.searching()));
        viewer.read_with(cx, |v, _| v.current_match())
    }

    #[gpui_kit::test]
    fn f3_after_a_text_search_shows_the_first_match(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Masks, "*.txt");
        set(cx, Field::Text, "NEEDLE");
        run_search(cx);
        // The fields changed since: the search's own text counts.
        set(cx, Field::Text, "here");
        cx.simulate_keystrokes("f3");
        cx.run_until_parked();
        assert_eq!(viewer_match(cx), Some(2..8));
    }

    #[gpui_kit::test]
    fn f3_in_a_fed_panel_shows_the_first_match_too(cx: &mut TestAppContext) {
        let (_tmp, commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Masks, "*.txt");
        set(cx, Field::Text, "NEEDLE");
        run_search(cx);
        cx.simulate_keystrokes("alt-l");
        cx.run_until_parked();
        assert!(commander.read_with(cx, |c, _| c.panel(c.active()).in_results()));
        // Ctrl-R re-checks the results and keeps the text.
        cx.simulate_keystrokes("ctrl-r end f3");
        cx.run_until_parked();
        assert_eq!(viewer_match(cx), Some(2..8));
    }

    #[gpui_kit::test]
    fn f3_after_a_not_containing_search_just_views(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        set(cx, Field::Masks, "*.txt");
        set(cx, Field::Text, "absent");
        click(cx, "find-not", 1);
        run_search(cx);
        cx.simulate_keystrokes("f3");
        cx.run_until_parked();
        assert_eq!(viewer_match(cx), None);
    }

    #[gpui_kit::test]
    fn bad_input_shows_an_error_and_keeps_the_dialog(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = opened(cx);
        cx.simulate_keystrokes("alt-f7");
        cx.run_until_parked();
        let gone = tmp.path().join("gone").display().to_string();
        let cases = [
            (Field::SearchIn, gone.as_str(), "is not a folder"),
            (Field::Text, "(", "Invalid regular expression"),
        ];
        click(cx, "find-regex", 1);
        for (field, text, message) in cases {
            set(cx, field, text);
            let view = dialog(cx);
            view.update_in(cx, |v, window, cx| v.focus_field(Field::Masks, window, cx));
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            let shown = box_text(cx);
            assert!(shown.contains(message), "{shown}");
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(dialog_open(cx), "the find dialog stays");
            assert!(!running(cx));
            // Put it right for the next case.
            let fixed = match field {
                Field::SearchIn => tmp.path().display().to_string(),
                _ => String::new(),
            };
            set(cx, field, &fixed);
        }
    }

    #[test]
    fn status_lines() {
        assert_eq!(status_text(&Status::Idle), "");
        assert_eq!(
            status_text(&Status::Running {
                seen: 12345,
                found: 12
            }),
            "Searching... 12,345 files, 12 found"
        );
        let done = SearchSummary {
            found: 12,
            unreadable: 3,
            other_filesystems: 1,
            stopped: true,
            ..SearchSummary::default()
        };
        assert_eq!(
            status_text(&Status::Done(done)),
            "12 found, 3 unreadable, 1 other filesystem skipped, stopped"
        );
        let two = SearchSummary {
            other_filesystems: 2,
            ..SearchSummary::default()
        };
        assert_eq!(
            status_text(&Status::Done(two)),
            "0 found, 2 other filesystems skipped"
        );
        let skipped = SearchSummary {
            skipped: 1,
            ..SearchSummary::default()
        };
        assert_eq!(
            status_text(&Status::Done(skipped)),
            "0 found, 1 folder skipped"
        );
    }
}

mod open_terminal {
    use super::*;

    /// A stand-in terminal that writes the folder it started in to `out`.
    fn use_recording_terminal(
        dir: &std::path::Path,
        cx: &mut VisualTestContext,
    ) -> std::path::PathBuf {
        let out = dir.join("terminal-ran-in");
        let command = format!("sh -c 'pwd > \"{}\"'", out.display());
        file_manager(cx).update(cx, |this, _| this.terminal = Some(command));
        out
    }

    fn started_in(out: &std::path::Path, cx: &mut VisualTestContext) -> String {
        wait_until(cx, |_| {
            std::fs::read_to_string(out).is_ok_and(|s| s.ends_with('\n'))
        });
        std::fs::read_to_string(out).unwrap().trim().to_owned()
    }

    #[gpui_kit::test]
    fn ctrl_shift_t_starts_a_terminal_in_the_active_panels_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let out = use_recording_terminal(tmp.path(), cx);
        cx.simulate_keystrokes("tab down enter"); // the right panel into a
        cx.simulate_keystrokes("ctrl-shift-t");
        assert_eq!(
            started_in(&out, cx),
            tmp.path().join("a").display().to_string()
        );
        assert!(!dialog_open(cx));
    }

    #[gpui_kit::test]
    fn the_menu_item_does_the_same(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = open(cx);
        let out = use_recording_terminal(tmp.path(), cx);
        cx.dispatch_action(crate::actions::OpenTerminal);
        assert_eq!(started_in(&out, cx), tmp.path().display().to_string());
    }

    #[gpui_kit::test]
    fn inside_an_archive_it_starts_in_the_archives_folder(cx: &mut TestAppContext) {
        let (tmp, _commander, cx) = super::archive_browsing::inside_zip(cx);
        let out = use_recording_terminal(tmp.path(), cx);
        cx.simulate_keystrokes("ctrl-shift-t");
        assert_eq!(started_in(&out, cx), tmp.path().display().to_string());
    }

    #[gpui_kit::test]
    fn a_terminal_that_cannot_start_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, _commander, cx) = open(cx);
        file_manager(cx).update(cx, |this, _| {
            this.terminal = Some("no-such-terminal-here".into())
        });
        cx.simulate_keystrokes("ctrl-shift-t");
        wait_until(cx, dialog_open);
        let text = box_text(cx);
        assert!(text.starts_with("Cannot open a terminal"), "{text}");
        assert!(text.contains("no-such-terminal-here"), "{text}");
    }
}
