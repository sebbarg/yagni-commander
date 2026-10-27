//! The row of buttons at the bottom of every dialog.
//!
//! gpui-component's `Dialog` maps Enter to its OK action for the whole
//! dialog, even when another button has keyboard focus, and has no arrow-key
//! navigation. This row owns the keyboard instead: it keeps its own selected
//! button (drawn as the primary one), Left/Right and Tab/Shift-Tab move the
//! selection, Enter and Space press it. Tab past either end leaves the row,
//! e.g. back to a prompt's text field. Escape is left to the dialog (cancel).
//! In a message box, Ctrl-C / Ctrl-Ins (Cmd-C on macOS) copy its text.
//! A label may mark one letter with `&` (`Feed to &listbox`), drawn
//! underlined; the key that goes with it is bound by the dialog.

use std::rc::Rc;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::{
    App, ClipboardItem, Context, FocusHandle, Focusable, HighlightStyle, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, StyledText, UnderlineStyle, Window,
    div, prelude::FluentBuilder, px,
};
use yagni_commander_core::hotlist;

use crate::actions::{
    BUTTON_ROW_CONTEXT, CopyText, NextButton, PressButton, PrevButton, TabNext, TabPrev,
};
use crate::zoom::rems_from_px;

/// What a button does when pressed. It closes the dialog itself if it should.
pub type OnPress = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct ButtonRow {
    focus: FocusHandle,
    buttons: Vec<(SharedString, OnPress)>,
    /// Indices of buttons that are drawn disabled and do nothing.
    disabled: Vec<usize>,
    selected: usize,
    /// What Ctrl-C copies while the row has focus (a message box's text).
    copy_text: Option<SharedString>,
    /// The first this many buttons sit apart at the left (a side action);
    /// the keys still go through all of them in order.
    apart: usize,
    /// The button the row starts on, the default for Enter in a text
    /// field; the selection goes back to it when the row loses focus.
    default: usize,
}

impl ButtonRow {
    /// `buttons` in display order; `selected` is the one Enter presses at first.
    pub fn new(
        buttons: Vec<(SharedString, OnPress)>,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = selected.min(buttons.len().saturating_sub(1));
        Self {
            focus: cx.focus_handle().tab_stop(true),
            selected,
            default: selected,
            buttons,
            disabled: Vec::new(),
            copy_text: None,
            apart: 0,
        }
    }

    /// Builds a row with one callback per label.
    pub fn build<const N: usize>(
        buttons: [(&'static str, OnPress); N],
        selected: usize,
        cx: &mut App,
    ) -> gpui_kit::Entity<Self> {
        use gpui_kit::AppContext;
        let buttons = buttons
            .into_iter()
            .map(|(label, on_press)| (SharedString::from(label), on_press))
            .collect();
        cx.new(|cx| Self::new(buttons, selected, cx))
    }

    /// Focuses the row on its first button, or its last (Tab or Shift-Tab
    /// from a field before it).
    pub fn enter(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.buttons.len().saturating_sub(1);
        self.select(if first { 0 } else { last }, cx);
        self.focus.focus(window, cx);
    }

    /// Draws the first `count` buttons apart, at the left.
    pub fn set_apart(&mut self, count: usize) {
        self.apart = count;
    }

    /// Ctrl-C / Ctrl-Ins (Cmd-C on macOS) copy `text` while the row has
    /// focus. Without it the keys pass on.
    pub fn set_copy_text(&mut self, text: impl Into<SharedString>) {
        self.copy_text = Some(text.into());
    }

    /// Renames the button at `index` (Search becomes Stop).
    pub fn set_label(&mut self, index: usize, label: impl Into<SharedString>) {
        if let Some(button) = self.buttons.get_mut(index) {
            button.0 = label.into();
        }
    }

    /// A disabled button is drawn so and does nothing when pressed.
    pub fn set_enabled(&mut self, index: usize, enabled: bool) {
        self.disabled.retain(|&ix| ix != index);
        if !enabled {
            self.disabled.push(index);
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        cx.notify();
    }

    /// Presses the button at `index`. The callback runs after this update so
    /// it may close the dialog (which re-renders this row).
    fn press(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select(index, cx);
        if self.disabled.contains(&index) {
            return;
        }
        let on_press = self.buttons[index].1.clone();
        window.defer(cx, move |window, cx| on_press(window, cx));
    }
}

/// The ring drawn around the control that has keyboard focus: the same
/// accent band for buttons, the find dialog's option boxes and lists, so
/// one look says "the keys go here". Its parent must be `relative()`.
pub fn focus_ring(accent: gpui_kit::Rgba, what: &'static str) -> gpui_kit::Div {
    div()
        .debug_selector(move || format!("focus-ring-{what}"))
        .absolute()
        .top(rems_from_px(-3.0))
        .left(rems_from_px(-3.0))
        .right(rems_from_px(-3.0))
        .bottom(rems_from_px(-3.0))
        .border_2()
        .border_color(accent)
        .rounded(rems_from_px(8.0))
}

impl Focusable for ButtonRow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `text` with the byte range of its marked letter underlined.
fn underlined(text: SharedString, range: std::ops::Range<usize>) -> StyledText {
    let underline = HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.0),
            ..Default::default()
        }),
        ..Default::default()
    };
    StyledText::new(text).with_highlights([(range, underline)])
}

impl Render for ButtonRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.buttons.len().saturating_sub(1);
        // The selected button is the default (Enter in a text field); the
        // ring says the row itself has the keys.
        let focused = self.focus.is_focused(window);
        // Out of the row, the highlight shows the default again: Enter in a
        // text field presses that one, whatever was selected in the row.
        if !focused {
            self.selected = self.default;
        }
        let accent = crate::theme::Theme::get(cx).colors.accent;
        div()
            .key_context(BUTTON_ROW_CONTEXT)
            .track_focus(&self.focus)
            .flex()
            .justify_end()
            .gap(rems_from_px(8.0))
            .on_action(cx.listener(move |this, _: &PrevButton, _, cx| {
                this.select(this.selected.saturating_sub(1), cx)
            }))
            .on_action(cx.listener(move |this, _: &NextButton, _, cx| {
                this.select((this.selected + 1).min(last), cx)
            }))
            .on_action(cx.listener(move |this, _: &TabNext, window, cx| {
                if this.selected < last {
                    this.select(this.selected + 1, cx);
                } else {
                    window.focus_next(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TabPrev, window, cx| {
                if this.selected > 0 {
                    this.select(this.selected - 1, cx);
                } else {
                    window.focus_prev(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PressButton, window, cx| {
                this.press(this.selected, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CopyText, _, cx| match &this.copy_text {
                    Some(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
                    }
                    None => cx.propagate(),
                }),
            )
            .children(
                self.buttons
                    .iter()
                    .enumerate()
                    .flat_map(|(ix, (label, _))| {
                        // The space that pushes the rest to the right.
                        let gap = (self.apart > 0 && ix == self.apart).then(|| div().flex_1());
                        let label = hotlist::label(label);
                        let text = SharedString::from(label.text);
                        let selector = format!("button-{text}");
                        let button = div()
                            .relative()
                            .debug_selector(move || selector)
                            .when(focused && ix == self.selected, |d| {
                                d.child(focus_ring(accent, "button"))
                            })
                            .child(
                                Button::new(("button", ix))
                                    .map(|b| match label.underline {
                                        Some(range) => b
                                            .accessibility_label(text.clone())
                                            .child(underlined(text, range)),
                                        None => b.label(text),
                                    })
                                    .tab_stop(false)
                                    .disabled(self.disabled.contains(&ix))
                                    .when(ix == self.selected, |b| b.primary())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.press(ix, window, cx)
                                    })),
                            );
                        gap.into_iter().chain([button])
                    }),
            )
    }
}
