//! One side of the file manager: path header, column headers, the entry list
//! and a summary footer. Reads its panel from the shared [`Commander`].

use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::{
    Context, Div, Entity, MouseButton, MouseDownEvent, Rgba, ScrollStrategy, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use yagni_commander_core::{
    Command, Commander, Entry, EntryKind, Side, SortKey, Summary, format_size,
};

use crate::columns::COLUMNS;
use crate::file_manager::execute;
use crate::theme::{Colors, Theme};

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
        let colors = &Theme::get(cx).colors;
        let commander = self.commander.read(cx);
        let panel = commander.panel(side);
        let is_active = commander.active() == side;
        range
            .map(|ix| {
                let entry = &panel.entries()[ix];
                let row = RowState {
                    cursor: (ix == panel.cursor()).then_some(is_active),
                    selected: panel.is_selected(entry),
                };
                entry_row(entry, row, colors).on_mouse_down(
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
        let colors = &Theme::get(cx).colors;
        let sort = self.commander.read(cx).panel(side).sort();
        div()
            .h(px(COLUMN_HEADER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(CELL_SPACING))
            .border_b_1()
            .border_color(colors.border)
            .text_size(px(12.0))
            .children(COLUMNS.iter().map(|column| {
                let key = column.key;
                column
                    .cell()
                    .cursor_pointer()
                    .text_color(if sort.key == key {
                        colors.text
                    } else {
                        colors.text_dim
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
        let colors = &Theme::get(cx).colors;
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
                colors.header_active_bg
            } else {
                colors.header_bg
            })
            .text_color(if is_active {
                colors.text
            } else {
                colors.text_dim
            })
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .child(panel.path().display().to_string()),
            );

        let footer = div()
            .h(px(FOOTER_HEIGHT))
            .flex_none()
            .px(px(10.0))
            .flex()
            .items_center()
            .bg(colors.header_bg)
            .text_color(colors.text_dim)
            .text_size(px(12.0))
            .child(footer_text(&panel.summary()));

        let border = if is_active {
            colors.accent
        } else {
            colors.border
        };
        let panel_bg = colors.panel_bg;
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

#[derive(Clone, Copy)]
struct RowState {
    /// `None` if not under the cursor, `Some(panel_is_active)` otherwise.
    cursor: Option<bool>,
    selected: bool,
}

fn entry_row(entry: &Entry, row: RowState, colors: &Colors) -> Div {
    // Selected rows are orange throughout; under the active cursor the bar
    // itself turns orange so selection stays visible.
    let (name, detail) = if row.selected {
        (colors.selected, colors.selected)
    } else {
        (name_color(entry, colors), colors.text_dim)
    };
    let (fg, detail, bg) = match row.cursor {
        Some(true) => {
            let (bar, text) = if row.selected {
                (colors.selected, colors.text_on_selected)
            } else {
                (colors.accent, colors.text_on_accent)
            };
            (text, text, Some(bar))
        }
        Some(false) => (name, detail, Some(colors.cursor_inactive_bg)),
        None => (name, detail, None),
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

/// "2 dirs, 5 files, 1.2 MiB", or with a selection
/// "3 of 7 selected, 512 B of 1.2 MiB".
fn footer_text(summary: &Summary) -> String {
    if summary.selected() == 0 {
        format!(
            "{} dirs, {} files, {}",
            summary.dirs,
            summary.files,
            format_size(summary.bytes)
        )
    } else {
        format!(
            "{} of {} selected, {} of {}",
            summary.selected(),
            summary.total(),
            format_size(summary.selected_bytes),
            format_size(summary.bytes)
        )
    }
}

fn name_color(entry: &Entry, colors: &Colors) -> Rgba {
    match entry.kind {
        EntryKind::Parent | EntryKind::Dir if entry.is_symlink => colors.symlink,
        EntryKind::Parent | EntryKind::Dir => colors.directory,
        EntryKind::File => colors.text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_shows_totals_without_selection() {
        let summary = Summary {
            dirs: 2,
            files: 5,
            bytes: 1536,
            ..Summary::default()
        };
        assert_eq!(footer_text(&summary), "2 dirs, 5 files, 1.5 KiB");
    }

    #[test]
    fn footer_shows_selection_counts_and_sizes() {
        let summary = Summary {
            dirs: 2,
            files: 5,
            bytes: 1536,
            selected_dirs: 1,
            selected_files: 2,
            selected_bytes: 512,
        };
        assert_eq!(footer_text(&summary), "3 of 7 selected, 512 B of 1.5 KiB");
    }
}
