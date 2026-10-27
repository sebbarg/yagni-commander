//! The Open with popup (Shift-Enter): an "Open with" title, then the
//! applications for the file under the cursor (core [`apps`]), the default
//! first. Like the mounts dropdown, our own view: Up/Down wrap, Enter or a
//! click starts the highlighted one, Escape closes, and a letter moves to
//! the next application starting with it, or starts it when it is the only
//! one.

use gpui_kit::{
    Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, ScrollHandle, StatefulInteractiveElement,
    Styled, StyledText, UnderlineStyle, Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::apps::App;
use yagni_commander_core::mounts::next_label;

use crate::actions::{OPEN_WITH_CONTEXT, open_with as act};
use crate::hotlist_popup::passes;
use crate::theme::{Theme, popup_frame};
use crate::zoom::rems_from_px;

pub enum OpenWithEvent {
    Pick(App),
    Dismiss,
}

/// Rows shown before the list scrolls.
const MAX_ROWS: f32 = 16.0;
const ROW_HEIGHT: f32 = 26.0;

pub struct OpenWithPopup {
    apps: Vec<App>,
    highlight: usize,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl EventEmitter<OpenWithEvent> for OpenWithPopup {}

impl Focusable for OpenWithPopup {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl OpenWithPopup {
    /// `apps` in display order; the highlight starts on the first.
    pub fn new(apps: Vec<App>, cx: &mut Context<Self>) -> Self {
        Self {
            apps,
            highlight: 0,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        }
    }

    #[cfg(test)]
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.apps.len() as isize;
        self.highlight = (self.highlight as isize + by).rem_euclid(rows) as usize;
        self.scroll.scroll_to_item(self.highlight);
        cx.notify();
    }

    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(app) = self.apps.get(row) {
            cx.emit(OpenWithEvent::Pick(app.clone()));
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
        let names: Vec<&str> = self.apps.iter().map(|app| app.name.as_str()).collect();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && let Some((ix, unique)) = next_label(&names, self.highlight, ch)
        {
            self.highlight = ix;
            self.scroll.scroll_to_item(ix);
            if unique {
                self.pick(ix, cx);
            }
            cx.notify();
        }
    }
}

impl Render for OpenWithPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let colors = theme.colors.clone();
        let underline = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = self.apps.iter().enumerate().map(|(ix, app)| {
            let highlighted = ix == self.highlight;
            let first = app.name.chars().next().map_or(0, char::len_utf8);
            let label = StyledText::new(app.name.clone()).with_highlights([(0..first, underline)]);
            div()
                .id(("open-with-row", ix))
                .debug_selector(move || format!("open-with-row-{ix}"))
                // Sized like the other popups' rows (gpui-component's menu items).
                .h(rems_from_px(ROW_HEIGHT))
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
                .child(div().flex_1().whitespace_nowrap().child(label))
                .when(app.default, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .when(!highlighted, |d| d.text_color(colors.text_secondary))
                            .child("(default)"),
                    )
                })
        });
        div()
            .key_context(OPEN_WITH_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::Pick, _, cx| this.pick(this.highlight, cx)))
            .on_action(cx.listener(|_, _: &act::Close, _, cx| cx.emit(OpenWithEvent::Dismiss)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(OpenWithEvent::Dismiss)))
            .debug_selector(|| "open-with-popup".into())
            .occlude()
            .min_w(rems_from_px(220.0))
            .py(rems_from_px(4.0))
            .map(|d| popup_frame(d, &theme))
            .text_sm()
            .text_color(colors.text)
            .child(
                div()
                    .debug_selector(|| "open-with-title".into())
                    .h(rems_from_px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .px(rems_from_px(10.0))
                    .mb(rems_from_px(4.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .text_color(colors.text_secondary)
                    .child("Open with"),
            )
            .child(
                div()
                    .id("open-with-list")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .max_h(rems_from_px(ROW_HEIGHT * MAX_ROWS))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
    }
}
