//! Ctrl-F / F3 / Shift-F3 in the viewer. The Find dialog (`FindView`) asks
//! for the text and options; the search runs on a `std::thread` of its own
//! with a second handle on the file (a read can hang on a dead mount) and
//! reports over a std channel polled every `OPENER_POLL` (a futures wake-up
//! from another thread panics gpui's test scheduler).

use std::io;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::InputState;
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, Global, IntoElement, ParentElement,
    Render, Styled, Window, div, px,
};
use yagni_commander_core::find::text::{Text, TextQuery};
use yagni_commander_core::viewer::{Direction, find};

use super::ViewerView;
use crate::button_row::{ButtonRow, OnPress};
use crate::file_manager::commands::{OPENER_POLL, dialog_field, show_error, show_message};

/// Polls before the status line says "searching...": ~300 ms.
const QUIET_POLLS: u32 = 3;

/// The last search asked for in any viewer (or handed over by Alt-F7): what
/// the Find dialog starts with. For the session only.
#[derive(Default)]
pub struct LastSearch(pub Text);

impl Global for LastSearch {}

/// A search on its thread.
pub(super) struct Running {
    id: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    rx: Receiver<io::Result<Option<Range<u64>>>>,
    polls: u32,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// This window's search.
#[derive(Default)]
pub(super) struct SearchState {
    /// What Ctrl-F asked for, and compiled.
    query: Option<(Text, TextQuery)>,
    /// The match last found.
    pub(super) current: Option<Range<u64>>,
    pub(super) running: Option<Running>,
    next_id: u64,
    /// Tests: the next search waits until this is cleared.
    #[cfg(test)]
    pub(super) hold: bool,
    #[cfg(test)]
    pub(super) held: Option<Arc<AtomicBool>>,
}

impl ViewerView {
    /// Ctrl-F: the Find dialog, with the last search.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let last = cx
            .try_global::<LastSearch>()
            .map(|l| l.0.clone())
            .unwrap_or_default();
        let view = cx.new(|cx| FindView::new(&last, window, cx));
        let this = cx.entity().downgrade();
        // Whether the dialog may close.
        let submit = Rc::new({
            let view = view.clone();
            move |window: &mut Window, cx: &mut App| {
                let text = view.read(cx).text(cx);
                let field = view.read(cx).input.focus_handle(cx);
                this.update(cx, |this, cx| this.submit(text, field, window, cx))
                    .unwrap_or(true)
            }
        });
        let focus = self.focus.clone();
        let find: OnPress = Rc::new({
            let (submit, focus) = (submit.clone(), focus.clone());
            move |window, cx| {
                if submit(window, cx) {
                    window.close_dialog(cx);
                    focus.focus(window, cx);
                }
            }
        });
        let cancel: OnPress = Rc::new({
            let focus = focus.clone();
            move |window, cx| {
                window.close_dialog(cx);
                focus.focus(window, cx);
            }
        });
        let buttons = ButtonRow::build([("Find", find), ("Cancel", cancel)], 0, cx);
        let input = view.read(cx).input.clone();
        window.defer(cx, move |window, cx| {
            input.update(cx, |state, cx| state.focus(window, cx))
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let submit = submit.clone();
            let (focus_ok, focus_cancel) = (focus.clone(), focus.clone());
            dialog
                .title("Find")
                .w(px(520.0))
                .close_button(false)
                .child(view.clone())
                .footer(buttons.clone())
                // Enter in the text field.
                .on_ok(move |_, window, cx| {
                    let done = submit(window, cx);
                    if done {
                        focus_ok.focus(window, cx);
                    }
                    done
                })
                .on_cancel(move |_, window, cx| {
                    focus_cancel.focus(window, cx);
                    true
                })
        });
    }

    /// The dialog's Find: compiles `text` and searches forward from the top
    /// of the screen. Returns whether the dialog may close (not on empty
    /// text or an invalid regular expression).
    fn submit(
        &mut self,
        text: Text,
        field: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if text.pattern.is_empty() {
            return false;
        }
        match text.compile() {
            Ok(query) => {
                cx.set_global(LastSearch(text.clone()));
                self.search.query = Some((text, query));
                self.search.current = None;
                self.start_search(Direction::Forward, window, cx);
                true
            }
            Err(e) => {
                let message = format!("Invalid regular expression: {e}");
                show_error("Cannot search", message, Some(field), window, cx);
                false
            }
        }
    }

    /// Alt-F7's search: from the start of the file. `text` was compiled
    /// before (it found this file), so a failure here just means no search.
    pub(super) fn search_from_start(
        &mut self,
        text: Text,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(query) = text.compile() else {
            return;
        };
        cx.set_global(LastSearch(text.clone()));
        self.search.query = Some((text, query));
        self.search.current = None;
        self.begin(0, Direction::Forward, window, cx);
    }

    /// F3 / Shift-F3: the next or previous match; without a search, the
    /// dialog.
    pub(super) fn find_again(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.query.is_none() {
            self.open_find(window, cx);
        } else {
            self.start_search(direction, window, cx);
        }
    }

    /// Where the next search starts: around the current match while it is
    /// on screen, else at the screen's edge.
    fn search_from(&self, direction: Direction) -> u64 {
        let on_screen = self
            .search
            .current
            .as_ref()
            .filter(|m| m.start >= self.top && m.start < self.shown_end.max(self.top + 1));
        match (direction, on_screen) {
            (Direction::Forward, Some(m)) => m.start + 1,
            (Direction::Backward, Some(m)) => m.start,
            (Direction::Forward, None) => self.top,
            (Direction::Backward, None) => self.shown_end,
        }
    }

    fn start_search(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.search_from(direction);
        self.begin(from, direction, window, cx);
    }

    /// Starts a search from `from` on a thread, replacing a running one.
    fn begin(
        &mut self,
        from: u64,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, query)) = &self.search.query else {
            return;
        };
        let source = match self.doc.source().try_clone() {
            Ok(source) => source,
            Err(e) => {
                let back = Some(self.focus.clone());
                show_error("Cannot search", e.to_string(), back, window, cx);
                return;
            }
        };
        let query = query.clone();
        self.search.next_id += 1;
        let id = self.search.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(from));
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(test)]
        let hold = std::mem::take(&mut self.search.hold).then(|| Arc::new(AtomicBool::new(true)));
        #[cfg(test)]
        {
            self.search.held = hold.clone();
        }
        {
            let (cancel, progress) = (cancel.clone(), progress.clone());
            std::thread::spawn(move || {
                #[cfg(test)]
                while hold.as_ref().is_some_and(|h| h.load(Ordering::Relaxed)) {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                let _ = tx.send(find(&source, &query, from, direction, &cancel, &progress));
            });
        }
        self.search.running = Some(Running {
            id,
            cancel,
            progress,
            rx,
            polls: 0,
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                let going = this
                    .update_in(cx, |this, window, cx| this.poll(id, window, cx))
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes in what search `id` found. Returns whether it still runs.
    fn poll(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.search.running.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        let result = match running.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                running.polls += 1;
                if running.polls >= QUIET_POLLS {
                    cx.notify();
                }
                return true;
            }
            // The thread died (it can't, short of a panic).
            Err(TryRecvError::Disconnected) => Err(io::ErrorKind::Interrupted.into()),
        };
        self.search.running = None;
        cx.notify();
        let back = Some(self.focus.clone());
        match result {
            Ok(Some(found)) => {
                self.search.current = Some(found.clone());
                self.reveal(found);
            }
            Ok(None) => {
                let pattern = self
                    .search
                    .query
                    .as_ref()
                    .map_or_else(String::new, |(t, _)| t.pattern.clone());
                let message = format!("\"{pattern}\" not found.");
                show_message("Find", message, "OK", back, window, cx);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => show_error("Cannot search", e.to_string(), back, window, cx),
        }
        false
    }

    /// Escape during a search: stops it; the view stays.
    pub(super) fn stop_search(&mut self, cx: &mut Context<Self>) -> bool {
        if self.search.running.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// "searching... 37%" once a search has run for a while.
    pub(super) fn searching_text(&self) -> Option<String> {
        let running = self.search.running.as_ref()?;
        if running.polls < QUIET_POLLS {
            return None;
        }
        let pos = running.progress.load(Ordering::Relaxed);
        let percent = (pos * 100)
            .checked_div(self.doc.len())
            .unwrap_or(100)
            .min(100);
        Some(format!("searching... {percent}%"))
    }

    /// Shows `found`: the view stays if it is entirely on screen, else its
    /// row goes a third down the screen. Without wrapping, the columns
    /// follow its start.
    fn reveal(&mut self, found: Range<u64>) {
        let wrap = self.layout();
        let on_screen = found.start >= self.top && found.end <= self.shown_end;
        let row_start = self.doc.row_start_at(found.start, wrap);
        if !on_screen {
            let top = self.doc.scroll_up(row_start, self.screen_rows / 3, wrap);
            let last = self.doc.last_top(self.screen_rows, wrap);
            self.top = top.min(last);
        }
        if !self.wraps_rows() {
            let row = self.doc.marked_rows(row_start, 1, wrap, Some(&found));
            if let Some(cols) = row.first().and_then(|r| r.mark_columns()) {
                let shown = self.h_offset..self.h_offset + self.cols;
                if !shown.contains(&cols.start) {
                    let start = cols.start.saturating_sub(self.cols / 3);
                    self.h_offset = start / super::H_STEP * super::H_STEP;
                }
            }
        }
    }
}

/// The Find dialog's body: the text and three option boxes.
pub struct FindView {
    input: Entity<InputState>,
    /// Case-sensitive, regular expression, whole words.
    options: [bool; 3],
    option_focus: [FocusHandle; 3],
}

const OPTIONS: [(&str, &str); 3] = [
    ("viewer-find-case", "Case-sensitive"),
    ("viewer-find-regex", "Regular expression"),
    ("viewer-find-words", "Whole words"),
];

impl FindView {
    fn new(last: &Text, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pattern = last.pattern.clone();
        let len = pattern.len();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(pattern));
        input.update(cx, |state, cx| state.set_selected_range(0..len, cx));
        Self {
            input,
            options: [last.case_sensitive, last.regex, last.whole_words],
            option_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
        }
    }

    fn text(&self, cx: &App) -> Text {
        Text {
            pattern: self.input.read(cx).value().to_string(),
            case_sensitive: self.options[0],
            regex: self.options[1],
            whole_words: self.options[2],
            not_containing: false,
        }
    }
}

impl Render for FindView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let boxes = (0..OPTIONS.len()).map(|ix| {
            let (id, label) = OPTIONS[ix];
            let this = cx.entity().downgrade();
            let toggle = move |_: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |this, cx| {
                    this.options[ix] = !this.options[ix];
                    cx.notify();
                });
            };
            crate::option_box::option_box(
                id,
                label,
                self.options[ix],
                &self.option_focus[ix],
                toggle,
                window,
                cx,
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(dialog_field(&self.input))
            .child(div().flex().gap(px(16.0)).pb(px(4.0)).children(boxes))
    }
}
