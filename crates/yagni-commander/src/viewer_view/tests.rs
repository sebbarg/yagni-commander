use super::*;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, VisualTestContext, point, px, size};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Theme::default());
        cx.set_global(AppState::default());
        cx.set_global(crate::config_state::CurrentConfig::default());
        crate::actions::bind_default_keys(cx);
    });
}

fn main_bounds() -> WindowBounds {
    WindowBounds::Windowed(gpui_kit::Bounds::new(
        point(px(0.0), px(0.0)),
        size(px(800.0), px(600.0)),
    ))
}

/// The viewer in `window`, if that window holds one.
pub(crate) fn viewer_in(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) -> Option<Entity<ViewerView>> {
    window
        .update(cx, |_, window, cx| {
            window
                .root::<gpui_kit::base::Root>()
                .flatten()
                .and_then(|root| root.read(cx).view().clone().downcast::<ViewerView>().ok())
        })
        .ok()
        .flatten()
}

/// Opens a viewer on a file with `text`; returns its window context and view.
fn view(
    text: &[u8],
    cx: &mut TestAppContext,
) -> (tempfile::TempDir, Entity<ViewerView>, VisualTestContext) {
    setup(cx);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("file.txt");
    std::fs::write(&path, text).unwrap();
    cx.update(|cx| open(path, main_bounds(), None, None, cx))
        .unwrap();
    let window = *cx.windows().last().unwrap();
    let viewer = viewer_in(window, cx).expect("a viewer window");
    let vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    (tmp, viewer, vcx)
}

fn numbered(lines: usize) -> Vec<u8> {
    (1..=lines)
        .map(|n| format!("line {n}\n"))
        .collect::<String>()
        .into_bytes()
}

fn shown(viewer: &Entity<ViewerView>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.run_until_parked();
    viewer.read_with(cx, |v, _| v.shown().to_vec())
}

#[gpui_kit::test]
fn shows_the_start_of_the_file(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
    let rows = shown(&viewer, &mut cx);
    assert_eq!(rows[0], "line 1");
    assert_eq!(rows.len(), viewer.read_with(&cx, |v, _| v.screen_rows()));
}

#[gpui_kit::test]
fn arrows_and_pages_scroll(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
    let screen = viewer.read_with(&cx, |v, _| v.screen_rows());
    cx.simulate_keystrokes("down down");
    assert_eq!(shown(&viewer, &mut cx)[0], "line 3");
    cx.simulate_keystrokes("up");
    assert_eq!(shown(&viewer, &mut cx)[0], "line 2");
    cx.simulate_keystrokes("pagedown");
    assert_eq!(
        shown(&viewer, &mut cx)[0],
        format!("line {}", 2 + screen - 1)
    );
    cx.simulate_keystrokes("pageup");
    assert_eq!(shown(&viewer, &mut cx)[0], "line 2");
}

#[gpui_kit::test]
fn end_and_home_jump(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
    cx.simulate_keystrokes("ctrl-end");
    assert_eq!(shown(&viewer, &mut cx).last().unwrap(), "line 1000");
    cx.simulate_keystrokes("down");
    assert_eq!(
        shown(&viewer, &mut cx).last().unwrap(),
        "line 1000",
        "no blank space below"
    );
    cx.simulate_keystrokes("home");
    assert_eq!(shown(&viewer, &mut cx)[0], "line 1");
    cx.simulate_keystrokes("end ctrl-home");
    assert_eq!(shown(&viewer, &mut cx)[0], "line 1");
}

#[gpui_kit::test]
fn w_toggles_wrap_and_keeps_the_top_line(cx: &mut TestAppContext) {
    let long = format!("{}\nnext\n", "word ".repeat(400));
    let (_tmp, viewer, mut cx) = view(long.as_bytes(), cx);
    assert!(viewer.read_with(&cx, |v, _| v.wraps()));
    assert!(shown(&viewer, &mut cx).len() > 2, "the long line wraps");
    cx.simulate_keystrokes("down w");
    assert!(!viewer.read_with(&cx, |v, _| v.wraps()));
    // The top row was inside line 1, so line 1 stays on top.
    assert!(shown(&viewer, &mut cx)[0].starts_with("word word"));
    assert_eq!(shown(&viewer, &mut cx)[1], "next");
    cx.simulate_keystrokes("w");
    assert!(viewer.read_with(&cx, |v, _| v.wraps()));
}

#[gpui_kit::test]
fn left_and_right_scroll_only_without_wrap(cx: &mut TestAppContext) {
    let long = format!("{}\n", "0123456789".repeat(100));
    let (_tmp, viewer, mut cx) = view(long.as_bytes(), cx);
    cx.simulate_keystrokes("right");
    assert_eq!(
        viewer.read_with(&cx, |v, _| v.h_offset()),
        0,
        "wrap mode ignores Right"
    );
    cx.simulate_keystrokes("w right right");
    assert_eq!(viewer.read_with(&cx, |v, _| v.h_offset()), 16);
    assert!(shown(&viewer, &mut cx)[0].starts_with("6789"));
    cx.simulate_keystrokes("left left left");
    assert_eq!(viewer.read_with(&cx, |v, _| v.h_offset()), 0);
}

fn assert_key_closes(key: &str, cx: &mut TestAppContext) {
    let (_tmp, _viewer, mut vcx) = view(b"text\n", cx);
    assert_eq!(cx.windows().len(), 1);
    vcx.simulate_keystrokes(key);
    vcx.run_until_parked();
    assert!(cx.windows().is_empty(), "{key}");
}

#[gpui_kit::test]
fn escape_closes_the_viewer(cx: &mut TestAppContext) {
    assert_key_closes("escape", cx);
}

#[gpui_kit::test]
fn q_closes_the_viewer(cx: &mut TestAppContext) {
    assert_key_closes("q", cx);
}

#[gpui_kit::test]
fn alt_f4_closes_only_the_viewer(cx: &mut TestAppContext) {
    // The global Alt-F4 is Quit; in a viewer it must close just that window.
    assert_key_closes("alt-f4", cx);
}

#[gpui_kit::test]
fn status_line_counts_lines_in_the_background(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(500), cx);
    cx.run_until_parked();
    let status = viewer.update(&mut cx, |v, _| v.status_text());
    assert!(status.contains("line 1 of 500"), "{status}");
    cx.simulate_keystrokes("down down");
    let status = viewer.update(&mut cx, |v, _| v.status_text());
    assert!(status.contains("line 3 of 500"), "{status}");
    assert!(
        status.contains("file.txt") && status.contains("wrap"),
        "{status}"
    );
}

#[gpui_kit::test]
fn empty_file_shows_nothing(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(b"", cx);
    assert!(shown(&viewer, &mut cx).is_empty());
    let status = viewer.update(&mut cx, |v, _| v.status_text());
    assert!(
        status.contains("0 lines") && !status.contains("line 1"),
        "{status}"
    );
    cx.simulate_keystrokes("end pagedown down up");
    assert!(shown(&viewer, &mut cx).is_empty());
}

#[gpui_kit::test]
fn window_title_names_the_file() {
    assert_eq!(
        window_title(std::path::Path::new("/a/b/notes.txt")),
        "notes.txt - yagni-commander"
    );
}

#[gpui_kit::test]
fn the_next_viewer_opens_where_the_last_one_was(cx: &mut TestAppContext) {
    let (_tmp, _viewer, vcx) = view(b"x\n", cx);
    vcx.run_until_parked();
    let saved = cx.update(|cx| cx.global::<AppState>().state.viewer);
    assert!(saved.is_some(), "the viewer's bounds are remembered");
}

fn bounds_of(
    cx: &mut VisualTestContext,
    selector: &'static str,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.run_until_parked();
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} not drawn"))
}

#[gpui_kit::test]
fn wheel_scrolls_rows(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
    let at = bounds_of(&mut cx, "viewer-row-0").center();
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: at,
        delta: gpui_kit::ScrollDelta::Lines(point(0.0, -3.0)),
        ..Default::default()
    });
    assert_eq!(shown(&viewer, &mut cx)[0], "line 4");
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: at,
        delta: gpui_kit::ScrollDelta::Lines(point(0.0, 1.0)),
        ..Default::default()
    });
    assert_eq!(shown(&viewer, &mut cx)[0], "line 3");
}

#[gpui_kit::test]
fn dragging_the_scrollbar_moves_by_byte_position(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(10_000), cx);
    let track = bounds_of(&mut cx, "viewer-scrollbar");
    let thumb = bounds_of(&mut cx, "viewer-thumb");
    let x = track.center().x;
    let modifiers = gpui_kit::Modifiers::default();
    let left = gpui_kit::MouseButton::Left;
    cx.simulate_mouse_down(thumb.center(), left, modifiers);
    let middle = point(x, track.origin.y + track.size.height / 2.0);
    cx.simulate_mouse_move(middle, left, modifiers);
    cx.simulate_mouse_up(middle, left, modifiers);
    let top = viewer.read_with(&cx, |v, _| v.top());
    let len = numbered(10_000).len() as u64;
    assert!(top > len * 4 / 10 && top < len * 6 / 10, "{top} of {len}");
    // Rows still start at line starts.
    assert!(shown(&viewer, &mut cx)[0].starts_with("line "));
    // Dragging to the bottom shows the end, without blank space.
    let bottom = point(x, track.origin.y + track.size.height);
    let thumb = bounds_of(&mut cx, "viewer-thumb");
    cx.simulate_mouse_down(thumb.center(), left, modifiers);
    cx.simulate_mouse_move(bottom, left, modifiers);
    cx.simulate_mouse_up(bottom, left, modifiers);
    assert_eq!(shown(&viewer, &mut cx).last().unwrap(), "line 10000");
}

#[gpui_kit::test]
fn clicking_the_track_jumps_there(cx: &mut TestAppContext) {
    let (_tmp, viewer, mut cx) = view(&numbered(10_000), cx);
    let track = bounds_of(&mut cx, "viewer-scrollbar");
    let at = point(track.center().x, track.origin.y + track.size.height * 0.75);
    let modifiers = gpui_kit::Modifiers::default();
    cx.simulate_mouse_down(at, gpui_kit::MouseButton::Left, modifiers);
    cx.simulate_mouse_up(at, gpui_kit::MouseButton::Left, modifiers);
    let top = viewer.read_with(&cx, |v, _| v.top());
    let len = numbered(10_000).len() as u64;
    assert!(top > len * 6 / 10, "{top} of {len}");
}

#[gpui_kit::test]
fn closing_the_main_window_closes_the_viewers(cx: &mut TestAppContext) {
    setup(cx);
    cx.add_empty_window();
    let main = cx.windows()[0];
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("f");
    std::fs::write(&path, b"x").unwrap();
    cx.update(|cx| {
        crate::windows::close_all_when_main_closes(main, cx).detach();
        open(path.clone(), main_bounds(), None, None, cx).unwrap();
        open(path, main_bounds(), None, None, cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 3);
    main.update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(cx.windows().is_empty());
}

mod search {
    use super::*;
    use crate::viewer_view::search::LastSearch;
    use gpui_kit::component::WindowExt;
    use yagni_commander_core::find::text::Text;

    /// Lets the running search end and its result arrive.
    fn settle(viewer: &Entity<ViewerView>, cx: &mut VisualTestContext) {
        for _ in 0..400 {
            cx.executor()
                .advance_clock(crate::file_manager::commands::OPENER_POLL);
            cx.run_until_parked();
            if !viewer.read_with(cx, |v, _| v.searching()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the search never ended");
    }

    fn dialog_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// Ctrl-F, `text` typed over the prefill, Enter; waits for the result.
    fn find(viewer: &Entity<ViewerView>, text: &str, cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("ctrl-f");
        cx.run_until_parked();
        assert!(dialog_open(cx));
        cx.simulate_input(text);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        settle(viewer, cx);
    }

    /// (row on screen, marked text) of every marked row.
    fn marks(viewer: &Entity<ViewerView>, cx: &mut VisualTestContext) -> Vec<(usize, String)> {
        cx.run_until_parked();
        viewer.read_with(cx, |v, _| {
            v.shown_marks()
                .iter()
                .enumerate()
                .filter_map(|(ix, m)| m.clone().map(|m| (ix, m)))
                .collect()
        })
    }

    fn top_row(viewer: &Entity<ViewerView>, cx: &mut VisualTestContext) -> String {
        shown(viewer, cx)[0].clone()
    }

    #[gpui_kit::test]
    fn ctrl_f_finds_and_marks_the_match_a_third_down(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        find(&viewer, "line 500", &mut cx);
        assert!(!dialog_open(&mut cx));
        let screen = viewer.read_with(&cx, |v, _| v.screen_rows());
        assert_eq!(marks(&viewer, &mut cx), [(screen / 3, "line 500".into())]);
        // The view keeps keys: Escape still closes it.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.windows().is_empty());
    }

    #[gpui_kit::test]
    fn a_match_on_screen_leaves_the_view_alone(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        find(&viewer, "line 5", &mut cx);
        assert_eq!(top_row(&viewer, &mut cx), "line 1");
        assert_eq!(marks(&viewer, &mut cx), [(4, "line 5".into())]);
    }

    #[gpui_kit::test]
    fn f3_and_shift_f3_step_through_matches(cx: &mut TestAppContext) {
        let text: String = (0..300)
            .map(|n| {
                if n % 100 == 50 {
                    format!("needle {n}\n")
                } else {
                    format!("{n}\n")
                }
            })
            .collect();
        let (_tmp, viewer, mut cx) = view(text.as_bytes(), cx);
        let current = |cx: &mut VisualTestContext| {
            let m = viewer.read_with(cx, |v, _| v.current_match()).unwrap();
            text[m.start as usize..].lines().next().unwrap().to_owned()
        };
        find(&viewer, "needle", &mut cx);
        assert_eq!(current(&mut cx), "needle 50");
        cx.simulate_keystrokes("f3");
        settle(&viewer, &mut cx);
        assert_eq!(current(&mut cx), "needle 150");
        cx.simulate_keystrokes("f3");
        settle(&viewer, &mut cx);
        assert_eq!(current(&mut cx), "needle 250");
        cx.simulate_keystrokes("f3");
        settle(&viewer, &mut cx);
        assert!(dialog_open(&mut cx), "not found");
        assert_eq!(current(&mut cx), "needle 250", "the match stays");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1, "Escape closed only the box");
        cx.simulate_keystrokes("shift-f3");
        settle(&viewer, &mut cx);
        assert_eq!(current(&mut cx), "needle 150");
        // Off screen, F3 starts at the top of the screen.
        cx.simulate_keystrokes("ctrl-home f3");
        settle(&viewer, &mut cx);
        assert_eq!(current(&mut cx), "needle 50");
        // Enter is F3 too.
        cx.simulate_keystrokes("enter");
        settle(&viewer, &mut cx);
        assert_eq!(current(&mut cx), "needle 150");
    }

    #[gpui_kit::test]
    fn f3_without_a_search_opens_the_dialog(cx: &mut TestAppContext) {
        for key in ["f3", "shift-f3", "enter"] {
            let (_tmp, _viewer, mut vcx) = view(b"text\n", cx);
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            assert!(dialog_open(&mut vcx), "{key}");
            vcx.simulate_keystrokes("escape");
            vcx.run_until_parked();
            assert!(!dialog_open(&mut vcx));
            vcx.simulate_keystrokes("escape");
            vcx.run_until_parked();
        }
    }

    #[gpui_kit::test]
    fn empty_text_does_nothing_and_a_bad_regex_says_so(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"a (b)\n", cx);
        cx.simulate_keystrokes("ctrl-f enter");
        cx.run_until_parked();
        assert!(dialog_open(&mut cx), "empty: the dialog stays");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            cx.set_global(LastSearch(Text {
                pattern: "(".into(),
                regex: true,
                ..Text::default()
            }))
        });
        cx.simulate_keystrokes("ctrl-f enter");
        cx.run_until_parked();
        assert!(!viewer.read_with(&cx, |v, _| v.searching()));
        // The error box over the dialog; closing it goes back to the dialog.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(dialog_open(&mut cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!dialog_open(&mut cx));
        assert_eq!(viewer.read_with(&cx, |v, _| v.current_match()), None);
    }

    #[gpui_kit::test]
    fn the_dialog_starts_with_the_last_search_from_any_viewer(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut vcx) = view(b"one Two two\n", cx);
        vcx.simulate_keystrokes("ctrl-f");
        vcx.run_until_parked();
        vcx.simulate_input("two");
        // Tab to Case-sensitive, Space ticks it.
        vcx.simulate_keystrokes("tab space enter");
        vcx.run_until_parked();
        settle(&viewer, &mut vcx);
        assert_eq!(
            viewer.read_with(&vcx, |v, _| v.current_match()),
            Some(8..11)
        );
        // A second viewer's dialog: Enter searches the same way.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("other.txt");
        std::fs::write(&path, b"Two two\n").unwrap();
        cx.update(|cx| open(path, main_bounds(), None, None, cx))
            .unwrap();
        let window = *cx.windows().last().unwrap();
        let other = viewer_in(window, cx).unwrap();
        let mut ocx = VisualTestContext::from_window(window, cx);
        ocx.run_until_parked();
        ocx.simulate_keystrokes("ctrl-f enter");
        ocx.run_until_parked();
        settle(&other, &mut ocx);
        assert_eq!(other.read_with(&ocx, |v, _| v.current_match()), Some(4..7));
    }

    #[gpui_kit::test]
    fn escape_stops_a_long_search_and_the_status_line_says_so(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        viewer.update(&mut cx, |v, _| v.search.hold = true);
        cx.simulate_keystrokes("ctrl-f");
        cx.run_until_parked();
        cx.simulate_input("line 900");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let status = |cx: &mut VisualTestContext| viewer.update(cx, |v, _| v.status_text());
        assert!(!status(&mut cx).contains("searching"), "quiet at first");
        for _ in 0..3 {
            cx.executor()
                .advance_clock(crate::file_manager::commands::OPENER_POLL);
            cx.run_until_parked();
        }
        assert!(
            status(&mut cx).contains("searching... 0%"),
            "{}",
            status(&mut cx)
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1, "Escape stopped the search only");
        assert!(!viewer.read_with(&cx, |v, _| v.searching()));
        viewer.update(&mut cx, |v, _| {
            v.search
                .held
                .take()
                .unwrap()
                .store(false, Ordering::Relaxed)
        });
        for _ in 0..5 {
            cx.executor()
                .advance_clock(crate::file_manager::commands::OPENER_POLL);
            cx.run_until_parked();
        }
        assert_eq!(viewer.read_with(&cx, |v, _| v.current_match()), None);
        assert_eq!(top_row(&viewer, &mut cx), "line 1");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(cx.windows().is_empty());
    }

    #[gpui_kit::test]
    fn without_wrap_the_columns_follow_the_match(cx: &mut TestAppContext) {
        let line = format!("{}needle\n", "x".repeat(500));
        let (_tmp, viewer, mut cx) = view(line.as_bytes(), cx);
        cx.simulate_keystrokes("w");
        find(&viewer, "needle", &mut cx);
        assert!(viewer.read_with(&cx, |v, _| v.h_offset()) > 0);
        assert_eq!(marks(&viewer, &mut cx), [(0, "needle".into())]);
    }

    #[gpui_kit::test]
    fn a_wrapped_match_is_marked_on_each_row(cx: &mut TestAppContext) {
        let (tmp, first, vcx) = view(b"y", cx);
        let cols = first.read_with(&vcx, |v, _| v.cols) as usize;
        let path = tmp.path().join("wide.txt");
        std::fs::write(&path, format!("{}abcdef\n", "x".repeat(cols - 3))).unwrap();
        cx.update(|cx| open(path, main_bounds(), None, None, cx))
            .unwrap();
        let window = *cx.windows().last().unwrap();
        let viewer = viewer_in(window, cx).unwrap();
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.run_until_parked();
        find(&viewer, "abcdef", &mut cx);
        assert_eq!(
            marks(&viewer, &mut cx),
            [(0, "abc".into()), (1, "def".into())]
        );
    }

    #[gpui_kit::test]
    fn alt_f7_text_opens_at_the_first_match(cx: &mut TestAppContext) {
        setup(cx);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, numbered(1000)).unwrap();
        let text = Text {
            pattern: "LINE 700".into(),
            ..Text::default()
        };
        cx.update(|cx| open(path, main_bounds(), None, Some(text), cx))
            .unwrap();
        let window = *cx.windows().last().unwrap();
        let viewer = viewer_in(window, cx).unwrap();
        let mut vcx = VisualTestContext::from_window(window, cx);
        settle(&viewer, &mut vcx);
        let screen = viewer.read_with(&vcx, |v, _| v.screen_rows());
        assert_eq!(marks(&viewer, &mut vcx), [(screen / 3, "line 700".into())]);
        let last = cx.update(|cx| cx.global::<LastSearch>().0.pattern.clone());
        assert_eq!(last, "LINE 700");
    }
}
