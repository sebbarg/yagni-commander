//! The directory hotlist popup (Ctrl-D): the entries, then "Add current
//! folder" and "Configure...". Our own view rather than gpui-component's
//! `PopupMenu`, which has no letter keys. Letters pick an entry; Up/Down
//! wrap; Enter picks; Escape closes. `FileManager` acts on the events and
//! keeps other keys away while it is open (see [`passes`]).

use gpui_kit::{
    Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, InteractiveElement, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, ParentElement, Render, Styled, StyledText,
    UnderlineStyle, Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::hotlist::{self, HotlistEntry};

use crate::actions::{HOTLIST_CONTEXT, hotlist as act};
use crate::theme::Theme;
use crate::zoom::rems_from_px;

pub enum HotlistEvent {
    /// This entry's folder, as stored (the popup's own copy of the list).
    Pick(String),
    Add,
    Configure,
    Dismiss,
}

pub struct HotlistPopup {
    entries: Vec<HotlistEntry>,
    /// Row under the highlight: entries, then Add, then Configure.
    highlight: usize,
    focus: FocusHandle,
}

impl EventEmitter<HotlistEvent> for HotlistPopup {}

impl Focusable for HotlistPopup {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Keys the open popup acts on: Up/Down/Enter/Escape, and characters typed
/// without Ctrl, Alt or Cmd (letters). Everything else is ignored.
pub fn passes(keystroke: &Keystroke) -> bool {
    let m = keystroke.modifiers;
    if m.control || m.alt || m.platform || m.function {
        return false;
    }
    matches!(keystroke.key.as_str(), "up" | "down" | "enter" | "escape")
        || keystroke.key.chars().count() == 1
}

impl HotlistPopup {
    pub fn new(entries: Vec<HotlistEntry>, cx: &mut Context<Self>) -> Self {
        Self {
            entries,
            highlight: 0,
            focus: cx.focus_handle(),
        }
    }

    #[cfg(test)]
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    fn rows(&self) -> usize {
        self.entries.len() + 2
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.rows() as isize;
        self.highlight = (self.highlight as isize + by).rem_euclid(rows) as usize;
        cx.notify();
    }

    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        let count = self.entries.len();
        cx.emit(match row {
            r if r < count => HotlistEvent::Pick(self.entries[r].path.clone()),
            r if r == count => HotlistEvent::Add,
            _ => HotlistEvent::Configure,
        });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Never let a letter reach the panel's quick search.
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        if !passes(keystroke) {
            return;
        }
        let text = keystroke.key_char.as_deref().unwrap_or(&keystroke.key);
        let mut chars = text.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && let Some(ix) = hotlist::find_letter(&self.entries, ch)
        {
            cx.emit(HotlistEvent::Pick(self.entries[ix].path.clone()));
        }
    }
}

impl Render for HotlistPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let count = self.entries.len();
        let row = |ix: usize, highlighted: bool| {
            div()
                .id(("hotlist-row", ix))
                .debug_selector(move || format!("hotlist-row-{ix}"))
                // Sized like gpui-component's menu items, so the popup
                // reads like the menus.
                .h(rems_from_px(26.0))
                .flex()
                .items_center()
                .px(rems_from_px(10.0))
                .when(highlighted, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.pick(ix, cx);
                    }),
                )
        };
        let entries = self.entries.iter().enumerate().map(|(ix, entry)| {
            let label = hotlist::display_label(entry);
            let underline = HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let text = StyledText::new(label.text)
                .with_highlights(label.underline.map(|range| (range, underline)));
            let highlighted = ix == self.highlight;
            // Names only; Configure shows the paths.
            row(ix, highlighted).child(div().flex_1().child(text))
        });
        div()
            .key_context(HOTLIST_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::Pick, _, cx| this.pick(this.highlight, cx)))
            .on_action(cx.listener(|_, _: &act::Close, _, cx| cx.emit(HotlistEvent::Dismiss)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(HotlistEvent::Dismiss)))
            .debug_selector(|| "hotlist-popup".into())
            .occlude()
            .min_w(rems_from_px(260.0))
            .py(rems_from_px(4.0))
            .flex()
            .flex_col()
            .bg(colors.header_bg)
            .border_1()
            .border_color(colors.border)
            .rounded(rems_from_px(4.0))
            .text_sm()
            .text_color(colors.text)
            .children(entries)
            .when(count > 0, |d| {
                d.child(div().my(rems_from_px(4.0)).h(px(1.0)).bg(colors.border))
            })
            .child(row(count, self.highlight == count).child("Add current folder"))
            .child(row(count + 1, self.highlight == count + 1).child("Configure..."))
    }
}
