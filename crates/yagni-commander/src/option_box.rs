//! An option box: a ticked or empty square and its label; Space (the
//! `FindOption` context's `Toggle`) or a click toggles it. Our own rather
//! than gpui-component's `Checkbox`, whose focus ring is too faint to see and
//! whose focus handle is private. Used by the find dialog and the viewer's
//! Find dialog.

use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    App, Div, FocusHandle, InteractiveElement, MouseButton, MouseDownEvent, ParentElement,
    Stateful, Styled, Window, div, prelude::FluentBuilder, px,
};

use crate::actions::{FIND_OPTION_CONTEXT, find_results::Toggle};
use crate::button_row::focus_ring;
use crate::theme::Theme;

/// An option box with element id and debug selector `id`. `toggle` runs on
/// Space and on a click (which also focuses the box).
pub fn option_box(
    id: &'static str,
    label: &'static str,
    checked: bool,
    focus: &FocusHandle,
    toggle: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &Window,
    cx: &App,
) -> Stateful<Div> {
    let colors = Theme::get(cx).colors.clone();
    let on_key = toggle.clone();
    let click_focus = focus.clone();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .key_context(FIND_OPTION_CONTEXT)
        .track_focus(focus)
        .relative()
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .on_action(move |_: &Toggle, window, cx| on_key(window, cx))
        .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, cx| {
            click_focus.focus(window, cx);
            toggle(window, cx);
        })
        .child(check_square(checked, &colors))
        .child(label)
        .when(focus.is_focused(window), |d| {
            d.child(focus_ring(colors.accent, id))
        })
}

/// A ticked or empty square, as in the option boxes.
pub fn check_square(on: bool, colors: &crate::theme::Colors) -> Div {
    div()
        .size(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(if on { colors.accent } else { colors.text_dim })
        .when(on, |d| {
            d.bg(colors.accent).child(
                Icon::new(IconName::Check)
                    .size(px(12.0))
                    .text_color(colors.text_on_accent),
            )
        })
}
