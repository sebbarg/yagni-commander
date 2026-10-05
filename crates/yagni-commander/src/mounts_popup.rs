//! The mounts dropdown (Alt-F1/Alt-F2, or the button at the right end of a
//! path header): Root, Home and the mounted filesystems (core
//! [`mounts::places`]). Like the hotlist popup, our own view: Up/Down wrap,
//! Enter or a click picks, Escape closes, and a letter moves to the next
//! entry starting with it, or goes there when it is the only one.

use std::path::PathBuf;

use gpui_kit::{
    Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, Styled, StyledText, UnderlineStyle, Window,
    div, prelude::FluentBuilder, px,
};
use yagni_commander_core::mounts::{self, Place};

use crate::actions::{MOUNTS_CONTEXT, mounts as act};
use crate::hotlist_popup::passes;
use crate::theme::Theme;
use crate::zoom::rems_from_px;

pub enum MountsEvent {
    Pick(PathBuf),
    Dismiss,
}

pub struct MountsPopup {
    places: Vec<Place>,
    highlight: usize,
    focus: FocusHandle,
}

impl EventEmitter<MountsEvent> for MountsPopup {}

impl Focusable for MountsPopup {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MountsPopup {
    /// The highlight starts on `highlight`, the entry holding the panel's
    /// folder.
    pub fn new(places: Vec<Place>, highlight: usize, cx: &mut Context<Self>) -> Self {
        Self {
            places,
            highlight,
            focus: cx.focus_handle(),
        }
    }

    #[cfg(test)]
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.places.len() as isize;
        self.highlight = (self.highlight as isize + by).rem_euclid(rows) as usize;
        cx.notify();
    }

    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(place) = self.places.get(row) {
            cx.emit(MountsEvent::Pick(place.path.clone()));
        }
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
            && let Some((ix, unique)) = mounts::next_with_letter(&self.places, self.highlight, ch)
        {
            self.highlight = ix;
            if unique {
                self.pick(ix, cx);
            }
            cx.notify();
        }
    }
}

impl Render for MountsPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let underline = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = self.places.iter().enumerate().map(|(ix, place)| {
            let highlighted = ix == self.highlight;
            let first = place.label.chars().next().map_or(0, char::len_utf8);
            let label =
                StyledText::new(place.label.clone()).with_highlights([(0..first, underline)]);
            div()
                .id(("mounts-row", ix))
                .debug_selector(move || format!("mounts-row-{ix}"))
                // Sized like the hotlist popup's rows (gpui-component's menu items).
                .h(rems_from_px(26.0))
                .flex()
                .items_center()
                .gap(rems_from_px(16.0))
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
                .child(div().flex_none().child(label))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .justify_end()
                        .whitespace_nowrap()
                        .when(!highlighted, |d| d.text_color(colors.text_secondary))
                        .child(place.path.display().to_string()),
                )
        });
        div()
            .key_context(MOUNTS_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::Pick, _, cx| this.pick(this.highlight, cx)))
            .on_action(cx.listener(|_, _: &act::Close, _, cx| cx.emit(MountsEvent::Dismiss)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(MountsEvent::Dismiss)))
            .debug_selector(|| "mounts-popup".into())
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
            .children(rows)
    }
}
