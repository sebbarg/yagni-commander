//! A dropdown for dialogs: a closed control showing the chosen item, and a
//! list under it while open. Our own, not gpui-component's `Select`: that
//! one draws its list with a fixed faint edge (it vanished on dialogs drawn
//! in the popups' color), and on a closed `Select` Enter reached the
//! dialog's OK.
//!
//! Enter, Space or Alt-Down opens the list on the chosen item; Up/Down,
//! Home/End move; Enter, Space or a click picks; Escape, Alt-Up, Tab or a
//! click elsewhere closes. A pick of another item emits
//! [`DropdownEvent::Picked`].

use gpui_kit::component::{Icon, IconName};
use gpui_kit::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Render, SharedString, Styled, Window, anchored,
    deferred, div, prelude::FluentBuilder,
};

use crate::actions::{DROPDOWN_CONTEXT, DROPDOWN_LIST_CONTEXT, dropdown as act};
use crate::button_row::focus_ring;
use crate::theme::{Theme, field_frame, popup_frame};
use crate::zoom::rems_from_px;

/// The closed control's height, a text field's.
const HEIGHT: f32 = 30.0;
/// A row of the list, like the menus' and our popups' rows.
const ROW_HEIGHT: f32 = 26.0;

pub enum DropdownEvent {
    /// The item at this index was picked (and differs from the one before).
    Picked(usize),
}

pub struct Dropdown {
    /// Names `dropdown-<id>` (the control), `dropdown-<id>-list` and
    /// `dropdown-<id>-row-<ix>` for tests.
    id: &'static str,
    items: Vec<SharedString>,
    selected: usize,
    /// The highlighted row while the list is open.
    cursor: usize,
    open: bool,
    disabled: bool,
    width: f32,
    focus: FocusHandle,
    list_focus: FocusHandle,
}

impl EventEmitter<DropdownEvent> for Dropdown {}

impl Focusable for Dropdown {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Dropdown {
    /// `width` in px at the default zoom; the list is at least as wide.
    pub fn new(
        id: &'static str,
        items: Vec<SharedString>,
        selected: usize,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id,
            items,
            selected,
            cursor: selected,
            open: false,
            disabled: false,
            width,
            focus: cx.focus_handle().tab_stop(true),
            list_focus: cx.focus_handle(),
        }
    }

    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.items.is_empty() {
            return;
        }
        self.open = true;
        self.cursor = self.selected;
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    /// Closes the list and gives focus back to the control.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.items.len() as isize - 1;
        self.cursor = (self.cursor as isize + by).clamp(0, last) as usize;
        cx.notify();
    }

    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        if ix != self.selected {
            self.selected = ix;
            cx.emit(DropdownEvent::Picked(ix));
        }
    }

    /// A click elsewhere closes the list. Focus goes back to the control
    /// only if the click didn't take it (a text field, a button).
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        cx.notify();
        let (list, focus) = (self.list_focus.clone(), self.focus.clone());
        window.defer(cx, move |window, cx| {
            if list.is_focused(window) {
                focus.focus(window, cx);
            }
        });
    }

    fn list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::get(cx).clone();
        let colors = theme.colors.clone();
        let id = self.id;
        let rows = self.items.iter().enumerate().map(|(ix, item)| {
            let at_cursor = ix == self.cursor;
            div()
                .id(("dropdown-row", ix))
                .debug_selector(move || format!("dropdown-{id}-row-{ix}"))
                .h(rems_from_px(ROW_HEIGHT))
                .flex()
                .items_center()
                .justify_between()
                .gap(rems_from_px(8.0))
                .px(rems_from_px(10.0))
                .when(at_cursor, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.pick(ix, window, cx);
                    }),
                )
                .child(div().whitespace_nowrap().child(item.clone()))
                .when(ix == self.selected, |d| {
                    d.child(
                        Icon::new(IconName::Check)
                            .size(rems_from_px(14.0))
                            .text_color(if at_cursor {
                                colors.text_on_accent
                            } else {
                                colors.text
                            }),
                    )
                })
        });
        deferred(
            anchored().snap_to_window().child(
                div()
                    .id("dropdown-list")
                    .debug_selector(move || format!("dropdown-{id}-list"))
                    .key_context(DROPDOWN_LIST_CONTEXT)
                    .track_focus(&self.list_focus)
                    .occlude()
                    // Anchored at the control's top: open below it.
                    .mt(rems_from_px(HEIGHT + 4.0))
                    .min_w(rems_from_px(self.width))
                    .py(rems_from_px(4.0))
                    .flex()
                    .flex_col()
                    .map(|d| popup_frame(d, &theme))
                    .text_color(colors.text)
                    .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
                    .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
                    .on_action(cx.listener(|this, _: &act::First, _, cx| {
                        this.step(-(this.items.len() as isize), cx)
                    }))
                    .on_action(cx.listener(|this, _: &act::Last, _, cx| {
                        this.step(this.items.len() as isize, cx)
                    }))
                    .on_action(cx.listener(|this, _: &act::Pick, window, cx| {
                        this.pick(this.cursor, window, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &act::Close, window, cx| this.close(window, cx)),
                    )
                    .on_mouse_down_out(
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.dismiss(window, cx)
                        }),
                    )
                    .children(rows),
            ),
        )
        // Above the dialog layer (priority 10 and up), like gpui-base's
        // own popups.
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
    }
}

impl Render for Dropdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let colors = theme.colors.clone();
        let id = self.id;
        let focused = self.focus.is_focused(window);
        let text = if self.disabled {
            colors.text_dim
        } else {
            colors.text
        };
        let control = div()
            .id(id)
            .debug_selector(move || format!("dropdown-{id}"))
            .key_context(DROPDOWN_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .w(rems_from_px(self.width))
            .h(rems_from_px(HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .gap(rems_from_px(8.0))
            .px(rems_from_px(10.0))
            .map(|d| field_frame(d, &theme))
            .text_color(text)
            .when(!self.disabled, |d| d.cursor_pointer())
            .on_action(cx.listener(|this, _: &act::Open, window, cx| this.open(window, cx)))
            // In the capture phase, stopping there: a click on the control
            // while the list is open closes it, where the list's
            // click-outside handler would close it and this one reopen it.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if event.button != MouseButton::Left {
                    return;
                }
                cx.stop_propagation();
                if this.open {
                    this.close(window, cx);
                } else {
                    // Focus first, so a disabled dropdown is still
                    // where Tab goes on from.
                    this.focus.focus(window, cx);
                    this.open(window, cx);
                }
            }))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .children(self.items.get(self.selected).cloned()),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(rems_from_px(14.0))
                    .text_color(colors.text_dim),
            )
            .when(focused, |d| d.child(focus_ring(colors.accent, "dropdown")));
        let list = self.open.then(|| self.list(cx));
        div().flex().flex_col().child(control).children(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext, TestAppContext};

    fn names() -> Vec<SharedString> {
        ["One", "Two", "Three"].map(SharedString::from).to_vec()
    }

    #[gpui_kit::test]
    fn steps_stay_within_the_items(cx: &mut TestAppContext) {
        let dropdown = cx.new(|cx| Dropdown::new("t", names(), 1, 200.0, cx));
        dropdown.update(cx, |d, cx| {
            d.step(5, cx);
            assert_eq!(d.cursor, 2);
            d.step(-9, cx);
            assert_eq!(d.cursor, 0);
        });
    }
}
