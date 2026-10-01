use super::*;
use gpui_kit::{TestAppContext, VisualTestContext};

/// The globals and keymap a file manager window needs.
fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Theme::default());
        cx.set_global(AppState::default());
        cx.set_global(crate::CurrentConfig(Default::default()));
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
fn f12_button_test_reports_the_pressed_button(cx: &mut TestAppContext) {
    let (_tmp, _commander, cx) = open(cx);
    cx.simulate_keystrokes("f12");
    cx.run_until_parked();
    cx.simulate_keystrokes("right enter");
    cx.run_until_parked();
    assert!(!dialog_open(cx));
    let notice = file_manager(cx).read_with(cx, |this, _| this.notice.clone());
    assert!(notice.unwrap().contains("“Four”"));
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
    for key in [
        "f2", "f4", "f5", "f6", "f7", "f8", "shift-f8", "shift-f4", "f12",
    ] {
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
fn panel_paths_and_active_side_are_remembered(cx: &mut TestAppContext) {
    let (tmp, _commander, cx) = open(cx);
    let state = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            let s = &cx.global::<AppState>().state;
            (s.left.clone(), s.right.clone(), s.active)
        })
    };
    let root = Some(tmp.path().to_path_buf());
    assert_eq!(state(cx), (root.clone(), root.clone(), Some(Side::Left)));
    cx.simulate_keystrokes("tab down enter");
    cx.run_until_parked();
    assert_eq!(
        state(cx),
        (root, Some(tmp.path().join("a")), Some(Side::Right))
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
