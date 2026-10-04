//! The F3 viewer: a read-only window on one file. All byte work is in
//! `yagni_commander_core::viewer`; this draws the rows around the top byte
//! position, handles keys and the mouse, and shows the line count once the
//! background scan finishes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, AppContext, Context, FocusHandle, HighlightStyle, SharedString, StyledText, Subscription,
    Window, WindowBounds, WindowOptions, div, font, prelude::*, px,
};
use yagni_commander_core::archive::IN_ARCHIVE;
use yagni_commander_core::find::text::Text;
use yagni_commander_core::format_size;
use yagni_commander_core::launch;
use yagni_commander_core::viewer::{
    Direction, Document, FileSource, Highlight, LineIndex, Row, Visible, Wrap, count_lines,
};

use crate::actions::VIEWER_CONTEXT;
use crate::actions::viewer::{
    Close, Copy as CopySelection, Edit, End, Find, FindNext, FindPrevious, LineDown, LineUp,
    PageDown, PageUp, ScrollLeft, ScrollRight, SelectAll, Start, ToggleHex, ToggleWrap, ZoomIn,
    ZoomOut, ZoomReset,
};
use crate::app_state::AppState;
use crate::file_manager::commands::{configured_editor, show_error};
use crate::rem_scope::RemScope;
use crate::theme::Theme;
use crate::zoom::{Zoom, rems_from_px, scaled};

mod search;
mod select;

use select::{Run, Selection};

const PADDING: f32 = 8.0;
const LINE_HEIGHT: f32 = 18.0;
const STATUS_HEIGHT: f32 = 22.0;
const SCROLLBAR_WIDTH: f32 = 12.0;
/// The text size (gpui-component's default monospace size).
const MONO_SIZE: f32 = 13.0;
/// The status line's text size.
const STATUS_TEXT: f32 = 12.0;
/// Columns per Left/Right.
const H_STEP: u32 = 8;
/// Before the first layout.
const FALLBACK_ROWS: usize = 20;

pub fn window_title(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    format!("{name} - yagni-commander")
}

/// Opens `path` in a new viewer window, placed where the last viewer was or
/// over the main window (`main`).
/// `temp`: the private copy of an archive entry that `path` is in, deleted
/// with the window.
/// `search`: Alt-F7's text, searched for from the start of the file.
pub fn open(
    path: PathBuf,
    main: WindowBounds,
    temp: Option<tempfile::TempDir>,
    search: Option<Text>,
    cx: &mut App,
) -> io::Result<()> {
    let source = FileSource::open(&path)?;
    let options = WindowOptions {
        window_bounds: Some(cx.global::<AppState>().viewer_bounds(main, cx)),
        app_id: Some(crate::windows::APP_ID.into()),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut view = ViewerView::new(path, source, temp, window, cx);
            if let Some(text) = search {
                view.search_from_start(text, window, cx);
            }
            view
        })
    })
    .map_err(io::Error::other)?;
    Ok(())
}

pub struct ViewerView {
    path: PathBuf,
    doc: Document<FileSource>,
    focus: FocusHandle,
    wrap: bool,
    /// Hex mode (`H`); `wrap` is kept for the way back.
    hex: bool,
    /// Byte position of the top row (always a row start).
    top: u64,
    /// Columns per row in wrap mode, from the last layout.
    cols: u32,
    screen_rows: usize,
    /// No-wrap mode: first column shown.
    h_offset: u32,
    /// Widest row on screen, for clamping `h_offset`.
    widest: u32,
    lines: Option<Arc<LineIndex>>,
    /// Line of `top`, cached while `top` doesn't change.
    top_line: Option<(u64, u64)>,
    cancel_count: Arc<AtomicBool>,
    /// Ctrl-F / F3 / Shift-F3.
    search: search::SearchState,
    /// Tests: what the rows last drawn showed.
    #[cfg(test)]
    shown: Shown,
    /// The mouse selection (a byte range), and whether a drag is extending
    /// it.
    selection: Option<Selection>,
    selecting: bool,
    /// Auto-scroll is ticking (a drag past an edge).
    autoscrolling: bool,
    /// Where the mouse was last, for auto-scroll.
    last_mouse: gpui_kit::Point<gpui_kit::Pixels>,
    /// Width of a column in pixels, from the last layout.
    char_width: f32,
    /// The rows last drawn, for hit tests.
    drawn: Vec<Row>,
    /// Bytes shown, for the scrollbar thumb and the percentage.
    shown_end: u64,
    dragging_thumb: bool,
    /// Fractional rows of wheel scrolling not applied yet.
    wheel_rows: f32,
    _subscriptions: Vec<Subscription>,
    /// The private copy of an archive entry, deleted with the window.
    temp: Option<tempfile::TempDir>,
}

impl ViewerView {
    fn new(
        path: PathBuf,
        source: FileSource,
        temp: Option<tempfile::TempDir>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        window.set_window_title(&window_title(&path));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subscriptions = vec![cx.observe_window_bounds(window, |_, window, cx| {
            AppState::remember_viewer(window.window_bounds(), cx);
        })];
        AppState::remember_viewer(window.window_bounds(), cx);

        let cancel_count = Arc::new(AtomicBool::new(false));
        if let Ok(counter) = source.try_clone() {
            let cancel = cancel_count.clone();
            let counting = cx
                .background_executor()
                .spawn(async move { count_lines(&counter, &cancel) });
            cx.spawn(async move |this, cx| {
                if let Ok(Some(index)) = counting.await {
                    let _ = this.update(cx, |this, cx| {
                        this.lines = Some(Arc::new(index));
                        cx.notify();
                    });
                }
            })
            .detach();
        }

        Self {
            path,
            doc: Document::new(source),
            focus,
            wrap: true,
            hex: false,
            top: 0,
            cols: 80,
            screen_rows: FALLBACK_ROWS,
            h_offset: 0,
            widest: 0,
            lines: None,
            top_line: None,
            cancel_count,
            search: search::SearchState::default(),
            #[cfg(test)]
            shown: Shown::default(),
            selection: None,
            selecting: false,
            autoscrolling: false,
            last_mouse: Default::default(),
            char_width: 8.0,
            drawn: Vec::new(),
            shown_end: 0,
            dragging_thumb: false,
            wheel_rows: 0.0,
            _subscriptions: subscriptions,
            temp,
        }
    }

    fn layout(&self) -> Wrap {
        if self.hex {
            Wrap::Hex
        } else if self.wrap {
            Wrap::Columns(self.cols)
        } else {
            Wrap::Off
        }
    }

    fn set_top(&mut self, top: u64, cx: &mut Context<Self>) {
        self.top = top;
        cx.notify();
    }

    fn down(&mut self, by: usize, cx: &mut Context<Self>) {
        let top = self
            .doc
            .scroll_down(self.top, by, self.screen_rows, self.layout());
        self.set_top(top, cx);
    }

    fn up(&mut self, by: usize, cx: &mut Context<Self>) {
        let top = self.doc.scroll_up(self.top, by, self.layout());
        self.set_top(top, cx);
    }

    fn page(&self) -> usize {
        self.screen_rows.saturating_sub(1).max(1)
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        let top = self.doc.last_top(self.screen_rows, self.layout());
        self.set_top(top, cx);
    }

    /// Whether rows are cut to the window (wrap mode) rather than scrolled
    /// sideways (no wrap, hex).
    fn wraps_rows(&self) -> bool {
        self.layout() == Wrap::Columns(self.cols)
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        if self.hex {
            return;
        }
        self.wrap = !self.wrap;
        self.relayout(cx);
    }

    fn toggle_hex(&mut self, cx: &mut Context<Self>) {
        self.hex = !self.hex;
        self.relayout(cx);
    }

    /// After a mode change: the row holding the top byte goes on top.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        self.h_offset = 0;
        let top = self.doc.row_start_at(self.top, self.layout());
        self.set_top(top, cx);
    }

    fn scroll_h(&mut self, right: bool, cx: &mut Context<Self>) {
        if self.wraps_rows() {
            return;
        }
        let max = self.widest.saturating_sub(self.cols);
        self.h_offset = if right {
            (self.h_offset + H_STEP).min(max.max(self.h_offset))
        } else {
            self.h_offset.saturating_sub(H_STEP)
        };
        cx.notify();
    }

    /// Escape: stops a running search, else closes the window.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.stop_search(cx) {
            window.remove_window();
        }
    }

    /// F4: opens the file in the configured editor; the viewer stays open.
    /// An archive entry is a private copy deleted with the window, so it is
    /// refused, as F4 is inside an archive in the panels.
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let refocus = Some(self.focus.clone());
        if self.temp.is_some() {
            show_error("Inside an archive", IN_ARCHIVE, refocus, window, cx);
            return;
        }
        let Some(editor) = configured_editor(refocus.clone(), window, cx) else {
            return;
        };
        if let Err(e) = launch::open_in_editor(&editor, &self.path) {
            show_error("Cannot open editor", e.to_string(), refocus, window, cx);
        }
    }

    /// Rows and columns that fit the window, and a column's width, from the
    /// monospace font.
    fn measure(&self, window: &mut Window, cx: &App) -> (usize, u32, f32) {
        let theme = cx.theme();
        let font = font(theme.mono_font_family.clone());
        let text_system = window.text_system();
        let char_width = text_system
            .advance(
                text_system.resolve_font(&font),
                px(at_level(MONO_SIZE, cx)),
                'M',
            )
            .map(|s| f32::from(s.width))
            .unwrap_or(8.0)
            .max(1.0);
        let viewport = window.viewport_size();
        let padding = padding(cx);
        let width = f32::from(viewport.width) - 2.0 * padding - scrollbar_width(cx);
        let height = f32::from(viewport.height) - 2.0 * padding - status_height(cx);
        let rows = (height / line_height(cx)).floor().max(1.0) as usize;
        let cols = (width / char_width).floor().max(1.0) as u32;
        (rows, cols, char_width)
    }

    /// Up to `n` rows from the top, with the search match and the selection.
    fn layout_rows(&mut self, n: usize) -> Vec<Row> {
        let selection = self.selection.as_ref().map(Selection::range);
        let hl = Highlight {
            mark: self.search.current.as_ref(),
            selection: selection.as_ref(),
        };
        self.doc.marked_rows(self.top, n, self.layout(), hl)
    }

    /// "file.txt · 1.2 MiB · 37% · wrap · line 120 of 5,000"
    pub(crate) fn status_text(&mut self) -> String {
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let len = self.doc.len();
        let percent = (self.shown_end * 100)
            .checked_div(len)
            .unwrap_or(100)
            .min(100);
        if let Some(searching) = self.searching_text() {
            return format!("{name} · {} · {searching}", format_size(len));
        }
        if self.hex {
            return format!(
                "{name} · {} · {percent}% · hex · offset {:X}",
                format_size(len),
                self.top
            );
        }
        let mode = if self.wrap { "wrap" } else { "no wrap" };
        let line = match &self.lines {
            None => "counting lines...".to_owned(),
            Some(index) => {
                let top_line = match self.top_line {
                    Some((top, line)) if top == self.top => line,
                    _ => {
                        let line = index.line_of(&mut self.doc, self.top);
                        self.top_line = Some((self.top, line));
                        line
                    }
                };
                if index.lines() == 0 {
                    "0 lines".to_owned()
                } else {
                    format!("line {top_line} of {}", index.lines())
                }
            }
        };
        format!(
            "{name} · {} · {percent}% · {mode} · {line}",
            format_size(len)
        )
    }

    #[cfg(test)]
    pub(crate) fn top(&self) -> u64 {
        self.top
    }
    #[cfg(test)]
    pub(crate) fn wraps(&self) -> bool {
        self.wrap
    }
    #[cfg(test)]
    pub(crate) fn hex(&self) -> bool {
        self.hex
    }

    #[cfg(test)]
    pub(crate) fn h_offset(&self) -> u32 {
        self.h_offset
    }
    #[cfg(test)]
    pub(crate) fn shown(&self) -> &[String] {
        &self.shown.rows
    }
    #[cfg(test)]
    pub(crate) fn screen_rows(&self) -> usize {
        self.screen_rows
    }

    #[cfg(test)]
    pub(crate) fn shown_selection(&self) -> &[Vec<String>] {
        &self.shown.selection
    }

    #[cfg(test)]
    pub(crate) fn selection_range(&self) -> Option<std::ops::Range<u64>> {
        self.selection
            .as_ref()
            .map(Selection::range)
            .filter(|r| !r.is_empty())
    }

    #[cfg(test)]
    pub(crate) fn char_width(&self) -> f32 {
        self.char_width
    }

    #[cfg(test)]
    pub(crate) fn shown_marks(&self) -> &[Vec<String>] {
        &self.shown.marks
    }

    #[cfg(test)]
    pub(crate) fn current_match(&self) -> Option<std::ops::Range<u64>> {
        self.search.current.clone()
    }

    #[cfg(test)]
    pub(crate) fn searching(&self) -> bool {
        self.search.running.is_some()
    }
}

/// Tests: the texts of the rows last drawn, and their marked and selected
/// parts.
#[cfg(test)]
#[derive(Default)]
struct Shown {
    rows: Vec<String>,
    marks: Vec<Vec<String>>,
    selection: Vec<Vec<String>>,
}

#[cfg(test)]
impl Shown {
    fn push(
        &mut self,
        text: &str,
        marks: &[std::ops::Range<usize>],
        selection: &[std::ops::Range<usize>],
    ) {
        let parts =
            |rs: &[std::ops::Range<usize>]| rs.iter().map(|r| text[r.clone()].to_owned()).collect();
        self.rows.push(text.to_owned());
        self.marks.push(parts(marks));
        self.selection.push(parts(selection));
    }
}

impl Drop for ViewerView {
    fn drop(&mut self) {
        self.cancel_count.store(true, Ordering::Relaxed);
    }
}

impl ViewerView {
    /// Scrollbar track geometry in window coordinates: (top, height). The
    /// layout is fixed (padding, status line), so it follows from the window.
    fn track(window: &Window, cx: &App) -> (f32, f32) {
        let padding = padding(cx);
        let height = f32::from(window.viewport_size().height) - 2.0 * padding - status_height(cx);
        (padding, height.max(1.0))
    }

    /// Shows the row containing the byte at `fraction` of the file, but never
    /// past the last screen.
    fn jump_to(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let len = self.doc.len();
        if len == 0 {
            return;
        }
        let pos = (len as f64 * f64::from(fraction.clamp(0.0, 1.0))) as u64;
        let wrap = self.layout();
        let top = self.doc.row_start_at(pos, wrap);
        let last = self.doc.last_top(self.screen_rows, wrap);
        self.set_top(top.min(last), cx);
    }

    fn drag_to(&mut self, y: f32, window: &Window, cx: &mut Context<Self>) {
        let (top, height) = Self::track(window, cx);
        self.jump_to((y - top) / height, cx);
    }

    fn on_wheel(&mut self, event: &gpui_kit::ScrollWheelEvent, cx: &mut Context<Self>) {
        let line = line_height(cx);
        let dy = f32::from(event.delta.pixel_delta(px(line)).y);
        self.wheel_rows += dy / line;
        let whole = self.wheel_rows.trunc();
        self.wheel_rows -= whole;
        let rows = whole.abs() as usize;
        if whole < 0.0 {
            self.down(rows, cx);
        } else if whole > 0.0 {
            self.up(rows, cx);
        }
    }

    fn render_scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let len = self.doc.len().max(1) as f32;
        let start = self.top as f32 / len;
        let size = ((self.shown_end - self.top) as f32 / len).max(0.02);
        div()
            .id("viewer-scrollbar")
            .debug_selector(|| "viewer-scrollbar".into())
            .w(rems_from_px(SCROLLBAR_WIDTH))
            .h_full()
            .flex_none()
            .relative()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                    this.dragging_thumb = true;
                    this.drag_to(f32::from(event.position.y), window, cx);
                }),
            )
            .child(
                div()
                    .debug_selector(|| "viewer-thumb".into())
                    .absolute()
                    .left(rems_from_px(2.0))
                    .right(rems_from_px(2.0))
                    .top(gpui_kit::relative(start.min(1.0 - size)))
                    .h(gpui_kit::relative(size.min(1.0)))
                    .rounded(rems_from_px(3.0))
                    .bg(if self.dragging_thumb {
                        colors.accent
                    } else {
                        colors.border
                    }),
            )
    }
}

impl Render for ViewerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, cols, char_width) = self.measure(window, cx);
        self.screen_rows = rows;
        self.char_width = char_width;
        if cols != self.cols {
            self.cols = cols;
            if self.wraps_rows() {
                self.top = self.doc.row_start_at(self.top, self.layout());
            }
        }
        let visible = self.layout_rows(rows);
        self.shown_end = visible.last().map_or(self.top, |r| r.end);
        self.widest = visible.iter().map(|r| r.width).max().unwrap_or(0);

        let colors = Theme::get(cx).colors.clone();
        let dim = HighlightStyle {
            color: Some(colors.hidden.into()),
            ..Default::default()
        };
        let marked = HighlightStyle {
            color: Some(colors.text_on_accent.into()),
            background_color: Some(colors.accent.into()),
            ..Default::default()
        };
        let selected = HighlightStyle {
            color: Some(colors.text_on_selected.into()),
            background_color: Some(colors.selected.into()),
            ..Default::default()
        };
        #[cfg(test)]
        {
            self.shown = Shown::default();
        }
        let mut row_elements = Vec::with_capacity(visible.len());
        for (ix, row) in visible.iter().enumerate() {
            let Visible {
                text,
                dim: dims,
                marks,
                selection,
            } = if self.wraps_rows() {
                Visible {
                    text: row.text.clone(),
                    dim: row.dim.clone(),
                    marks: row.marks.clone(),
                    selection: row.selection.clone(),
                }
            } else {
                row.visible(self.h_offset, self.cols)
            };
            #[cfg(test)]
            self.shown.push(&text, &marks, &selection);
            let highlights: Vec<_> = select::highlight_runs(&dims, &selection, &marks)
                .into_iter()
                .map(|(range, run)| {
                    let style = match run {
                        Run::Dim => dim,
                        Run::Selected => selected,
                        Run::Marked => marked,
                    };
                    (range, style)
                })
                .collect();
            let styled = StyledText::new(SharedString::from(text)).with_highlights(highlights);
            row_elements.push(
                div()
                    .debug_selector(move || format!("viewer-row-{ix}"))
                    .h(rems_from_px(LINE_HEIGHT))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(styled),
            );
        }
        self.drawn = visible;

        let status = self.status_text();
        let error = self.doc.error().map(str::to_owned);
        let mono = cx.theme().mono_font_family.clone();

        div()
            .key_context(VIEWER_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &Edit, window, cx| this.edit(window, cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, window, cx| {
                this.find_again(Direction::Forward, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindPrevious, window, cx| {
                this.find_again(Direction::Backward, window, cx)
            }))
            .on_action(cx.listener(|this, _: &LineDown, _, cx| this.down(1, cx)))
            .on_action(cx.listener(|this, _: &LineUp, _, cx| this.up(1, cx)))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let page = this.page();
                this.down(page, cx)
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let page = this.page();
                this.up(page, cx)
            }))
            .on_action(cx.listener(|this, _: &Start, _, cx| this.set_top(0, cx)))
            .on_action(cx.listener(|this, _: &End, _, cx| this.end(cx)))
            .on_action(cx.listener(|this, _: &ToggleWrap, _, cx| this.toggle_wrap(cx)))
            .on_action(cx.listener(|this, _: &ToggleHex, _, cx| this.toggle_hex(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &CopySelection, window, cx| this.copy(window, cx)))
            .on_action(cx.listener(|this, _: &ScrollLeft, _, cx| this.scroll_h(false, cx)))
            .on_action(cx.listener(|this, _: &ScrollRight, _, cx| this.scroll_h(true, cx)))
            .on_action(cx.listener(|_, _: &ZoomIn, _, cx| crate::zoom::change_viewer(1.0, cx)))
            .on_action(cx.listener(|_, _: &ZoomOut, _, cx| crate::zoom::change_viewer(-1.0, cx)))
            .on_action(cx.listener(|_, _: &ZoomReset, _, cx| crate::zoom::reset_viewer(cx)))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.on_wheel(event, cx)))
            .on_mouse_move(
                cx.listener(|this, event: &gpui_kit::MouseMoveEvent, window, cx| {
                    if !this.dragging_thumb {
                        return;
                    }
                    if event.pressed_button == Some(gpui_kit::MouseButton::Left) {
                        this.drag_to(f32::from(event.position.y), window, cx);
                    } else {
                        this.dragging_thumb = false; // released outside the window
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging_thumb = false;
                    cx.notify();
                }),
            )
            .size_full()
            .relative()
            .child(select::follow_drag(cx))
            .flex()
            .flex_col()
            .bg(colors.panel_bg)
            .text_color(colors.text)
            .child(
                // The viewer's own level: everything below sizes in rems of it.
                RemScope::new(px(Zoom::get(cx).viewer))
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .p(rems_from_px(PADDING))
                            .child(
                                div()
                                    .id("viewer-text")
                                    .on_mouse_down(
                                        gpui_kit::MouseButton::Left,
                                        cx.listener(|this, event, window, cx| {
                                            this.on_text_mouse_down(event, window, cx)
                                        }),
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .font_family(mono)
                                    .text_size(rems_from_px(MONO_SIZE))
                                    .line_height(rems_from_px(LINE_HEIGHT))
                                    .children(row_elements),
                            )
                            .child(self.render_scrollbar(cx)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "viewer-status".into())
                            .h(rems_from_px(STATUS_HEIGHT))
                            .flex_none()
                            .px(rems_from_px(10.0))
                            .flex()
                            .items_center()
                            .bg(colors.header_bg)
                            .text_size(rems_from_px(STATUS_TEXT))
                            .text_color(colors.text_secondary)
                            .child(match error {
                                Some(e) => div()
                                    .text_color(colors.error)
                                    .child(format!("Read error: {e}")),
                                None => div().child(status),
                            }),
                    ),
            )
    }
}

/// `px_at_base` (px at the 16 px base) in px at the viewer level: for
/// arithmetic on the sizes the viewer draws in rems.
fn at_level(px_at_base: f32, cx: &App) -> f32 {
    scaled(px_at_base, Zoom::get(cx).viewer)
}

/// A row's height in px at the viewer level.
fn line_height(cx: &App) -> f32 {
    at_level(LINE_HEIGHT, cx)
}

/// The padding around the rows, in px at the viewer level.
fn padding(cx: &App) -> f32 {
    at_level(PADDING, cx)
}

fn status_height(cx: &App) -> f32 {
    at_level(STATUS_HEIGHT, cx)
}

fn scrollbar_width(cx: &App) -> f32 {
    at_level(SCROLLBAR_WIDTH, cx)
}

#[cfg(test)]
pub(crate) mod tests;
