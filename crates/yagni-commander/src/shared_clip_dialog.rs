//! The shared clipboard dialog's list (Ctrl-K Ctrl-V): the host's items
//! (core [`shared_clip::list`]), newest first, each its preview and its
//! machine and age. Up/Down wrap, Home/End and Page Up/Page Down move, F3
//! views, Del deletes, a double click copies; Enter is the dialog's (Copy).
//! The dialog around it is built in `file_manager/shared_clip.rs`.

use std::time::SystemTime;

use gpui_kit::{
    Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder,
};
use yagni_commander_core::shared_clip::{self, Item};

use crate::actions::{SHARED_CLIP_CONTEXT, shared_clip as act};
use crate::theme::Theme;
use crate::zoom::rems_from_px;

pub enum SharedClipEvent {
    /// A double click: copy the highlighted item.
    Pick,
    /// F3: view the item, the file name given.
    View(Vec<u8>),
    /// Deleted from the list; the host still has it.
    Remove(Vec<u8>),
}

/// Rows shown before the list scrolls.
const MAX_ROWS: usize = 14;
const ROW_HEIGHT: f32 = 26.0;

pub struct SharedClipList {
    items: Vec<Item>,
    highlight: usize,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// When the list was read, for the ages.
    now: SystemTime,
    /// The file manager's subscription to this list's events.
    _events: Option<Subscription>,
}

impl EventEmitter<SharedClipEvent> for SharedClipList {}

impl Focusable for SharedClipList {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SharedClipList {
    /// `items` newest first; the highlight starts on the first.
    pub fn new(items: Vec<Item>, cx: &mut Context<Self>) -> Self {
        Self {
            items,
            highlight: 0,
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            now: SystemTime::now(),
            _events: None,
        }
    }

    /// Keeps `events` as long as the list lives.
    pub fn keep(&mut self, events: Subscription) {
        self._events = Some(events);
    }

    #[cfg(test)]
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The highlighted item's file name.
    pub fn current(&self) -> Option<Vec<u8>> {
        self.items.get(self.highlight).map(|item| item.name.clone())
    }

    fn go_to(&mut self, row: usize, cx: &mut Context<Self>) {
        self.highlight = row.min(self.items.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.highlight);
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.items.len() as isize;
        if rows > 0 {
            self.go_to((self.highlight as isize + by).rem_euclid(rows) as usize, cx);
        }
    }

    fn page(&mut self, down: bool, cx: &mut Context<Self>) {
        let row = if down {
            self.highlight + (MAX_ROWS - 1)
        } else {
            self.highlight.saturating_sub(MAX_ROWS - 1)
        };
        self.go_to(row, cx);
    }

    /// Del or the Delete button: the highlighted item leaves the list.
    pub fn remove(&mut self, cx: &mut Context<Self>) {
        if self.highlight >= self.items.len() {
            return;
        }
        let item = self.items.remove(self.highlight);
        cx.emit(SharedClipEvent::Remove(item.name));
        self.go_to(self.highlight, cx);
    }
}

impl Render for SharedClipList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        // Like the hotlist's list: the border says the list has the keys,
        // and the cursor is bright only then.
        let focused = self.focus.is_focused(window);
        let rows = self.items.iter().enumerate().map(|(ix, item)| {
            let highlighted = ix == self.highlight;
            let detail = format!(
                "{} · {}",
                item.machine,
                shared_clip::age(self.now, item.modified)
            );
            div()
                .id(("shared-row", ix))
                .debug_selector(move || format!("shared-row-{ix}"))
                .h(rems_from_px(ROW_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .gap(rems_from_px(16.0))
                .px(rems_from_px(8.0))
                .when(highlighted && focused, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .when(highlighted && !focused, |d| d.bg(colors.cursor_inactive_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.go_to(ix, cx);
                        if event.click_count == 2 {
                            cx.emit(SharedClipEvent::Pick);
                        }
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(item.preview.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .when(!(highlighted && focused), |d| {
                            d.text_color(colors.text_secondary)
                        })
                        .child(detail),
                )
        });
        div()
            .id("shared-list")
            .debug_selector(|| "shared-list".into())
            .key_context(SHARED_CLIP_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::First, _, cx| this.go_to(0, cx)))
            .on_action(cx.listener(|this, _: &act::Last, _, cx| this.go_to(usize::MAX, cx)))
            .on_action(cx.listener(|this, _: &act::PageUp, _, cx| this.page(false, cx)))
            .on_action(cx.listener(|this, _: &act::PageDown, _, cx| this.page(true, cx)))
            .on_action(cx.listener(|this, _: &act::Remove, _, cx| this.remove(cx)))
            .on_action(cx.listener(|this, _: &act::View, _, cx| {
                if let Some(name) = this.current() {
                    cx.emit(SharedClipEvent::View(name));
                }
            }))
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .mt(rems_from_px(4.0))
            .mb(rems_from_px(4.0))
            .min_h(rems_from_px(ROW_HEIGHT * 4.0))
            .max_h(rems_from_px(ROW_HEIGHT * MAX_ROWS as f32))
            .flex()
            .flex_col()
            .border_1()
            .border_color(if focused {
                colors.accent
            } else {
                colors.border
            })
            .bg(colors.panel_bg)
            .text_color(colors.text)
            .when(self.items.is_empty(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "shared-empty".into())
                        .p(rems_from_px(8.0))
                        .text_color(colors.text_dim)
                        .child("The shared clipboard is empty"),
                )
            })
            .children(rows)
    }
}
