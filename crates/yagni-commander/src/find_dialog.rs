//! Alt-F7: the find dialog. Name masks, the folder, and an optional text,
//! with case options for each and the text's other options; a live list of results below. The search
//! runs on a `std::thread` of its own (a read can hang on a dead mount) and
//! sends its results over a std channel, polled every `OPENER_POLL`. The
//! view outlives the dialog, so Alt-F7 reopens it with the last search.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::component::input::InputState;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    App, AppContext, Context, CursorStyle, DispatchPhase, Div, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollStrategy, SharedString, Size,
    Stateful, Styled, UniformListScrollHandle, Window, anchored, canvas, deferred, div,
    prelude::FluentBuilder, px, size, uniform_list,
};
use yagni_commander_core::find::masks::Masks;
use yagni_commander_core::find::text::Text;
use yagni_commander_core::find::{
    DEFAULT_SKIP, Progress, Query, Results, SKIP_FOLDERS, SearchSummary, search,
};
use yagni_commander_core::{Entry, EntryKind, format_count, hotlist};

use crate::actions::{
    FIND_DIALOG_CONTEXT, FIND_RESULTS_CONTEXT, FIND_SKIP_BUTTON_CONTEXT, FIND_SKIP_CONTEXT,
    find_results as act,
};
use crate::app_state::AppState;
use crate::button_row::{ButtonRow, OnPress, focus_ring};
use crate::file_manager::commands::{OPENER_POLL, dialog_field, show_error, text_field};
use crate::theme::Theme;

/// Rows the results list moves by on PageUp/PageDown.
const PAGE_ROWS: usize = 10;
/// Height of the results list at the dialog's smallest.
const LIST_HEIGHT: f32 = 240.0;
/// Width of the dialog at its smallest.
pub const MIN_WIDTH: f32 = 680.0;
/// The option boxes, in their tab order; indices below.
const OPTIONS: [(&str, &str); 5] = [
    // Next to Search for.
    ("find-case-names", "Case-sensitive"),
    // Under Containing text.
    ("find-case", "Case-sensitive"),
    ("find-regex", "Regular expression"),
    ("find-words", "Whole words"),
    ("find-not", "Not containing"),
];
/// Height of the "Skip folders" control (a text field's).
const SKIP_CONTROL_HEIGHT: f32 = 30.0;
/// Width of a column of the text's option boxes.
const OPTION_COLUMN: f32 = 200.0;
const CASE_NAMES: usize = 0;
const CASE_TEXT: usize = 1;
const REGEX: usize = 2;
const WHOLE_WORDS: usize = 3;
const NOT_CONTAINING: usize = 4;

/// The dialog's text fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Masks,
    SearchIn,
    Text,
}

/// What the status line describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running { seen: u64, found: u64 },
    Done(SearchSummary),
}

/// What the dialog asks the file manager to do.
pub enum FindEvent {
    /// Show this result in the active panel; close the dialog.
    GoTo(PathBuf),
    /// F3 on a result, with the text searched for (to show its first
    /// match); the dialog stays.
    View(PathBuf, Option<Text>),
    /// Show the results in the active panel; close the dialog.
    Feed(Results),
    /// The Close button.
    Closed,
}

/// What the search thread sends.
enum Msg {
    Found(Entry),
    Done(SearchSummary),
}

struct Running {
    /// Tells a late timer tick that its search was stopped or replaced.
    id: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    rx: Receiver<Msg>,
}

pub struct FindDialog {
    masks: Entity<InputState>,
    search_in: Entity<InputState>,
    text: Entity<InputState>,
    /// The option boxes' states and focus handles, in `OPTIONS` order.
    options: [bool; 5],
    option_focus: [FocusHandle; 5],
    /// "Skip folders": which of `SKIP_FOLDERS` are on; the control's and
    /// its popup's focus, whether the popup is open, its cursor.
    skip: [bool; SKIP_FOLDERS.len()],
    skip_focus: FocusHandle,
    skip_popup_focus: FocusHandle,
    skip_open: bool,
    skip_cursor: usize,
    /// How much bigger than its smallest the user dragged the dialog.
    grown: Size<Pixels>,
    /// While the corner is dragged: where the mouse went down, and `grown`
    /// then.
    resizing: Option<(Point<Pixels>, Size<Pixels>)>,
    /// Named by their path relative to `root`; shared with a results
    /// panel after "Feed to panel".
    found: Arc<Vec<Entry>>,
    /// The folder and masks `found` came from (the panel's header).
    root: PathBuf,
    masks_text: String,
    /// The text of the search that found the results, for F3 (none for
    /// "Not containing").
    searched: Option<Text>,
    cursor: usize,
    list_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    running: Option<Running>,
    status: Status,
    next_id: u64,
    /// A relative "Search in" starts here: the active panel's folder.
    base: PathBuf,
    home: PathBuf,
    pub(crate) buttons: Entity<ButtonRow>,
    /// Tests: the next search waits before its walk until released.
    #[cfg(test)]
    pub hold: bool,
    #[cfg(test)]
    held: Option<Arc<AtomicBool>>,
}

impl EventEmitter<FindEvent> for FindDialog {}

impl FindDialog {
    pub fn new(home: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        let on = |f: fn(&mut Self, &mut Window, &mut Context<Self>)| -> OnPress {
            let this = this.clone();
            Rc::new(move |window, cx| {
                let _ = this.update(cx, |v, cx| f(v, window, cx));
            })
        };
        let buttons = ButtonRow::build(
            [
                ("Search", on(Self::search_or_stop)),
                ("Feed to panel", on(|v, _, cx| v.feed(cx))),
                ("Close", on(|v, _, cx| v.close(cx))),
            ],
            0,
            cx,
        );
        let this = Self {
            masks: cx.new(|cx| InputState::new(window, cx)),
            search_in: cx.new(|cx| InputState::new(window, cx)),
            text: cx.new(|cx| InputState::new(window, cx)),
            options: [false; 5],
            option_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            skip: Self::saved_skip(cx),
            skip_focus: cx.focus_handle().tab_stop(true),
            skip_popup_focus: cx.focus_handle(),
            skip_open: false,
            skip_cursor: 0,
            grown: Size::default(),
            resizing: None,
            found: Arc::default(),
            root: PathBuf::new(),
            masks_text: String::new(),
            searched: None,
            cursor: 0,
            list_focus: cx.focus_handle().tab_stop(true),
            scroll: UniformListScrollHandle::new(),
            running: None,
            status: Status::Idle,
            next_id: 0,
            base: home.clone(),
            home,
            buttons,
            #[cfg(test)]
            hold: false,
            #[cfg(test)]
            held: None,
        };
        this.update_buttons(cx);
        this
    }

    /// Alt-F7 again: "Search in" becomes `folder` (the active panel's);
    /// the other fields and the results stay.
    pub fn reopen(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let text = folder.display().to_string();
        self.search_in
            .update(cx, |s, cx| s.set_value(text, window, cx));
        self.base = folder;
        self.masks.update(cx, |s, cx| {
            let end = s.value().len();
            s.set_selected_range(0..end, cx);
        });
    }

    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    pub fn masks_focus(&self, cx: &App) -> FocusHandle {
        self.masks.focus_handle(cx)
    }

    fn input(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::Masks => &self.masks,
            Field::SearchIn => &self.search_in,
            Field::Text => &self.text,
        }
    }

    pub fn field(&self, field: Field, cx: &App) -> String {
        self.input(field).read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn set_field(
        &mut self,
        field: Field,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        self.input(field)
            .clone()
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }

    #[cfg(test)]
    pub fn focus_field(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        self.input(field)
            .clone()
            .update(cx, |s, cx| s.focus(window, cx));
    }

    #[cfg(test)]
    pub fn result_labels(&self) -> Vec<String> {
        self.found.iter().map(|e| e.label.clone()).collect()
    }

    /// The option boxes, in `OPTIONS` order.
    #[cfg(test)]
    pub fn options(&self) -> [bool; 5] {
        self.options
    }

    #[cfg(test)]
    pub fn found_entries(&self) -> Arc<Vec<Entry>> {
        self.found.clone()
    }

    #[cfg(test)]
    pub fn status(&self) -> String {
        status_text(&self.status)
    }

    #[cfg(test)]
    pub fn held(&self) -> Option<Arc<AtomicBool>> {
        self.held.clone()
    }

    fn search_or_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.stop(cx);
        } else {
            self.search(window, cx);
        }
    }

    /// Starts a search with the fields as they are. Bad input shows an
    /// error box and starts nothing.
    pub fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = match self.query(cx) {
            Ok(query) => query,
            Err((field, message)) => {
                let focus = self.input(field).focus_handle(cx);
                show_error("Cannot search", message, Some(focus), window, cx);
                return;
            }
        };
        self.stop(cx);
        // A fresh list: a results panel may share the old one.
        self.found = Arc::default();
        self.cursor = 0;
        self.root = query.root.clone();
        self.masks_text = self.field(Field::Masks, cx);
        self.searched = self.text(cx).filter(|t| !t.not_containing);
        self.next_id += 1;
        let id = self.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(test)]
        let hold = std::mem::take(&mut self.hold).then(|| Arc::new(AtomicBool::new(true)));
        #[cfg(test)]
        {
            self.held = hold.clone();
        }
        {
            let (cancel, progress) = (cancel.clone(), progress.clone());
            std::thread::spawn(move || {
                #[cfg(test)]
                while hold.as_ref().is_some_and(|h| h.load(Ordering::Relaxed)) {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                let summary = search(&query, &cancel, &progress, &mut |found| {
                    let _ = tx.send(Msg::Found(found));
                });
                let _ = tx.send(Msg::Done(summary));
            });
        }
        self.running = Some(Running {
            id,
            cancel,
            progress,
            rx,
        });
        self.status = Status::Running { seen: 0, found: 0 };
        self.update_buttons(cx);
        self.list_focus.focus(window, cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                let waiting = this.update(cx, |this, cx| this.poll(id, cx));
                if !matches!(waiting, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The query the fields describe, or the field at fault and why.
    /// The "Containing text" part of the fields, if any.
    fn text(&self, cx: &App) -> Option<Text> {
        let pattern = self.field(Field::Text, cx);
        (!pattern.is_empty()).then(|| Text {
            pattern,
            case_sensitive: self.options[CASE_TEXT],
            regex: self.options[REGEX],
            whole_words: self.options[WHOLE_WORDS],
            not_containing: self.options[NOT_CONTAINING],
        })
    }

    fn query(&self, cx: &App) -> Result<Query, (Field, String)> {
        let typed = self.field(Field::SearchIn, cx);
        let typed = typed.trim();
        let root = if typed.starts_with('~') {
            hotlist::expand(typed, &self.home)
        } else {
            self.base.join(typed)
        };
        if !root.is_dir() {
            return Err((
                Field::SearchIn,
                format!("{} is not a folder.", root.display()),
            ));
        }
        let text = match self.text(cx) {
            None => None,
            Some(text) => Some(
                text.compile()
                    .map_err(|e| (Field::Text, format!("Invalid regular expression: {e}")))?,
            ),
        };
        Ok(Query {
            root,
            masks: Masks::new(&self.field(Field::Masks, cx), self.options[CASE_NAMES]),
            skip: self.skipped_names(),
            text,
        })
    }

    /// Takes in what search `id` sent. Returns whether it still runs.
    fn poll(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.running.as_ref().filter(|r| r.id == id) else {
            return false;
        };
        let before = self.found.len();
        let mut done = None;
        loop {
            match running.rx.try_recv() {
                // Not shared while a search runs, so no copy is made.
                Ok(Msg::Found(found)) => Arc::make_mut(&mut self.found).push(found),
                Ok(Msg::Done(summary)) => {
                    done = Some(summary);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                // The thread died (it can't, short of a panic): stopped.
                Err(TryRecvError::Disconnected) => {
                    done = Some(SearchSummary {
                        found: self.found.len() as u64,
                        stopped: true,
                        ..running.progress.so_far()
                    });
                    break;
                }
            }
        }
        let status = match done {
            Some(summary) => Status::Done(summary),
            None => Status::Running {
                seen: running.progress.so_far().seen,
                found: self.found.len() as u64,
            },
        };
        if done.is_some() {
            self.running = None;
            self.update_buttons(cx);
        }
        if status != self.status || self.found.len() != before {
            self.status = status;
            cx.notify();
        }
        self.running.is_some()
    }

    /// Stops a running search; what it found so far stays.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(running) = self.running.take() else {
            return;
        };
        running.cancel.store(true, Ordering::Relaxed);
        // The counts so far: unreadable and skipped entries stay shown.
        self.status = Status::Done(SearchSummary {
            found: self.found.len() as u64,
            stopped: true,
            ..running.progress.so_far()
        });
        self.update_buttons(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        cx.emit(FindEvent::Closed);
    }

    /// "Feed to panel" (also Alt-L).
    pub fn feed(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() || self.found.is_empty() {
            return;
        }
        cx.emit(FindEvent::Feed(
            Results::new(
                self.root.clone(),
                self.masks_text.clone(),
                self.found.clone(),
            )
            .with_text(self.searched.clone()),
        ));
    }

    /// Search becomes Stop while running; Feed only with finished results.
    fn update_buttons(&self, cx: &mut Context<Self>) {
        let running = self.running.is_some();
        let can_feed = !running && !self.found.is_empty();
        self.buttons.update(cx, |row, cx| {
            row.set_label(0, if running { "Stop" } else { "Search" });
            row.set_enabled(1, can_feed);
            cx.notify();
        });
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.found.len().saturating_sub(1);
        self.select(self.cursor.saturating_add_signed(by).min(last), cx);
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = ix;
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn cursor_path(&self) -> Option<PathBuf> {
        self.found.get(self.cursor).map(|e| self.root.join(&e.name))
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.cursor_path() {
            cx.emit(FindEvent::GoTo(path));
        }
    }

    fn view(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.found.get(self.cursor) else {
            return;
        };
        if entry.kind == EntryKind::File {
            cx.emit(FindEvent::View(
                self.root.join(&entry.name),
                self.searched.clone(),
            ));
        }
    }

    /// The cursor is bright only while the list has the keys; otherwise it
    /// is muted like an inactive panel's.
    fn render_rows(
        &mut self,
        range: Range<usize>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let colors = Theme::get(cx).colors.clone();
        range
            .map(|ix| {
                let entry = &self.found[ix];
                let mut label = entry.label.clone();
                if entry.kind == EntryKind::Dir {
                    label.push('/');
                }
                div()
                    .debug_selector(move || format!("find-result-{ix}"))
                    .px(px(8.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(ix == self.cursor && focused, |d| {
                        d.bg(colors.accent).text_color(colors.text_on_accent)
                    })
                    .when(ix == self.cursor && !focused, |d| {
                        d.bg(colors.cursor_inactive_bg)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.select(ix, cx);
                            this.list_focus.focus(window, cx);
                            if event.click_count == 2 {
                                this.open(cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .when(ix == self.cursor, |d| {
                                d.debug_selector(move || {
                                    if focused {
                                        "find-cursor-focused"
                                    } else {
                                        "find-cursor"
                                    }
                                    .into()
                                })
                            })
                            .child(label),
                    )
            })
            .collect()
    }

    /// The dialog's width (the builder reads it on every render).
    pub fn width(&self) -> Pixels {
        px(MIN_WIDTH) + self.grown.width
    }

    fn start_resize(&mut self, at: Point<Pixels>) {
        self.resizing = Some((at, self.grown));
    }

    /// The corner follows the mouse: the dialog is centered, so the width
    /// grows twice the move. Never below the size it opened with.
    fn resize_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((start, grown)) = self.resizing else {
            return;
        };
        let width = (grown.width + (at.x - start.x) * 2.0).max(px(0.0));
        let height = (grown.height + (at.y - start.y)).max(px(0.0));
        self.grown = size(width, height);
        cx.notify();
    }

    fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            cx.notify();
        }
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.options[ix] = !self.options[ix];
        cx.notify();
    }

    /// The skip choices from the state file, or the defaults.
    fn saved_skip(cx: &App) -> [bool; SKIP_FOLDERS.len()] {
        let saved = cx.global::<AppState>().state.find_skip.clone();
        let on: Vec<String> = saved.unwrap_or_else(|| DEFAULT_SKIP.map(str::to_owned).to_vec());
        SKIP_FOLDERS.map(|f| on.iter().any(|s| s == f))
    }

    /// What the "Skip folders" control says.
    pub fn skip_text(&self) -> String {
        skip_text(&self.skip)
    }

    fn skipped_names(&self) -> Vec<String> {
        SKIP_FOLDERS
            .iter()
            .zip(self.skip)
            .filter(|(_, on)| *on)
            .map(|(f, _)| (*f).to_owned())
            .collect()
    }

    fn open_skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.skip_open = true;
        self.skip_cursor = 0;
        self.skip_popup_focus.focus(window, cx);
        cx.notify();
    }

    fn close_skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.skip_open {
            self.skip_open = false;
            self.skip_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn step_skip(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = SKIP_FOLDERS.len() as isize - 1;
        self.skip_cursor = (self.skip_cursor as isize + by).clamp(0, last) as usize;
        cx.notify();
    }

    /// Turns a folder on or off; remembered across restarts (state file).
    fn toggle_skip(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.skip[ix] = !self.skip[ix];
        let names = self.skipped_names();
        cx.global_mut::<AppState>().state.find_skip = Some(names);
        cx.notify();
    }

    /// The "Skip folders" control, and its popup while open.
    fn skip_control(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let colors = Theme::get(cx).colors.clone();
        let focused = self.skip_focus.is_focused(window);
        let control = div()
            .id("find-skip")
            .debug_selector(|| "find-skip".into())
            .key_context(FIND_SKIP_BUTTON_CONTEXT)
            .track_focus(&self.skip_focus)
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .h(px(SKIP_CONTROL_HEIGHT))
            .px(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(colors.border)
            .cursor_pointer()
            .on_action(
                cx.listener(|this, _: &act::SkipOpen, window, cx| this.open_skip(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if this.skip_open {
                        this.close_skip(window, cx);
                    } else {
                        this.open_skip(window, cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.skip_text()),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(14.0))
                    .text_color(colors.text_dim),
            )
            .when(focused, |d| d.child(focus_ring(colors.accent, "find-skip")));
        let popup = self.skip_open.then(|| {
            let rows = SKIP_FOLDERS.iter().enumerate().map(|(ix, name)| {
                let at_cursor = ix == self.skip_cursor;
                let on = self.skip[ix];
                div()
                    .id(("find-skip-row", ix))
                    .debug_selector(move || format!("find-skip-row-{ix}"))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(2.0))
                    .when(at_cursor, |d| {
                        d.bg(colors.accent).text_color(colors.text_on_accent)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.skip_cursor = ix;
                            this.toggle_skip(ix, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(crate::option_box::check_square(on, &colors))
                    .child(*name)
            });
            deferred(
                anchored().snap_to_window().child(
                    div()
                        .id("find-skip-popup")
                        .debug_selector(|| "find-skip-popup".into())
                        .key_context(FIND_SKIP_CONTEXT)
                        .track_focus(&self.skip_popup_focus)
                        .occlude()
                        // Anchored at the control's top: open below it.
                        .mt(px(SKIP_CONTROL_HEIGHT + 2.0))
                        .min_w(px(200.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(colors.accent)
                        .bg(colors.panel_bg)
                        .text_color(colors.text)
                        .on_action(
                            cx.listener(|this, _: &act::SkipUp, _, cx| this.step_skip(-1, cx)),
                        )
                        .on_action(
                            cx.listener(|this, _: &act::SkipDown, _, cx| this.step_skip(1, cx)),
                        )
                        .on_action(cx.listener(|this, _: &act::SkipToggle, _, cx| {
                            this.toggle_skip(this.skip_cursor, cx)
                        }))
                        .on_action(cx.listener(|this, _: &act::SkipClose, window, cx| {
                            this.close_skip(window, cx)
                        }))
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.close_skip(window, cx)
                        }))
                        .children(rows),
                ),
            )
            // Above the dialog layer (priority 10 and up), like
            // gpui-base's own popups.
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
        });
        div().flex().flex_col().child(control).children(popup)
    }

    fn option_box(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let (id, label) = OPTIONS[ix];
        let this = cx.entity().downgrade();
        let toggle = move |_: &mut Window, cx: &mut App| {
            let _ = this.update(cx, |this, cx| this.toggle(ix, cx));
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
    }
}

/// Size of the grip's mouse target.
const GRIP: f32 = 16.0;
/// The grip's inset from the dialog's right and bottom edges (the footer
/// sits 16 px in from both, the dialog's padding).
const GRIP_INSET: f32 = 5.0;
/// Its dots: 2 px squares on a 4 px grid, a triangle in the corner.
const GRIP_DOTS: [(f32, f32); 6] = [(8., 0.), (4., 4.), (8., 4.), (0., 8.), (4., 8.), (8., 8.)];

/// The resize grip in the dialog's bottom-right corner (it sits in the
/// footer, outside the view). Drawn from squares, not a glyph, so it lands
/// at the same inset from both edges.
pub fn grip(view: &Entity<FindDialog>, cx: &App) -> Stateful<Div> {
    let colors = Theme::get(cx).colors.clone();
    let view = view.clone();
    let offset = px(GRIP_INSET - 16.0);
    div()
        .id("find-grip")
        .debug_selector(|| "find-grip".into())
        .absolute()
        .right(offset)
        .bottom(offset)
        .size(px(GRIP))
        .cursor(CursorStyle::ResizeUpLeftDownRight)
        .child(
            div()
                .debug_selector(|| "find-grip-dots".into())
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(10.0))
                .children(GRIP_DOTS.map(|(x, y)| {
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .size(px(2.0))
                        .bg(colors.text_dim)
                })),
        )
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
            view.update(cx, |v, _| v.start_resize(event.position));
            cx.stop_propagation();
        })
}

impl Render for FindDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let list_focused = self.list_focus.is_focused(window);
        // A drag of the corner goes on wherever the mouse is.
        let this = cx.weak_entity();
        let follow = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let _ = moved.update(cx, |v, cx| {
                        if v.resizing.is_none() {
                            return;
                        }
                        if event.pressed_button == Some(MouseButton::Left) {
                            v.resize_to(event.position, cx);
                        } else {
                            v.end_resize(cx); // released outside the window
                        }
                    });
                });
                let released = this.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = released.update(cx, |v, cx| v.end_resize(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let row = |label: &'static str, field: Div| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().flex_none().w(px(140.0)).child(label))
                .child(div().flex_1().child(field))
        };
        let list = uniform_list(
            "find-results",
            self.found.len(),
            cx.processor(move |this, range, _window, cx| this.render_rows(range, list_focused, cx)),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .key_context(FIND_DIALOG_CONTEXT)
            .on_action(cx.listener(|this, _: &act::Feed, _, cx| this.feed(cx)))
            .relative()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(follow)
            // Each case box sits by what it applies to: names here, the
            // text's options under its field.
            .child(
                row("Search for", dialog_field(&self.masks))
                    .child(self.option_box(CASE_NAMES, window, cx).flex_none()),
            )
            .child(row("Search in", div().child(text_field(&self.search_in))))
            .child(row("Skip folders", self.skip_control(window, cx)))
            // Which files above, what's in them below.
            .child(div().h(px(1.0)).my(px(4.0)).bg(colors.border))
            .child(row("Containing text", div().child(text_field(&self.text))))
            .child(
                // Two aligned columns under the field.
                div()
                    .flex()
                    .flex_wrap()
                    .gap_y(px(8.0))
                    .ml(px(148.0))
                    .children([CASE_TEXT, WHOLE_WORDS, REGEX, NOT_CONTAINING].map(|ix| {
                        div()
                            .flex()
                            .w(px(OPTION_COLUMN))
                            .child(self.option_box(ix, window, cx).flex_none())
                    })),
            )
            .child(
                div()
                    .id("find-list")
                    .debug_selector(|| "find-list".into())
                    .key_context(FIND_RESULTS_CONTEXT)
                    .track_focus(&self.list_focus)
                    .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
                    .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
                    .on_action(cx.listener(|this, _: &act::PageUp, _, cx| {
                        this.step(-(PAGE_ROWS as isize), cx)
                    }))
                    .on_action(cx.listener(|this, _: &act::PageDown, _, cx| {
                        this.step(PAGE_ROWS as isize, cx)
                    }))
                    .on_action(cx.listener(|this, _: &act::Home, _, cx| this.select(0, cx)))
                    .on_action(cx.listener(|this, _: &act::End, _, cx| this.step(isize::MAX, cx)))
                    .on_action(cx.listener(|this, _: &act::Open, _, cx| this.open(cx)))
                    .on_action(cx.listener(|this, _: &act::View, _, cx| this.view(cx)))
                    .mt(px(4.0))
                    .h(px(LIST_HEIGHT) + self.grown.height)
                    .border_1()
                    .border_color(if list_focused {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .bg(colors.panel_bg)
                    .child(list),
            )
            .child(
                div()
                    .debug_selector(|| "find-status".into())
                    .h(px(20.0))
                    .text_color(colors.text_secondary)
                    .child(SharedString::from(status_text(&self.status))),
            )
    }
}

/// The "Skip folders" control's text: the folders on (`skip` follows
/// `SKIP_FOLDERS`), the first two and a count past three.
pub fn skip_text(skip: &[bool]) -> String {
    let on: Vec<&str> = SKIP_FOLDERS
        .iter()
        .zip(skip)
        .filter(|(_, on)| **on)
        .map(|(f, _)| *f)
        .collect();
    match on.len() {
        0 => "none".to_owned(),
        1..=3 => on.join(", "),
        n => format!("{} +{}", on[..2].join(", "), n - 2),
    }
}

/// The status line under the results.
pub fn status_text(status: &Status) -> String {
    match *status {
        Status::Idle => String::new(),
        Status::Running { seen, found } => format!(
            "Searching... {} files, {} found",
            format_count(seen),
            format_count(found)
        ),
        Status::Done(s) => {
            let mut text = format!("{} found", format_count(s.found));
            if s.unreadable > 0 {
                text.push_str(&format!(", {} unreadable", format_count(s.unreadable)));
            }
            match s.other_filesystems {
                0 => {}
                1 => text.push_str(", 1 other filesystem skipped"),
                n => text.push_str(&format!(", {} other filesystems skipped", format_count(n))),
            }
            match s.skipped {
                0 => {}
                1 => text.push_str(", 1 folder skipped"),
                n => text.push_str(&format!(", {} folders skipped", format_count(n))),
            }
            if s.stopped {
                text.push_str(", stopped");
            }
            text
        }
    }
}
