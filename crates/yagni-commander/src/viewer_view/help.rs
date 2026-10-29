//! The viewer's key overlay (`?`, or the button at the right of the status
//! line): the keys as data, drawn over the text. It takes no focus, so the
//! viewer's keys keep working while it shows; Escape, `?` or a click closes
//! it.

use gpui_kit::{Context, div, prelude::*};

use super::{STATUS_HEIGHT, ViewerView};
use crate::theme::{Theme, popup_frame};
use crate::zoom::rems_from_px;

/// When a key does something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Needs {
    Always,
    /// Rows scrolled sideways: no wrap or hex.
    Sideways,
    /// The text view (`W`): neither hex nor Markdown.
    Text,
    /// Another mode than text (`T`).
    NotText,
    /// Another mode than hex (`H`).
    NotHex,
    /// Another mode than Markdown (`M`).
    NotMarkdown,
    /// The file itself, not an archive entry's private copy (F4).
    File,
}

/// The viewer's state, as far as the keys care.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Modes {
    pub hex: bool,
    pub markdown: bool,
    pub sideways: bool,
    pub archive: bool,
}

impl Needs {
    pub(super) fn met(self, m: Modes) -> bool {
        match self {
            Needs::Always => true,
            Needs::Sideways => m.sideways && !m.markdown,
            Needs::Text => !m.hex && !m.markdown,
            Needs::NotText => m.hex || m.markdown,
            Needs::NotHex => !m.hex || m.markdown,
            Needs::NotMarkdown => !m.markdown,
            Needs::File => !m.archive,
        }
    }
}

pub(super) struct Entry {
    /// gpui keystrokes, as bound in `actions::bind_default_keys`.
    pub keys: &'static [&'static str],
    pub what: &'static str,
    pub needs: Needs,
}

const fn entry(keys: &'static [&'static str], what: &'static str, needs: Needs) -> Entry {
    Entry { keys, what, needs }
}

/// The overlay's two columns of groups: (title, entries).
pub(super) const COLUMNS: [&[(&str, &[Entry])]; 2] = [
    &[
        (
            "Move",
            &[
                entry(&["up", "down"], "One row", Needs::Always),
                entry(&["pageup", "pagedown"], "One screen", Needs::Always),
                entry(&["home", "end"], "Start, end of file", Needs::Always),
                entry(&["left", "right"], "Scroll sideways", Needs::Sideways),
            ],
        ),
        (
            "View",
            &[
                entry(&["t"], "Text", Needs::NotText),
                entry(&["h"], "Hex", Needs::NotHex),
                entry(&["m"], "Markdown", Needs::NotMarkdown),
                entry(&["w"], "Wrap on, off", Needs::Text),
                entry(&["ctrl-=", "ctrl--"], "Zoom in, out", Needs::Always),
                entry(&["ctrl-0"], "Actual size", Needs::Always),
            ],
        ),
    ],
    &[
        (
            "Search",
            &[
                entry(&["ctrl-f"], "Find", Needs::Always),
                entry(&["f3", "enter"], "Find next", Needs::Always),
                entry(&["shift-f3"], "Find previous", Needs::Always),
            ],
        ),
        (
            "Selection",
            &[
                entry(&["ctrl-a"], "Select all", Needs::Always),
                entry(&["ctrl-c", "ctrl-insert"], "Copy", Needs::Always),
            ],
        ),
        (
            "Other",
            &[
                entry(&["f4"], "Edit", Needs::File),
                entry(&["?"], "These keys", Needs::Always),
                entry(&["escape", "q"], "Close", Needs::Always),
            ],
        ),
    ],
];

/// A gpui keystroke as the menus write it: `ctrl-f` is "Ctrl-F".
pub(super) fn key_label(key: &str) -> String {
    let mut rest = key;
    let mut label = String::new();
    for (prefix, name) in [
        ("ctrl-", "Ctrl-"),
        ("alt-", "Alt-"),
        ("shift-", "Shift-"),
        ("cmd-", "Cmd-"),
    ] {
        if let Some(r) = rest.strip_prefix(prefix).filter(|r| !r.is_empty()) {
            label.push_str(name);
            rest = r;
        }
    }
    let name = match rest {
        "pageup" => "PageUp".to_owned(),
        "pagedown" => "PageDown".to_owned(),
        "escape" => "Esc".to_owned(),
        "insert" => "Ins".to_owned(),
        _ if rest.chars().count() == 1 => rest.to_uppercase(),
        _ => {
            let mut chars = rest.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    label.push_str(&name);
    label
}

/// "Up, Down": an entry's keys.
fn keys_text(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| key_label(k))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The keys column's width, at the 16 px base.
const KEYS_WIDTH: f32 = 140.0;
const DESCRIPTION_WIDTH: f32 = 150.0;
const ROW_HEIGHT: f32 = 20.0;

impl ViewerView {
    pub(super) fn toggle_help(&mut self, cx: &mut Context<Self>) {
        self.help = !self.help;
        cx.notify();
    }

    fn modes(&self) -> Modes {
        Modes {
            hex: self.hex,
            markdown: self.markdown.is_some(),
            sideways: !self.wraps_rows(),
            archive: self.temp.is_some(),
        }
    }

    /// The `?` button at the right of the status line.
    pub(super) fn help_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let (bg, fg) = if self.help {
            (colors.accent, colors.text_on_accent)
        } else {
            (colors.header_bg, colors.text_secondary)
        };
        div()
            .id("viewer-help-button")
            .debug_selector(|| "viewer-help-button".into())
            .flex_none()
            .ml(rems_from_px(8.0))
            .px(rems_from_px(6.0))
            .h(rems_from_px(16.0))
            .flex()
            .items_center()
            .rounded(rems_from_px(3.0))
            .border_1()
            .border_color(colors.border)
            .bg(bg)
            .text_color(fg)
            .cursor_pointer()
            .child("?")
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_help(cx);
                }),
            )
    }

    /// The overlay over the text (not the status line, so the button stays
    /// in reach); a click anywhere on it closes it.
    pub(super) fn render_help(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let colors = &theme.colors;
        let modes = self.modes();
        let columns = COLUMNS.iter().map(|groups| {
            div()
                .flex()
                .flex_col()
                .gap(rems_from_px(12.0))
                .children(groups.iter().map(|(title, entries)| {
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(rems_from_px(ROW_HEIGHT))
                                .text_color(colors.text_secondary)
                                .child(*title),
                        )
                        .children(entries.iter().map(|e| {
                            let (key_color, what_color) = if e.needs.met(modes) {
                                (colors.accent, colors.text)
                            } else {
                                (colors.text_dim, colors.text_dim)
                            };
                            div()
                                .flex()
                                .h(rems_from_px(ROW_HEIGHT))
                                .items_center()
                                .child(
                                    div()
                                        .w(rems_from_px(KEYS_WIDTH))
                                        .text_color(key_color)
                                        .child(keys_text(e.keys)),
                                )
                                .child(
                                    div()
                                        .w(rems_from_px(DESCRIPTION_WIDTH))
                                        .text_color(what_color)
                                        .child(e.what),
                                )
                        }))
                }))
        });
        let card = popup_frame(div(), theme)
            .debug_selector(|| "viewer-help".into())
            .p(rems_from_px(16.0))
            .flex()
            .flex_col()
            .gap(rems_from_px(12.0))
            .text_size(rems_from_px(13.0))
            .child(div().flex().gap(rems_from_px(24.0)).children(columns))
            .child(
                div()
                    .text_color(colors.text_secondary)
                    .child("? or Esc closes. The keys work while this shows."),
            );
        div()
            .id("viewer-help-backdrop")
            .debug_selector(|| "viewer-help-backdrop".into())
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom(rems_from_px(STATUS_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .occlude()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_help(cx)),
            )
            .child(card)
    }

    #[cfg(test)]
    pub(crate) fn shows_help(&self) -> bool {
        self.help
    }
}
