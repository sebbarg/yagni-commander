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
    cx.update(|cx| open(path, main_bounds(), cx)).unwrap();
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
        open(path.clone(), main_bounds(), cx).unwrap();
        open(path, main_bounds(), cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 3);
    main.update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(cx.windows().is_empty());
}
