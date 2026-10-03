//! Mouse selection and copy in the viewer. The selection is a byte range of
//! the file (`anchor`, `head`), so it survives scrolling, `W`, `H` and
//! resizing; the rows last drawn turn mouse positions into bytes.

use std::ops::Range;

use gpui_kit::{
    Bounds, ClipboardItem, Context, DispatchPhase, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Styled, Window, canvas, point, px, size,
};
use yagni_commander_core::format_size;
use yagni_commander_core::viewer::{COPY_CAP, Copy, HexColumn, TooLarge};

use super::{LINE_HEIGHT, PADDING, SCROLLBAR_WIDTH, STATUS_HEIGHT, ViewerView};
use crate::file_manager::commands::show_error;

/// Auto-scroll tick while dragging past an edge.
pub(super) const TICK: std::time::Duration = std::time::Duration::from_millis(50);
/// Most rows one tick scrolls.
const MAX_ROWS_PER_TICK: usize = 20;

#[derive(Clone, Debug)]
pub(super) struct Selection {
    pub anchor: u64,
    pub head: u64,
    /// Hex: the column a drag started in (Ctrl-C copies that one). `None`
    /// for text-mode drags and search matches: copied as text, extended in
    /// the characters.
    pub column: Option<HexColumn>,
}

impl Selection {
    pub fn new(range: Range<u64>, column: Option<HexColumn>) -> Self {
        Self {
            anchor: range.start,
            head: range.end,
            column,
        }
    }

    pub fn range(&self) -> Range<u64> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Run {
    Dim,
    Selected,
    Marked,
}

/// Non-overlapping, ordered highlight runs over a row's text: a mark wins
/// over the selection, the selection over a dim placeholder. Each list is
/// sorted and non-overlapping, so one cursor per list keeps this linear.
pub(super) fn highlight_runs(
    dims: &[Range<usize>],
    selection: &[Range<usize>],
    marks: &[Range<usize>],
) -> Vec<(Range<usize>, Run)> {
    let mut edges: Vec<usize> = dims
        .iter()
        .chain(selection)
        .chain(marks)
        .flat_map(|r| [r.start, r.end])
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let lists = [
        (marks, Run::Marked),
        (selection, Run::Selected),
        (dims, Run::Dim),
    ];
    let mut cursors = [0; 3];
    let mut runs: Vec<(Range<usize>, Run)> = Vec::new();
    for pair in edges.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let mut found = None;
        for (k, (ranges, run)) in lists.iter().enumerate() {
            while ranges.get(cursors[k]).is_some_and(|r| r.end <= from) {
                cursors[k] += 1;
            }
            if found.is_none() && ranges.get(cursors[k]).is_some_and(|r| r.start <= from) {
                found = Some(*run);
            }
        }
        let Some(run) = found else {
            continue;
        };
        match runs.last_mut() {
            Some((r, last)) if *last == run && r.end == from => r.end = to,
            _ => runs.push((from..to, run)),
        }
    }
    runs
}

/// Window-wide mouse listeners, so a selection drag follows the mouse and
/// sees the release outside the window too (an element's own listeners only
/// fire while the mouse is over it).
pub(super) fn follow_drag(cx: &mut Context<ViewerView>) -> impl IntoElement {
    let this = cx.weak_entity();
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let moved = this.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = moved.update(cx, |v, cx| v.on_select_move(event, window, cx));
                }
            });
            let released = this.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble {
                    let _ = released.update(cx, |v, _| v.end_select());
                }
            });
        },
    )
    .absolute()
    .size_full()
}

impl ViewerView {
    /// The rows' area in window coordinates (the layout is fixed, as for
    /// the scrollbar).
    pub(super) fn text_area(window: &Window) -> Bounds<Pixels> {
        let v = window.viewport_size();
        Bounds::new(
            point(px(PADDING), px(PADDING)),
            size(
                v.width - px(2.0 * PADDING + SCROLLBAR_WIDTH),
                v.height - px(2.0 * PADDING + STATUS_HEIGHT),
            ),
        )
    }

    /// The drawn row under `p` (clamped to the rows drawn) and the column
    /// (fractional, `h_offset` added). `None` without rows. Below the last
    /// row: that row and a column past its end.
    fn hit(&self, p: Point<Pixels>, window: &Window) -> Option<(usize, f32)> {
        let area = Self::text_area(window);
        let last = self.drawn.len().checked_sub(1)?;
        let y = f32::from(p.y - area.origin.y);
        let mut x = f32::from(p.x - area.origin.x) / self.char_width;
        if !self.wraps_rows() {
            x += self.h_offset as f32;
        }
        if y >= (last + 1) as f32 * LINE_HEIGHT {
            return Some((last, f32::MAX));
        }
        let row = ((y / LINE_HEIGHT).floor().max(0.0) as usize).min(last);
        Some((row, x))
    }

    pub(super) fn on_text_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((row, x)) = self.hit(event.position, window) else {
            return;
        };
        let column = if self.hex {
            self.drawn[row].hex_column(x)
        } else {
            HexColumn::Codes
        };
        let hex_column = self.hex.then_some(column);
        self.last_mouse = event.position;
        if event.click_count <= 1 {
            let at = self.drawn[row].boundary_at(x, column);
            match self.selection.as_mut().filter(|_| event.modifiers.shift) {
                Some(sel) => sel.head = at,
                None => self.selection = Some(Selection::new(at..at, hex_column)),
            }
            self.selecting = true;
        } else {
            let at = self.drawn[row].char_at(x, column);
            let range = match (event.click_count, self.hex) {
                (2, false) => self.doc.word_at(at),
                (_, false) => self.doc.line_at(at),
                (2, true) => at..(at + 1).min(self.doc.len()),
                (_, true) => self.drawn[row].start..self.drawn[row].end,
            };
            self.selection = Some(Selection::new(range, hex_column));
            self.selecting = false;
        }
        cx.notify();
    }

    /// A mouse move anywhere: while selecting, the head follows and past an
    /// edge the view scrolls.
    fn on_select_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false; // released where we didn't see it
            return;
        }
        self.last_mouse = event.position;
        self.follow_mouse(window, cx);
        if self.overflow(window) != (0.0, 0.0) && !self.autoscrolling {
            self.autoscrolling = true;
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    let going = this
                        .update_in(cx, |this, window, cx| this.autoscroll(window, cx))
                        .unwrap_or(false);
                    if !going {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    fn end_select(&mut self) {
        self.selecting = false;
    }

    /// Moves the head to the byte under the last mouse position, clamped
    /// into the text area. In hex, a selection without a column (a search
    /// match, or one made in text mode) copies as text, so it follows the
    /// characters.
    fn follow_mouse(&mut self, window: &Window, cx: &mut Context<Self>) {
        let column = self
            .selection
            .as_ref()
            .and_then(|s| s.column)
            .unwrap_or(HexColumn::Chars);
        let area = Self::text_area(window);
        let p = self.last_mouse;
        let clamped = point(
            p.x.clamp(area.origin.x, area.origin.x + area.size.width),
            p.y.max(area.origin.y),
        );
        let Some((row, x)) = self.hit(clamped, window) else {
            return;
        };
        let at = self.drawn[row].boundary_at(x, column);
        if let Some(sel) = self.selection.as_mut() {
            sel.head = at;
            cx.notify();
        }
    }

    /// How far the mouse is past the text area: (x, y) in pixels, negative
    /// for left and up. Sideways only when rows scroll sideways.
    fn overflow(&self, window: &Window) -> (f32, f32) {
        let area = Self::text_area(window);
        let p = self.last_mouse;
        let over = |v: Pixels, lo: Pixels, hi: Pixels| {
            if v < lo {
                f32::from(v - lo)
            } else if v > hi {
                f32::from(v - hi)
            } else {
                0.0
            }
        };
        let x = if self.wraps_rows() {
            0.0
        } else {
            over(p.x, area.origin.x, area.origin.x + area.size.width)
        };
        (
            x,
            over(p.y, area.origin.y, area.origin.y + area.size.height),
        )
    }

    /// One auto-scroll tick. Returns whether to keep ticking.
    fn autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (x, y) = self.overflow(window);
        if !self.selecting || (x, y) == (0.0, 0.0) {
            self.autoscrolling = false;
            return false;
        }
        let rows = (1 + (y.abs() / LINE_HEIGHT) as usize).min(MAX_ROWS_PER_TICK);
        if y > 0.0 {
            self.down(rows, cx);
        } else if y < 0.0 {
            self.up(rows, cx);
        }
        if x != 0.0 {
            self.scroll_h(x > 0.0, cx);
        }
        // Lay out the new rows before hit testing them.
        self.drawn = self.layout_rows(self.screen_rows);
        self.follow_mouse(window, cx);
        true
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        let column = self.hex.then_some(HexColumn::Codes);
        self.selection = Some(Selection::new(0..self.doc.len(), column));
        cx.notify();
    }

    /// Tests: select `range` as if dragged in text.
    #[cfg(test)]
    pub(super) fn select_bytes(&mut self, range: Range<u64>) {
        self.selection = Some(Selection::new(range, None));
    }

    /// Ctrl-C: the selection to the clipboard; nothing without one.
    pub(super) fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.selection.as_ref().filter(|s| s.anchor != s.head) else {
            return;
        };
        let how = if self.hex && sel.column == Some(HexColumn::Codes) {
            Copy::Hex
        } else {
            Copy::Text
        };
        match self.doc.copy_text(sel.range(), how) {
            Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Err(TooLarge(size)) => {
                let message = format!(
                    "The selection is too large to copy ({}, at most {} MiB).",
                    format_size(size),
                    COPY_CAP >> 20
                );
                let back = Some(self.focus.clone());
                show_error("Cannot copy", message, back, window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_win_over_the_selection_and_the_selection_over_dims() {
        let one = |r: Range<usize>| std::iter::once(r).collect::<Vec<_>>();
        let runs = highlight_runs(&[0..1, 5..6], &one(0..8), &one(3..4));
        assert_eq!(
            runs,
            [
                (0..3, Run::Selected),
                (3..4, Run::Marked),
                (4..8, Run::Selected),
            ]
        );
        let runs = highlight_runs(&[0..1, 5..6], &[], &[]);
        assert_eq!(runs, [(0..1, Run::Dim), (5..6, Run::Dim)]);
    }

    #[test]
    fn highlight_runs_is_linear() {
        // A row of mostly binary data: 10,000 dim placeholders with a
        // character between each, nothing selected or marked.
        let dims: Vec<Range<usize>> = (0..10_000).map(|i| 3 * i..3 * i + 2).collect();
        let start = std::time::Instant::now();
        let runs = highlight_runs(&dims, &[], &[]);
        assert_eq!(runs.len(), 10_000);
        // Quadratic takes seconds here in a debug build; linear ~1 ms.
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[test]
    fn dims_outside_the_selection_stay_dim() {
        let dims: Vec<Range<usize>> = vec![0..2, 4..6, 8..10];
        let one = |r: Range<usize>| std::iter::once(r).collect::<Vec<_>>();
        assert_eq!(
            highlight_runs(&dims, &one(3..7), &[]),
            [(0..2, Run::Dim), (3..7, Run::Selected), (8..10, Run::Dim)]
        );
    }
}
