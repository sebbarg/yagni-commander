use std::cell::Cell;

use fm_core::theme::{Rgb, TOKYO_NIGHT as P};
use fm_core::{Command, Commander, Entry, EntryKind, Side, format_size, scroll_offset};
use iced::keyboard::{self, Key, key::Named};
use iced::mouse::ScrollDelta;
use iced::widget::text::Wrapping;
use iced::widget::{Column, column, container, mouse_area, pane_grid, responsive, row, text};
use iced::{Color, Element, Length, Size, Subscription, Theme};

const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 24.0;
const FONT_SIZE: f32 = 14.0;
const WHEEL_ROWS_PER_LINE: f32 = 3.0;

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .theme(|_: &App| Theme::TokyoNight)
        .window_size(Size::new(1200.0, 800.0))
        .run()
}

struct App {
    commander: Commander,
    panes: pane_grid::State<Side>,
    /// First visible row per panel. Updated during `view`, because only layout
    /// knows how many rows fit; `Cell` lets the immutable view write it back.
    offsets: [Cell<usize>; 2],
    /// Visible rows per panel from the last layout, used for page up/down.
    visible_rows: [Cell<usize>; 2],
    /// False after the wheel scrolled a panel, so the view stays put instead of
    /// snapping back to the cursor. The next cursor movement sets it again.
    follow_cursor: [bool; 2],
    /// Fractional rows left over from smooth (touchpad) scrolling.
    scroll_remainder: [f32; 2],
}

#[derive(Debug, Clone)]
enum Message {
    Command(Command),
    Page {
        down: bool,
    },
    Click(Side, usize),
    DoubleClick(Side, usize),
    /// Mouse wheel over a panel, in rows (positive scrolls down).
    Scroll(Side, f32),
    Resized(pane_grid::ResizeEvent),
}

impl App {
    fn new() -> Self {
        let mut args = std::env::args().skip(1);
        let left = args.next().unwrap_or_else(|| ".".into());
        let right = args.next().unwrap_or_else(|| left.clone());
        let commander = Commander::new(&left, &right)
            .unwrap_or_else(|e| panic!("cannot open {left} / {right}: {e}"));

        let panes = pane_grid::State::with_configuration(pane_grid::Configuration::Split {
            axis: pane_grid::Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane_grid::Configuration::Pane(Side::Left)),
            b: Box::new(pane_grid::Configuration::Pane(Side::Right)),
        });

        Self {
            commander,
            panes,
            offsets: Default::default(),
            visible_rows: [Cell::new(20), Cell::new(20)],
            follow_cursor: [true; 2],
            scroll_remainder: [0.0; 2],
        }
    }

    fn title(&self) -> String {
        let path = self.commander.panel(self.commander.active()).path();
        format!("{} - fm (iced)", path.display())
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Command(cmd) => {
                // Opening files arrives with F4/editor support.
                let _ = self.commander.execute(cmd);
                if !matches!(cmd, Command::SwitchPanel | Command::Focus(_)) {
                    self.follow_cursor[index(self.commander.active())] = true;
                }
            }
            Message::Page { down } => {
                let rows = self.visible_rows[index(self.commander.active())].get() as isize;
                let delta = (rows - 1).max(1);
                self.commander
                    .execute(Command::CursorBy(if down { delta } else { -delta }));
                self.follow_cursor[index(self.commander.active())] = true;
            }
            Message::Click(side, ix) => {
                self.commander.execute(Command::CursorTo(side, ix));
            }
            Message::DoubleClick(side, ix) => {
                self.commander.execute(Command::CursorTo(side, ix));
                let _ = self.commander.execute(Command::Activate);
                self.follow_cursor[index(side)] = true;
            }
            Message::Scroll(side, rows) => {
                let slot = index(side);
                let total = self.scroll_remainder[slot] + rows;
                let whole = total.trunc();
                self.scroll_remainder[slot] = total - whole;
                // Clamped to the listing end during the next layout.
                let offset = self.offsets[slot]
                    .get()
                    .saturating_add_signed(whole as isize);
                self.offsets[slot].set(offset);
                self.follow_cursor[slot] = false;
            }
            Message::Resized(event) => self.panes.resize(event.split, event.ratio),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().filter_map(key_to_message)
    }

    fn view(&self) -> Element<'_, Message> {
        let grid = pane_grid::PaneGrid::new(&self.panes, |_, side, _| {
            pane_grid::Content::new(self.panel_view(*side))
        })
        .spacing(6)
        .on_resize(8, Message::Resized)
        .style(|_| pane_grid::Style {
            hovered_region: pane_grid::Highlight {
                background: color(P.cursor_inactive_bg).into(),
                border: Default::default(),
            },
            picked_split: pane_grid::Line {
                color: color(P.accent),
                width: 2.0,
            },
            hovered_split: pane_grid::Line {
                color: color(P.accent),
                width: 2.0,
            },
        });

        let status = match self.commander.error() {
            Some(err) => text(err.to_owned()).color(color(P.error)),
            None => {
                text("Tab switch · ↑↓ move · Enter open · Backspace up").color(color(P.text_dim))
            }
        };

        container(column![
            container(grid).padding(6).height(Length::Fill),
            container(status.size(12)).padding([2, 10]),
        ])
        .style(|_| background(P.window_bg))
        .into()
    }

    fn panel_view(&self, side: Side) -> Element<'_, Message> {
        let panel = self.commander.panel(side);
        let is_active = self.commander.active() == side;

        let header = container(
            text(panel.path().display().to_string())
                .size(FONT_SIZE)
                .wrapping(Wrapping::None)
                .color(color(if is_active { P.text } else { P.text_dim })),
        )
        .padding([4, 10])
        .height(HEADER_HEIGHT)
        .width(Length::Fill)
        .clip(true)
        .style(move |_| {
            background(if is_active {
                P.header_active_bg
            } else {
                P.header_bg
            })
        });

        let rows = responsive(move |size| {
            let entries = panel.entries();
            let visible = (size.height / ROW_HEIGHT).floor().max(1.0) as usize;
            let slot = index(side);
            self.visible_rows[slot].set(visible);
            let offset = if self.follow_cursor[slot] {
                scroll_offset(
                    self.offsets[slot].get(),
                    panel.cursor(),
                    visible,
                    entries.len(),
                )
            } else {
                self.offsets[slot]
                    .get()
                    .min(entries.len().saturating_sub(visible))
            };
            self.offsets[slot].set(offset);

            let end = (offset + visible).min(entries.len());
            let mut list = Column::new();
            for (ix, entry) in entries[offset..end]
                .iter()
                .enumerate()
                .map(|(i, e)| (offset + i, e))
            {
                let cursor = if ix == panel.cursor() {
                    Some(is_active)
                } else {
                    None
                };
                list = list.push(
                    mouse_area(entry_row(entry, cursor))
                        .on_press(Message::Click(side, ix))
                        .on_double_click(Message::DoubleClick(side, ix)),
                );
            }
            mouse_area(container(list).width(Length::Fill).height(Length::Fill))
                .on_scroll(move |delta| {
                    let rows = match delta {
                        ScrollDelta::Lines { y, .. } => y * WHEEL_ROWS_PER_LINE,
                        ScrollDelta::Pixels { y, .. } => y / ROW_HEIGHT,
                    };
                    Message::Scroll(side, -rows)
                })
                .into()
        });

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
        let footer = container(
            text(format!("{dirs} dirs, {files} files"))
                .size(12)
                .color(color(P.text_dim)),
        )
        .padding([4, 10])
        .height(FOOTER_HEIGHT)
        .width(Length::Fill)
        .style(|_| background(P.header_bg));

        let border = if is_active { P.accent } else { P.border };
        container(column![header, rows, footer])
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(move |_| container::Style {
                border: iced::Border {
                    color: color(border),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..background(P.panel_bg)
            })
            .into()
    }
}

/// `cursor`: `None` if not under the cursor, `Some(panel_is_active)` otherwise.
fn entry_row<'a>(entry: &'a Entry, cursor: Option<bool>) -> Element<'a, Message> {
    let (fg, bg) = match cursor {
        Some(true) => (P.text_on_accent, Some(P.accent)),
        Some(false) => (name_color(entry), Some(P.cursor_inactive_bg)),
        None => (name_color(entry), None),
    };
    let size_label = match entry.kind {
        EntryKind::Parent => String::new(),
        EntryKind::Dir => "<DIR>".into(),
        EntryKind::File => entry.size.map(format_size).unwrap_or_default(),
    };
    let name = text(&entry.label)
        .size(FONT_SIZE)
        .wrapping(Wrapping::None)
        .color(color(fg));
    let size = text(size_label)
        .size(FONT_SIZE - 1.0)
        .color(color(if cursor == Some(true) { fg } else { P.text_dim }))
        .align_x(iced::alignment::Horizontal::Right);

    container(row![
        container(name).width(Length::Fill).clip(true),
        container(size).width(90).align_right(90),
    ])
    .padding([2, 10])
    .height(ROW_HEIGHT)
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: bg.map(|c| color(c).into()),
        ..Default::default()
    })
    .into()
}

fn name_color(entry: &Entry) -> Rgb {
    match entry.kind {
        EntryKind::Parent | EntryKind::Dir if entry.is_symlink => P.symlink,
        EntryKind::Parent | EntryKind::Dir => P.dir,
        EntryKind::File => P.text,
    }
}

fn key_to_message(event: keyboard::Event) -> Option<Message> {
    let keyboard::Event::KeyPressed {
        key: Key::Named(named),
        ..
    } = event
    else {
        return None;
    };
    let cmd = match named {
        Named::Tab => Command::SwitchPanel,
        Named::ArrowUp => Command::CursorUp,
        Named::ArrowDown => Command::CursorDown,
        Named::Home => Command::CursorHome,
        Named::End => Command::CursorEnd,
        Named::Enter => Command::Activate,
        Named::Backspace => Command::GoUp,
        Named::PageUp => return Some(Message::Page { down: false }),
        Named::PageDown => return Some(Message::Page { down: true }),
        _ => return None,
    };
    Some(Message::Command(cmd))
}

fn index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn color(rgb: Rgb) -> Color {
    let (r, g, b) = rgb.channels();
    Color::from_rgb8(r, g, b)
}

fn background(rgb: Rgb) -> container::Style {
    container::Style {
        background: Some(color(rgb).into()),
        ..Default::default()
    }
}
