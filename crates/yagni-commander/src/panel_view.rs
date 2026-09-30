//! One side of the file manager: path header, column headers, the entry list
//! and a summary footer. Reads its panel from the shared [`Commander`].

use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    Context, Div, Entity, MouseButton, MouseDownEvent, Rgba, ScrollStrategy, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use yagni_commander_core::{Command, Commander, Entry, EntryKind, Side, SortKey};

use crate::columns::COLUMNS;
use crate::file_manager::execute;
use crate::theme::Theme;

const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 28.0;
const COLUMN_HEADER_HEIGHT: f32 = 22.0;
const FOOTER_HEIGHT: f32 = 24.0;
const CELL_SPACING: f32 = 10.0;

pub struct PanelView {
    commander: Entity<Commander>,
    side: Side,
    scroll: UniformListScrollHandle,
    /// Directory and cursor last scrolled into view. Scrolling only follows
    /// actual cursor moves, so mouse-wheel scrolling isn't undone by unrelated
    /// updates such as activity in the other panel.
    revealed: Option<(PathBuf, usize)>,
    _subscriptions: Vec<Subscription>,
}

impl PanelView {
    pub fn new(commander: Entity<Commander>, side: Side, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&commander, |this, _, cx| {
            this.reveal_cursor(cx);
            cx.notify();
        });
        Self {
            commander,
            side,
            scroll: UniformListScrollHandle::new(),
            revealed: None,
            _subscriptions: vec![subscription],
        }
    }

    /// Number of fully visible rows, once the list has been laid out.
    pub fn visible_rows(&self) -> Option<usize> {
        let height = f32::from(self.scroll.0.borrow().base_handle.bounds().size.height);
        (height > 0.0).then(|| (height / ROW_HEIGHT) as usize)
    }

    fn reveal_cursor(&mut self, cx: &mut Context<Self>) {
        let panel = self.commander.read(cx).panel(self.side);
        if self
            .revealed
            .as_ref()
            .is_some_and(|(path, cursor)| path == panel.path() && *cursor == panel.cursor())
        {
            return;
        }
        self.scroll
            .scroll_to_item(panel.cursor(), ScrollStrategy::Nearest);
        self.revealed = Some((panel.path().to_path_buf(), panel.cursor()));
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let side = self.side;
        let theme = Theme::get(cx);
        let commander = self.commander.read(cx);
        let panel = commander.panel(side);
        let is_active = commander.active() == side;
        range
            .map(|ix| {
                let cursor = (ix == panel.cursor()).then_some(is_active);
                entry_row(&panel.entries()[ix], cursor, theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        execute(&this.commander, Command::CursorTo(side, ix), cx);
                        if event.click_count == 2 {
                            execute(&this.commander, Command::Activate, cx);
                        }
                    }),
                )
            })
            .collect()
    }

    /// Clickable column titles; clicking sorts the panel by that column.
    fn render_column_headers(&self, cx: &mut Context<Self>) -> Div {
        let side = self.side;
        let theme = Theme::get(cx);
        let sort = self.commander.read(cx).panel(side).sort();
        div()
            .h(px(COLUMN_HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(CELL_SPACING))
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .children(COLUMNS.iter().map(|column| {
                let key = column.key;
                column
                    .cell()
                    .cursor_pointer()
                    .text_color(if sort.key == key {
                        theme.text
                    } else {
                        theme.text_dim
                    })
                    .child(column.header(sort))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            execute(&this.commander, Command::SortBy(side, key), cx);
                        }),
                    )
            }))
    }
}

impl Render for PanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let commander = self.commander.read(cx);
        let panel = commander.panel(self.side);
        let is_active = commander.active() == self.side;

        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .bg(if is_active {
                theme.header_active_bg
            } else {
                theme.header_bg
            })
            .text_color(if is_active {
                theme.text
            } else {
                theme.text_dim
            })
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .child(panel.path().display().to_string()),
            );

        let dirs = panel
            .entries()
            .iter()
            .filter(|e| e.kind == EntryKind::Dir)
            .count();
        let files = panel
            .entries()
            .iter()
            .filter(|e| e.kind == EntryKind::File)
            .count();
        let footer = div()
            .h(px(FOOTER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .bg(theme.header_bg)
            .text_color(theme.text_dim)
            .text_size(px(12.0))
            .child(format!("{dirs} dirs, {files} files"));

        let border = if is_active {
            theme.accent
        } else {
            theme.border
        };
        let panel_bg = theme.panel_bg;
        let entry_count = panel.entries().len();

        let list = uniform_list(
            ("entries", self.side as usize),
            entry_count,
            cx.processor(|this, range, _window, cx| this.render_rows(range, cx)),
        )
        .track_scroll(&self.scroll)
        .flex_1();

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .bg(panel_bg)
            .child(header)
            .child(self.render_column_headers(cx))
            .child(list)
            .child(footer)
    }
}

/// `cursor`: `None` if not under the cursor, `Some(panel_is_active)` otherwise.
fn entry_row(entry: &Entry, cursor: Option<bool>, theme: &Theme) -> Div {
    let name_color = name_color(entry, theme);
    let (fg, bg) = match cursor {
        Some(true) => (theme.text_on_accent, Some(theme.accent)),
        Some(false) => (name_color, Some(theme.cursor_inactive_bg)),
        None => (name_color, None),
    };
    let detail = if cursor == Some(true) {
        fg
    } else {
        theme.text_dim
    };
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(CELL_SPACING))
        .when_some(bg, |d, bg| d.bg(bg))
        .children(COLUMNS.iter().map(|column| {
            let is_name = column.key == SortKey::Name;
            column
                .cell()
                .when(!is_name, |d| d.text_size(px(13.0)))
                .text_color(if is_name { fg } else { detail })
                .child(column.text(entry))
        }))
}

fn name_color(entry: &Entry, theme: &Theme) -> Rgba {
    match entry.kind {
        EntryKind::Parent | EntryKind::Dir if entry.is_symlink => theme.symlink,
        EntryKind::Parent | EntryKind::Dir => theme.dir,
        EntryKind::File => theme.text,
    }
}
