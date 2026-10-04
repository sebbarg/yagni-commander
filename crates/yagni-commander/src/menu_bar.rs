//! The in-window menu bar (Linux; macOS has the native one). A row of menu
//! titles; the open one shows a gpui-component `PopupMenu` built by
//! [`crate::menus::popup`]. gpui-component's own `AppMenuBar` can't be
//! opened from the keyboard (its open and select methods are private),
//! hence this view.
//!
//! In an open menu the popup handles Up/Down/Enter/Escape; Left and Right
//! reach this view when the popup has no submenu to enter.

use gpui_kit::base::actions::{SelectLeft, SelectRight};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::{
    Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Window, anchored, deferred, div, prelude::FluentBuilder, px,
};

use crate::menus::{MenuDef, popup};
use crate::theme::Theme;

const HEIGHT: f32 = 24.0;

pub struct MenuBar {
    menus: Vec<MenuDef>,
    /// Where items dispatch and where focus returns: the file manager.
    action_context: FocusHandle,
    open: Option<OpenMenu>,
}

/// A menu opened, by key or mouse: the file manager ends its quick search.
pub struct Opened;

impl EventEmitter<Opened> for MenuBar {}

struct OpenMenu {
    index: usize,
    popup: Entity<PopupMenu>,
    _dismissed: Subscription,
    _blurred: Subscription,
}

impl MenuBar {
    pub fn new(menus: Vec<MenuDef>, action_context: FocusHandle) -> Self {
        Self {
            menus,
            action_context,
            open: None,
        }
    }

    /// New check marks show the next time a menu opens.
    pub fn set_menus(&mut self, menus: Vec<MenuDef>, cx: &mut Context<Self>) {
        self.menus = menus;
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open_index(&self) -> Option<usize> {
        self.open.as_ref().map(|open| open.index)
    }

    /// F10 or a lone Alt: opens the first menu, or closes the open one.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_open() {
            self.close(window, cx);
        } else {
            self.open(0, window, cx);
        }
    }

    /// Alt-F: opens the first menu (Files), unless it is already open.
    pub fn open_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_index() != Some(0) {
            self.open(0, window, cx);
        }
    }

    fn open(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(def) = self.menus.get(index) else {
            return;
        };
        let popup = popup(&def.entries, self.action_context.clone(), window, cx);
        // The popup closes itself after an item or Escape, and has already
        // moved focus back (or into a dialog the item opened).
        let dismissed = cx.subscribe_in(&popup, window, |this, _, _: &DismissEvent, _, cx| {
            this.open = None;
            cx.notify();
        });
        // Focus leaving the menu (e.g. a dialog opening on top) closes it,
        // so it can't keep swallowing keys.
        let popup_focus = popup.focus_handle(cx);
        let blurred = cx.on_focus_out(&popup_focus, window, |this, _, _, cx| {
            this.open = None;
            cx.notify();
        });
        popup_focus.focus(window, cx);
        self.open = Some(OpenMenu {
            index,
            popup,
            _dismissed: dismissed,
            _blurred: blurred,
        });
        cx.emit(Opened);
        cx.notify();
    }

    /// Closes the open menu and gives focus back to the file manager.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            self.action_context.focus(window, cx);
            cx.notify();
        }
    }

    fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.open_index() else {
            return;
        };
        let count = self.menus.len() as isize;
        let next = (index as isize + by).rem_euclid(count) as usize;
        self.open(next, window, cx);
    }
}

/// Keys an open menu acts on. Every other key is ignored while a menu is
/// open, so it can't act on the panel behind it.
pub fn passes(keystroke: &gpui_kit::Keystroke) -> bool {
    let alt_only = gpui_kit::Modifiers {
        alt: true,
        ..Default::default()
    };
    if keystroke.modifiers == alt_only && keystroke.key == "f" {
        return true;
    }
    !keystroke.modifiers.modified()
        && matches!(
            keystroke.key.as_str(),
            "up" | "down" | "left" | "right" | "enter" | "escape" | "f10" | "alt"
        )
}

impl Render for MenuBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let open = self.open_index();
        let titles = self.menus.iter().enumerate().map(|(index, def)| {
            let is_open = open == Some(index);
            let title = def.title;
            let popup = self
                .open
                .as_ref()
                .filter(|open| open.index == index)
                .map(|open| open.popup.clone());
            div()
                .id(index)
                .debug_selector(move || format!("menu-{title}"))
                .relative()
                .h_full()
                .px(px(8.0))
                .flex()
                .items_center()
                .when(is_open, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .when(!is_open, |d| d.hover(|d| d.bg(colors.header_active_bg)))
                .child(title)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if this.open_index() == Some(index) {
                            this.close(window, cx);
                        } else {
                            this.open(index, window, cx);
                        }
                    }),
                )
                // Like native menu bars: while a menu is open, hovering
                // another title opens that one.
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered && this.is_open() && this.open_index() != Some(index) {
                        this.open(index, window, cx);
                    }
                }))
                .when_some(popup, |d, popup| {
                    d.child(deferred(
                        anchored().snap_to_window_with_margin(px(8.0)).child(
                            div()
                                .debug_selector(|| "menu-popup".into())
                                .occlude()
                                .mt(px(HEIGHT))
                                .child(popup),
                        ),
                    ))
                })
        });
        div()
            .id("menu-bar")
            .key_context("MenuBar")
            .on_action(cx.listener(|this, _: &SelectLeft, window, cx| this.step(-1, window, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, window, cx| this.step(1, window, cx)))
            .h(px(HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(4.0))
            .bg(colors.header_bg)
            .text_color(colors.text)
            .text_size(px(13.0))
            .children(titles)
    }
}

#[cfg(test)]
mod tests {
    use super::passes;
    use gpui_kit::Keystroke;

    fn key(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn only_menu_keys_pass_while_open() {
        for k in [
            "up", "down", "left", "right", "enter", "escape", "f10", "alt", "alt-f",
        ] {
            assert!(passes(&key(k)), "{k}");
        }
        for k in [
            "f5",
            "a",
            "tab",
            "space",
            "ctrl-a",
            "shift-f4",
            "alt-z",
            "alt-shift-f",
            "ctrl-alt-f",
            "shift-down",
        ] {
            assert!(!passes(&key(k)), "{k}");
        }
    }
}
