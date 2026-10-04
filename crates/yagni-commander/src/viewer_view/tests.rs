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

    #[gpui_kit::test]
    fn a_found_match_becomes_the_selection(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        find(&viewer, "line 500", &mut cx);
        let m = viewer.read_with(&cx, |v, _| v.current_match()).unwrap();
        assert_eq!(viewer.read_with(&cx, |v, _| v.selection_range()), Some(m));
        cx.simulate_keystrokes("ctrl-c");
        let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some("line 500"));
    }

    fn prefill(
        viewer: &Entity<ViewerView>,
        range: std::ops::Range<u64>,
        cx: &mut VisualTestContext,
    ) -> Option<String> {
        viewer.update(cx, |v, _| {
            v.select_bytes(range);
            v.find_prefill()
        })
    }

    #[gpui_kit::test]
    fn ctrl_f_starts_with_a_short_one_line_selection(cx: &mut TestAppContext) {
        let mut text = b"hello world\nsecond\n".to_vec();
        text.extend(vec![b'x'; 300]);
        text.extend(b"\xff\xfe\n");
        let (_tmp, viewer, mut cx) = view(&text, cx);
        assert_eq!(prefill(&viewer, 6..11, &mut cx).as_deref(), Some("world"));
        assert_eq!(prefill(&viewer, 6..15, &mut cx), None, "spans a line break");
        assert_eq!(prefill(&viewer, 19..319, &mut cx), None, "over 256 bytes");
        assert_eq!(prefill(&viewer, 319..321, &mut cx), None, "invalid UTF-8");
        assert_eq!(prefill(&viewer, 3..3, &mut cx), None, "empty");
    }

    #[gpui_kit::test]
    fn after_a_search_ctrl_f_keeps_the_typed_pattern(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        // Case-insensitive: the match reads "line 500", the pattern doesn't.
        find(&viewer, "LINE 500", &mut cx);
        assert!(viewer.read_with(&cx, |v, _| v.selection_range()).is_some());
        assert_eq!(viewer.update(&mut cx, |v, _| v.find_prefill()), None);
    }

    #[gpui_kit::test]
    fn a_hex_match_extends_in_the_characters(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"0123456789abcdef", cx);
        cx.simulate_keystrokes("h");
        find(&viewer, "45", &mut cx);
        assert_eq!(
            viewer.read_with(&cx, |v, _| v.selection_range()),
            Some(4..6)
        );
        // Shift-press in the characters (they start at column 60), drag on
        // in them: the head follows the characters, not the codes.
        let row = cx.debug_bounds("viewer-row-0").unwrap();
        let w = viewer.read_with(&cx, |v, _| v.char_width());
        let col = |c: f32| point(row.origin.x + px(c * w), row.center().y);
        let shift = gpui_kit::Modifiers {
            shift: true,
            ..Default::default()
        };
        let left = gpui_kit::MouseButton::Left;
        cx.simulate_mouse_down(col(70.2), left, shift);
        cx.simulate_mouse_move(col(72.2), left, shift);
        cx.simulate_mouse_up(col(72.2), left, shift);
        cx.run_until_parked();
        assert_eq!(
            viewer.read_with(&cx, |v, _| v.selection_range()),
            Some(4..12)
        );
    }

    #[gpui_kit::test]
    fn the_prefill_is_what_ctrl_f_searches(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"one two\ntwo three\n", cx);
        cx.update(|_, cx| {
            cx.set_global(LastSearch(Text {
                pattern: "three".into(),
                ..Default::default()
            }))
        });
        viewer.update(&mut cx, |v, _| v.select_bytes(4..7));
        cx.simulate_keystrokes("ctrl-f");
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        settle(&viewer, &mut cx);
        assert_eq!(viewer.read_with(&cx, |v, _| v.current_match()), Some(4..7));
    }

    /// (row on screen, marked text) of every marked row.
    fn marks(viewer: &Entity<ViewerView>, cx: &mut VisualTestContext) -> Vec<(usize, String)> {
        cx.run_until_parked();
        viewer.read_with(cx, |v, _| {
            v.shown_marks()
                .iter()
                .enumerate()
                .flat_map(|(ix, m)| m.iter().map(move |m| (ix, m.clone())))
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

mod hex {
    use super::*;

    #[gpui_kit::test]
    fn h_toggles_hex_and_keeps_the_top_byte(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        cx.simulate_keystrokes("down down down down down"); // "line 6" at 35
        let top = viewer.read_with(&cx, |v, _| v.top());
        assert_eq!(top, 35);
        cx.simulate_keystrokes("h");
        assert!(viewer.read_with(&cx, |v, _| v.hex()));
        assert_eq!(viewer.read_with(&cx, |v, _| v.top()), 32);
        let rows = shown(&viewer, &mut cx);
        assert!(
            rows[0].starts_with("00000020  20 35 0A 6C 69 6E 65 20  36 0A "),
            "{}",
            rows[0]
        );
        assert!(rows[0].ends_with("   5.line 6.line 7"), "{}", rows[0]);
        cx.simulate_keystrokes("down");
        assert_eq!(viewer.read_with(&cx, |v, _| v.top()), 48);
        // Back to text: the line holding byte 48 ("line 7" starts at 42).
        cx.simulate_keystrokes("h");
        assert_eq!(viewer.read_with(&cx, |v, _| v.top()), 42);
        assert_eq!(shown(&viewer, &mut cx)[0], "line 7");
    }

    #[gpui_kit::test]
    fn w_does_nothing_in_hex_and_the_wrap_mode_comes_back(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(100), cx);
        cx.simulate_keystrokes("w h w");
        assert!(viewer.read_with(&cx, |v, _| v.hex() && !v.wraps()));
        cx.simulate_keystrokes("h");
        assert!(
            viewer.read_with(&cx, |v, _| !v.hex() && !v.wraps()),
            "still no wrap"
        );
    }

    #[gpui_kit::test]
    fn the_status_line_shows_the_offset(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(100), cx);
        cx.simulate_keystrokes("h down down");
        cx.run_until_parked();
        let status = viewer.update(&mut cx, |v, _| v.status_text());
        assert!(status.ends_with(" · hex · offset 20"), "{status}");
    }

    #[gpui_kit::test]
    fn ends_and_pages_work_in_hex(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx); // 8,893 bytes
        cx.simulate_keystrokes("h end");
        let rows = shown(&viewer, &mut cx);
        assert!(rows.last().unwrap().starts_with("000022B0  "), "{rows:?}");
        cx.simulate_keystrokes("home pagedown");
        let screen = viewer.read_with(&cx, |v, _| v.screen_rows()) as u64;
        assert_eq!(viewer.read_with(&cx, |v, _| v.top()), (screen - 1) * 16);
    }

    #[gpui_kit::test]
    fn left_and_right_scroll_a_narrow_hex_view(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(100), cx);
        cx.simulate_resize(gpui_kit::size(px(300.0), px(400.0)));
        cx.simulate_keystrokes("h");
        cx.run_until_parked();
        cx.simulate_keystrokes("right");
        assert_eq!(viewer.read_with(&cx, |v, _| v.h_offset()), 8);
        // Past the offset's 8 digits.
        assert!(shown(&viewer, &mut cx)[0].starts_with("  6C 69 6E "));
    }

    #[gpui_kit::test]
    fn a_match_shows_in_both_columns(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"abc needle xyz\n", cx);
        cx.simulate_keystrokes("h ctrl-f");
        cx.run_until_parked();
        cx.simulate_input("needle");
        cx.simulate_keystrokes("enter");
        for _ in 0..50 {
            cx.executor()
                .advance_clock(crate::file_manager::commands::OPENER_POLL);
            cx.run_until_parked();
            if !viewer.read_with(&cx, |v, _| v.searching()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let marks = viewer.read_with(&cx, |v, _| v.shown_marks()[0].clone());
        // Across the gap after the 8th byte.
        assert_eq!(marks, ["6E 65 65 64  6C 65", "needle"]);
    }
}

mod select {
    use super::*;
    use gpui_kit::{Modifiers, MouseButton, Pixels, Point};

    /// The window point of column `col` (fractional) on screen row `row`.
    fn at(
        viewer: &Entity<ViewerView>,
        row: usize,
        col: f32,
        cx: &mut VisualTestContext,
    ) -> Point<Pixels> {
        let selector: &'static str = Box::leak(format!("viewer-row-{row}").into_boxed_str());
        let b = bounds_of(cx, selector);
        let w = viewer.read_with(cx, |v, _| v.char_width());
        point(b.origin.x + px(col * w), b.center().y)
    }

    fn drag(from: Point<Pixels>, to: Point<Pixels>, cx: &mut VisualTestContext) {
        let m = Modifiers::default();
        cx.simulate_mouse_down(from, MouseButton::Left, m);
        cx.simulate_mouse_move(to, MouseButton::Left, m);
        cx.simulate_mouse_up(to, MouseButton::Left, m);
        cx.run_until_parked();
    }

    fn clicks(p: Point<Pixels>, count: usize, shift: bool, cx: &mut VisualTestContext) {
        let modifiers = Modifiers {
            shift,
            ..Default::default()
        };
        cx.simulate_event(gpui_kit::MouseDownEvent {
            button: MouseButton::Left,
            position: p,
            modifiers,
            click_count: count,
            first_mouse: false,
        });
        cx.simulate_event(gpui_kit::MouseUpEvent {
            button: MouseButton::Left,
            position: p,
            modifiers,
            click_count: count,
        });
        cx.run_until_parked();
    }

    pub(super) fn selection(
        viewer: &Entity<ViewerView>,
        cx: &mut VisualTestContext,
    ) -> Option<std::ops::Range<u64>> {
        cx.run_until_parked();
        viewer.read_with(cx, |v, _| v.selection_range())
    }

    pub(super) fn clipboard(cx: &mut VisualTestContext) -> Option<String> {
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
    }

    #[gpui_kit::test]
    fn dragging_selects_and_ctrl_c_copies(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"line 1\nline 2\nline 3\n", cx);
        let (from, to) = (at(&viewer, 0, 5.2, &mut cx), at(&viewer, 1, 4.8, &mut cx));
        drag(from, to, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(5..12));
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard(&mut cx).as_deref(), Some("1\nline "));
        let shown = viewer.read_with(&cx, |v, _| v.shown_selection().to_vec());
        assert_eq!(shown[0], ["1"]);
        assert_eq!(shown[1], ["line "]);
        assert!(shown[2].is_empty());
    }

    #[gpui_kit::test]
    fn ctrl_insert_copies_too(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"abc def\n", cx);
        let (from, to) = (at(&viewer, 0, 0.1, &mut cx), at(&viewer, 0, 2.9, &mut cx));
        drag(from, to, &mut cx);
        cx.simulate_keystrokes("ctrl-insert");
        assert_eq!(clipboard(&mut cx).as_deref(), Some("abc"));
    }

    #[gpui_kit::test]
    fn shift_click_extends_from_the_anchor(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"0123456789\n", cx);
        let (from, to) = (at(&viewer, 0, 2.1, &mut cx), at(&viewer, 0, 4.1, &mut cx));
        drag(from, to, &mut cx);
        let p = at(&viewer, 0, 8.1, &mut cx);
        clicks(p, 1, true, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(2..8));
        let p = at(&viewer, 0, 0.1, &mut cx);
        clicks(p, 1, true, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(0..2));
    }

    #[gpui_kit::test]
    fn double_click_selects_a_word_and_triple_click_a_line(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"say hello_world now\nnext\n", cx);
        let p = at(&viewer, 0, 6.5, &mut cx);
        clicks(p, 2, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(4..15));
        clicks(p, 3, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(0..20));
    }

    #[gpui_kit::test]
    fn double_click_past_the_last_line_selects_its_last_word(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"first\nsay hello", cx);
        let p = at(&viewer, 1, 30.0, &mut cx);
        clicks(p, 2, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(10..15));
        clicks(p, 3, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(6..15));
    }

    #[gpui_kit::test]
    fn a_click_clears_the_selection(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"abc def\n", cx);
        let (from, to) = (at(&viewer, 0, 0.1, &mut cx), at(&viewer, 0, 2.9, &mut cx));
        drag(from, to, &mut cx);
        let p = at(&viewer, 0, 5.0, &mut cx);
        clicks(p, 1, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), None);
    }

    #[gpui_kit::test]
    fn ctrl_c_without_a_selection_copies_nothing(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"abc\n", cx);
        cx.update(|_, cx| {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("before".into()))
        });
        let p = at(&viewer, 0, 1.0, &mut cx);
        clicks(p, 1, false, &mut cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard(&mut cx).as_deref(), Some("before"));
    }

    #[gpui_kit::test]
    fn ctrl_a_selects_the_whole_file(cx: &mut TestAppContext) {
        let text = numbered(1000);
        let (_tmp, viewer, mut cx) = view(&text, cx);
        cx.simulate_keystrokes("ctrl-a");
        assert_eq!(selection(&viewer, &mut cx), Some(0..text.len() as u64));
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard(&mut cx).unwrap().as_bytes(), &text[..]);
    }

    #[gpui_kit::test]
    fn a_selection_over_the_cap_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"abc\n", cx);
        let cap = yagni_commander_core::viewer::COPY_CAP;
        viewer.update(&mut cx, |v, _| v.select_bytes(0..cap + 1));
        cx.simulate_keystrokes("ctrl-c");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| {
            use gpui_kit::component::WindowExt;
            window.has_active_dialog(cx)
        }));
    }

    #[gpui_kit::test]
    fn the_selection_survives_w_h_and_scrolling(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let (from, to) = (at(&viewer, 1, 0.1, &mut cx), at(&viewer, 2, 2.1, &mut cx));
        drag(from, to, &mut cx);
        let before = selection(&viewer, &mut cx);
        assert!(before.is_some());
        cx.simulate_keystrokes("w h down down pagedown pageup h w");
        assert_eq!(selection(&viewer, &mut cx), before);
    }

    #[gpui_kit::test]
    fn hex_copies_the_column_the_drag_started_in(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"0123456789abcdefXYZ", cx);
        cx.simulate_keystrokes("h");
        // Byte i's code starts at column 10 + 3i (+1 from the 9th on).
        let code = |i: usize| (10 + i * 3 + usize::from(i >= 8)) as f32;
        let (from, to) = (
            at(&viewer, 0, code(1) + 0.2, &mut cx),
            at(&viewer, 0, code(3) + 1.8, &mut cx),
        );
        drag(from, to, &mut cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard(&mut cx).as_deref(), Some("31 32 33"));
        let marks = viewer.read_with(&cx, |v, _| v.shown_selection()[0].clone());
        assert_eq!(marks, ["31 32 33", "123"]);
        // The characters start at column 60.
        let (from, to) = (at(&viewer, 0, 60.2, &mut cx), at(&viewer, 1, 60.9, &mut cx));
        drag(from, to, &mut cx);
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(clipboard(&mut cx).as_deref(), Some("0123456789abcdefX"));
    }

    #[gpui_kit::test]
    fn the_scrollbar_does_not_touch_the_selection(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let (from, to) = (at(&viewer, 0, 0.1, &mut cx), at(&viewer, 0, 3.9, &mut cx));
        drag(from, to, &mut cx);
        let track = bounds_of(&mut cx, "viewer-scrollbar");
        clicks(track.center(), 1, false, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(0..4));
    }

    #[gpui_kit::test]
    fn a_release_outside_the_window_ends_the_drag(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"0123456789\n", cx);
        let m = Modifiers::default();
        let (a, b, c) = (
            at(&viewer, 0, 0.1, &mut cx),
            at(&viewer, 0, 3.1, &mut cx),
            at(&viewer, 0, 8.1, &mut cx),
        );
        cx.simulate_mouse_down(a, MouseButton::Left, m);
        cx.simulate_mouse_move(b, MouseButton::Left, m);
        // Released elsewhere: the next move has no button.
        cx.simulate_mouse_move(c, None::<MouseButton>, m);
        cx.simulate_mouse_move(a, MouseButton::Left, m);
        assert_eq!(selection(&viewer, &mut cx), Some(0..3));
    }

    #[gpui_kit::test]
    fn dragging_below_the_end_selects_the_last_byte(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"ab\ncd", cx);
        let p = at(&viewer, 1, 0.0, &mut cx);
        let below = point(p.x, p.y + px(5.0 * LINE_HEIGHT));
        let from = at(&viewer, 0, 0.1, &mut cx);
        drag(from, below, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(0..5));
    }

    fn tick(cx: &mut VisualTestContext) {
        cx.executor()
            .advance_clock(crate::viewer_view::select::TICK);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn dragging_past_the_bottom_scrolls_and_extends(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let m = Modifiers::default();
        let start = at(&viewer, 0, 0.1, &mut cx);
        let status = bounds_of(&mut cx, "viewer-status").center();
        cx.simulate_mouse_down(start, MouseButton::Left, m);
        cx.simulate_mouse_move(status, MouseButton::Left, m);
        let shown_end = selection(&viewer, &mut cx).unwrap().end;
        for _ in 0..5 {
            tick(&mut cx);
        }
        assert!(viewer.read_with(&cx, |v, _| v.top()) > 0, "scrolled");
        let sel = selection(&viewer, &mut cx).unwrap();
        assert_eq!(sel.start, 0);
        assert!(sel.end > shown_end, "{sel:?} past {shown_end}");
        cx.simulate_mouse_up(status, MouseButton::Left, m);
        let top = viewer.read_with(&cx, |v, _| v.top());
        tick(&mut cx);
        tick(&mut cx);
        assert_eq!(
            viewer.read_with(&cx, |v, _| v.top()),
            top,
            "stops on release"
        );
    }

    #[gpui_kit::test]
    fn dragging_past_the_right_edge_scrolls_sideways_without_wrap(cx: &mut TestAppContext) {
        let long = format!("{}\n", "0123456789".repeat(100));
        let (_tmp, viewer, mut cx) = view(long.as_bytes(), cx);
        cx.simulate_keystrokes("w");
        let m = Modifiers::default();
        let track = bounds_of(&mut cx, "viewer-scrollbar");
        let start = at(&viewer, 0, 0.1, &mut cx);
        cx.simulate_mouse_down(start, MouseButton::Left, m);
        let right = point(track.origin.x + px(1.0), start.y);
        cx.simulate_mouse_move(right, MouseButton::Left, m);
        tick(&mut cx);
        tick(&mut cx);
        assert!(viewer.read_with(&cx, |v, _| v.h_offset()) > 0);
        cx.simulate_mouse_up(right, MouseButton::Left, m);
    }

    #[gpui_kit::test]
    fn wrap_mode_never_scrolls_sideways(cx: &mut TestAppContext) {
        let long = format!("{}\n", "0123456789".repeat(100));
        let (_tmp, viewer, mut cx) = view(long.as_bytes(), cx);
        let m = Modifiers::default();
        let track = bounds_of(&mut cx, "viewer-scrollbar");
        let start = at(&viewer, 0, 0.1, &mut cx);
        cx.simulate_mouse_down(start, MouseButton::Left, m);
        let right = point(track.origin.x + px(1.0), start.y);
        cx.simulate_mouse_move(right, MouseButton::Left, m);
        tick(&mut cx);
        assert_eq!(viewer.read_with(&cx, |v, _| v.h_offset()), 0);
        cx.simulate_mouse_up(right, MouseButton::Left, m);
    }

    #[gpui_kit::test]
    fn a_release_below_the_window_stops_auto_scroll(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let m = Modifiers::default();
        let start = at(&viewer, 0, 0.1, &mut cx);
        let status = bounds_of(&mut cx, "viewer-status");
        cx.simulate_mouse_down(start, MouseButton::Left, m);
        cx.simulate_mouse_move(status.center(), MouseButton::Left, m);
        tick(&mut cx);
        let outside = point(status.center().x, status.bottom() + px(100.0));
        cx.simulate_mouse_up(outside, MouseButton::Left, m);
        cx.run_until_parked();
        let top = viewer.read_with(&cx, |v, _| v.top());
        let sel = selection(&viewer, &mut cx);
        tick(&mut cx);
        tick(&mut cx);
        assert_eq!(viewer.read_with(&cx, |v, _| v.top()), top, "stopped");
        assert_eq!(selection(&viewer, &mut cx), sel);
    }

    #[gpui_kit::test]
    fn further_below_the_window_scrolls_faster(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let m = Modifiers::default();
        let start = at(&viewer, 0, 0.1, &mut cx);
        let status = bounds_of(&mut cx, "viewer-status");
        cx.simulate_mouse_down(start, MouseButton::Left, m);
        let far = point(status.center().x, status.bottom() + px(10.0 * LINE_HEIGHT));
        cx.simulate_mouse_move(far, MouseButton::Left, m);
        tick(&mut cx);
        // "line N": the top row after one tick is well past line 3.
        let top_row = shown(&viewer, &mut cx)[0].clone();
        let n: usize = top_row["line ".len()..].parse().unwrap();
        assert!(n > 5, "one tick scrolled to {top_row}");
        cx.simulate_mouse_up(far, MouseButton::Left, m);
    }

    #[gpui_kit::test]
    fn a_drag_selects_under_the_mouse_when_zoomed(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(b"line 1\nline 2\nline 3\n", cx);
        for _ in 0..8 {
            cx.simulate_keystrokes("ctrl-=");
        }
        cx.run_until_parked();
        // `at` places the mouse by the drawn row and the measured character
        // width, so a hit test that ignores the zoom picks other bytes.
        let (from, to) = (at(&viewer, 0, 5.2, &mut cx), at(&viewer, 1, 4.8, &mut cx));
        drag(from, to, &mut cx);
        assert_eq!(selection(&viewer, &mut cx), Some(5..12));
    }
}

mod zoom {
    use super::*;

    #[gpui_kit::test]
    fn ctrl_equals_zooms_the_viewer_only(cx: &mut TestAppContext) {
        let (_tmp, viewer, mut cx) = view(&numbered(1000), cx);
        let rows = viewer.read_with(&cx, |v, _| v.screen_rows());
        for _ in 0..8 {
            cx.simulate_keystrokes("ctrl-=");
        }
        cx.run_until_parked();
        let zoomed = viewer.read_with(&cx, |v, _| v.screen_rows());
        assert!(zoomed < rows, "{zoomed} rows at 24 px, {rows} at 16 px");
        let zoom = cx.update(|_, cx| crate::zoom::Zoom::get(cx));
        assert_eq!((zoom.ui, zoom.viewer), (16.0, 24.0));
        let row = bounds_of(&mut cx, "viewer-row-0");
        assert_eq!(row.size.height, px(LINE_HEIGHT * 1.5), "rows drawn zoomed");
        cx.simulate_keystrokes("ctrl-- ctrl-0");
        cx.run_until_parked();
        assert_eq!(viewer.read_with(&cx, |v, _| v.screen_rows()), rows);
        assert_eq!(cx.update(|_, cx| crate::zoom::Zoom::get(cx).viewer), 16.0);
    }

    #[gpui_kit::test]
    fn keypad_plus_and_minus_zoom_the_viewer(cx: &mut TestAppContext) {
        let (_tmp, _viewer, mut cx) = view(&numbered(10), cx);
        cx.simulate_keystrokes("ctrl-add ctrl-add ctrl-subtract");
        cx.run_until_parked();
        let zoom = cx.update(|_, cx| crate::zoom::Zoom::get(cx));
        assert_eq!((zoom.ui, zoom.viewer), (16.0, 17.0));
    }

    #[gpui_kit::test]
    fn a_second_viewer_follows_the_level(cx: &mut TestAppContext) {
        let (tmp, first, mut cx) = view(&numbered(1000), cx);
        let rows = first.read_with(&cx, |v, _| v.screen_rows());
        cx.simulate_keystrokes("ctrl-= ctrl-= ctrl-= ctrl-=");
        cx.run_until_parked();
        let path = tmp.path().join("file.txt");
        cx.update(|_, cx| open(path, main_bounds(), None, None, cx))
            .unwrap();
        let window = *cx.windows().last().unwrap();
        let second = viewer_in(window, &mut cx).unwrap();
        let second_cx = VisualTestContext::from_window(window, &cx);
        second_cx.run_until_parked();
        let (a, b) = (
            first.read_with(&cx, |v, _| v.screen_rows()),
            second.read_with(&second_cx, |v, _| v.screen_rows()),
        );
        assert_eq!(a, b);
        assert!(a < rows, "zoomed: {a} rows, {rows} at 16 px");
    }

    #[gpui_kit::test]
    fn the_find_dialog_follows_the_ui_level(cx: &mut TestAppContext) {
        let (_tmp, _viewer, mut cx) = view(&numbered(10), cx);
        let find_dialog = |cx: &mut VisualTestContext| {
            cx.simulate_keystrokes("ctrl-f");
            let sizes = (
                bounds_of(cx, "dialog-0").size.width,
                bounds_of(cx, "prompt-field").size.height,
            );
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            sizes
        };
        let base = find_dialog(&mut cx);
        cx.simulate_keystrokes("ctrl-= ctrl-= ctrl-= ctrl-=");
        cx.run_until_parked();
        let size =
            cx.update(|_, cx| f32::from(gpui_kit::component::ActiveTheme::theme(cx).font_size));
        assert_eq!(size, 16.0, "dialogs use the UI level, not the viewer's");
        assert_eq!(
            find_dialog(&mut cx),
            base,
            "the viewer level leaves it alone"
        );
        // 20 px: 650 px wide still fits the 800 px window.
        cx.update(|_, cx| crate::zoom::change_ui(4.0, cx));
        cx.run_until_parked();
        let (width, field) = find_dialog(&mut cx);
        assert_eq!(width, base.0 * 1.25, "the UI level sizes it");
        assert_eq!(field, base.1 * 1.25);
    }
}

mod edit {
    use super::*;
    use gpui_kit::component::WindowExt;

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

    fn dialog_open(cx: &mut VisualTestContext) -> bool {
        cx.run_until_parked();
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// An editor command that writes the path it was given to `out`.
    fn recording_editor(out: &std::path::Path) -> String {
        format!("sh -c 'printf %s \"$0\" > \"{}\"'", out.display())
    }

    fn wait_for(out: &std::path::Path) -> String {
        for _ in 0..500 {
            if let Ok(text) = std::fs::read_to_string(out)
                && !text.is_empty()
            {
                return text;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the editor never ran");
    }

    #[gpui_kit::test]
    fn f4_opens_the_viewed_file_in_the_editor(cx: &mut TestAppContext) {
        let (tmp, viewer, mut cx) = view(b"text\n", cx);
        let out = tmp.path().join("out");
        set_editor(&recording_editor(&out), &mut cx);
        cx.simulate_keystrokes("f4");
        let path = tmp.path().join("file.txt");
        assert_eq!(wait_for(&out), path.display().to_string());
        assert!(!dialog_open(&mut cx));
        assert!(viewer.read_with(&cx, |_, _| true), "the viewer stays open");
    }

    #[gpui_kit::test]
    fn f4_without_an_editor_shows_an_error(cx: &mut TestAppContext) {
        let (_tmp, _viewer, mut cx) = view(b"text\n", cx);
        cx.simulate_keystrokes("f4");
        assert!(dialog_open(&mut cx));
    }

    #[gpui_kit::test]
    fn f4_on_an_archive_entry_is_refused(cx: &mut TestAppContext) {
        setup(cx);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("entry.txt");
        std::fs::write(&path, b"text\n").unwrap();
        let out = temp.path().join("out");
        cx.update(|cx| open(path, main_bounds(), Some(temp), None, cx))
            .unwrap();
        let window = *cx.windows().last().unwrap();
        let mut cx = VisualTestContext::from_window(window, cx);
        set_editor(&recording_editor(&out), &mut cx);
        cx.simulate_keystrokes("f4");
        assert!(dialog_open(&mut cx), "a box says why");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!out.exists(), "the private copy is never edited");
    }
}

#[gpui_kit::test]
fn viewer_windows_follow_a_theme_switch(cx: &mut TestAppContext) {
    let (_tmp, _viewer, mut cx) = view(&numbered(10), cx);
    cx.update(|_, cx| crate::theme::switch_theme(Theme::named(Some("classic")).0, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("viewer-status").is_some(), "redrawn");
    assert_eq!(cx.update(|_, cx| Theme::get(cx).name.clone()), "Classic");
}
