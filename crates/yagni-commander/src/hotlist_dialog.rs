//! The hotlist's Configure dialog: the list, Name and Path fields for the
//! entry under the cursor, and Add current folder / Remove / Move up /
//! Move down / Cancel / OK. Edits stay here until OK, which checks every
//! path and saves the whole list (`FileManager::save_hotlist`).

use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, HighlightStyle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, Styled, StyledText, Subscription,
    UnderlineStyle, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::hotlist::{self, HotlistEntry};

use crate::actions::{HOTLIST_LIST_CONTEXT, hotlist_list as act};
use crate::button_row::{ButtonRow, OnPress};
use crate::config_state::CurrentConfig;
use crate::file_manager::FileManager;
use crate::file_manager::commands::{dialog_field, focus_when_open, show_error, text_field};
use crate::theme::Theme;

pub struct HotlistDialog {
    entries: Vec<HotlistEntry>,
    cursor: usize,
    name: Entity<InputState>,
    path: Entity<InputState>,
    list_focus: FocusHandle,
    /// What "Add current folder" adds (the active panel's folder).
    current_folder: String,
    _subscriptions: Vec<Subscription>,
}

impl HotlistDialog {
    fn new(
        entries: Vec<HotlistEntry>,
        current_folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let path = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this, field, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let text = field.read(cx).value().to_string();
                    if let Some(entry) = this.entries.get_mut(this.cursor) {
                        entry.name = text;
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&path, window, |this, field, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let text = field.read(cx).value().to_string();
                    if let Some(entry) = this.entries.get_mut(this.cursor) {
                        entry.path = text;
                        cx.notify();
                    }
                }
            }),
        ];
        let mut this = Self {
            entries,
            cursor: 0,
            name,
            path,
            list_focus: cx.focus_handle().tab_stop(true),
            current_folder,
            _subscriptions: subscriptions,
        };
        this.show_cursor_entry(window, cx);
        this
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[HotlistEntry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[cfg(test)]
    pub fn field_texts(&self, cx: &App) -> (String, String) {
        (
            self.name.read(cx).value().to_string(),
            self.path.read(cx).value().to_string(),
        )
    }

    /// Puts the cursor entry into the fields (`set_value` emits no Change)
    /// and disables them while the list is empty.
    fn show_cursor_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.entries.get(self.cursor).cloned();
        let empty = entry.is_none();
        let (name, path) = entry.map_or_else(Default::default, |e| (e.name, e.path));
        self.name.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.set_disabled(empty, cx);
        });
        self.path.update(cx, |s, cx| {
            s.set_value(path, window, cx);
            s.set_disabled(empty, cx);
        });
        cx.notify();
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.entries.len() {
            self.cursor = ix;
            self.show_cursor_entry(window, cx);
        }
    }

    fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.entries.len().saturating_sub(1) as isize;
        let ix = (self.cursor as isize + by).clamp(0, last) as usize;
        self.select(ix, window, cx);
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor >= self.entries.len() {
            return;
        }
        self.entries.remove(self.cursor);
        self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        self.show_cursor_entry(window, cx);
    }

    /// Moves the cursor entry by one row, if it can.
    fn shift(&mut self, by: isize, cx: &mut Context<Self>) {
        let to = self.cursor as isize + by;
        if to < 0 || to as usize >= self.entries.len() {
            return;
        }
        self.entries.swap(self.cursor, to as usize);
        self.cursor = to as usize;
        cx.notify();
    }

    fn add_current_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entries.push(HotlistEntry {
            name: hotlist::default_name(&self.current_folder),
            path: self.current_folder.clone(),
        });
        self.cursor = self.entries.len() - 1;
        self.show_cursor_entry(window, cx);
        self.name.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The first entry whose path is not acceptable, with the reason.
    fn invalid(&self) -> Option<(usize, &'static str)> {
        self.entries
            .iter()
            .enumerate()
            .find_map(|(ix, e)| hotlist::validate(e.path.trim()).err().map(|why| (ix, why)))
    }

    /// The list as OK saves it: paths without surrounding spaces, which are
    /// easy to paste and would never match a folder.
    fn trimmed(&self) -> Vec<HotlistEntry> {
        self.entries
            .iter()
            .map(|e| HotlistEntry {
                name: e.name.clone(),
                path: e.path.trim().to_owned(),
            })
            .collect()
    }
}

impl Render for HotlistDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let underline = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = self.entries.iter().enumerate().map(|(ix, entry)| {
            let label = hotlist::display_label(entry);
            let at_cursor = ix == self.cursor;
            div()
                .id(("hotlist-entry", ix))
                .debug_selector(move || format!("hotlist-entry-{ix}"))
                .flex()
                .gap(px(16.0))
                .px(px(8.0))
                .py(px(2.0))
                .when(at_cursor, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.select(ix, window, cx);
                        this.list_focus.focus(window, cx);
                    }),
                )
                .child(
                    div().w(px(140.0)).child(
                        StyledText::new(label.text)
                            .with_highlights(label.underline.map(|r| (r, underline))),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .when(!at_cursor, |d| d.text_color(colors.text_secondary))
                        .child(entry.path.clone()),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .id("hotlist-list")
                    .key_context(HOTLIST_LIST_CONTEXT)
                    .track_focus(&self.list_focus)
                    .on_action(
                        cx.listener(|this, _: &act::Up, window, cx| this.step(-1, window, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &act::Down, window, cx| this.step(1, window, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &act::Remove, window, cx| this.remove(window, cx)),
                    )
                    .on_action(cx.listener(|this, _: &act::MoveUp, _, cx| this.shift(-1, cx)))
                    .on_action(cx.listener(|this, _: &act::MoveDown, _, cx| this.shift(1, cx)))
                    .mt(px(4.0))
                    .min_h(px(120.0))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.panel_bg)
                    .when(self.entries.is_empty(), |d| {
                        d.child(
                            div()
                                .debug_selector(|| "hotlist-empty".into())
                                .p(px(8.0))
                                .text_color(colors.text_dim)
                                .child("No folders yet"),
                        )
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_none().w(px(48.0)).child("Name"))
                    .child(div().flex_1().child(text_field(&self.name))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_none().w(px(48.0)).child("Path"))
                    .child(div().flex_1().child(dialog_field(&self.path))),
            )
    }
}

impl FileManager {
    /// The hotlist's "Configure...": an OK/Cancel dialog over a copy of
    /// the list. Refused while the config file is broken.
    pub(crate) fn open_hotlist_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_refused(window, cx) {
            return;
        }
        let entries = cx.global::<CurrentConfig>().config.hotlist.clone();
        let folder = self.current_folder_entry_path(cx);
        let view = cx.new(|cx| HotlistDialog::new(entries, folder, window, cx));
        self.hotlist_dialog = Some(view.clone());
        let me: WeakEntity<FileManager> = cx.entity().downgrade();
        let focus = self.focus.clone();

        // OK: check every path, then save and close. Returns whether it closed.
        let ok = {
            let (view, me, focus) = (view.clone(), me.clone(), focus.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| -> bool {
                if let Some((ix, why)) = view.read(cx).invalid() {
                    let name = entry_name(&view.read(cx).entries[ix], ix);
                    view.update(cx, |v, cx| v.select(ix, window, cx));
                    let path_field = view.read(cx).path.focus_handle(cx);
                    show_error(
                        "Hotlist not saved",
                        format!("{name}: {why}"),
                        Some(path_field),
                        window,
                        cx,
                    );
                    return false;
                }
                let entries = view.read(cx).trimmed();
                let _ = me.update(cx, |fm, cx| {
                    fm.hotlist_dialog = None;
                    fm.save_hotlist(entries, window, cx);
                });
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
                true
            })
        };
        let cancel = {
            let (me, focus) = (me.clone(), focus.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |fm, _| fm.hotlist_dialog = None);
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
            })
        };

        let on_view =
            |f: fn(&mut HotlistDialog, &mut Window, &mut Context<HotlistDialog>)| -> OnPress {
                let view = view.clone();
                Rc::new(move |window, cx| view.update(cx, |v, cx| f(v, window, cx)))
            };
        let ok_button: OnPress = Rc::new({
            let ok = ok.clone();
            move |window, cx| {
                if ok(window, cx) {
                    window.close_dialog(cx);
                }
            }
        });
        let cancel_button: OnPress = Rc::new({
            let cancel = cancel.clone();
            move |window, cx| {
                cancel(window, cx);
                window.close_dialog(cx);
            }
        });
        let buttons = ButtonRow::build(
            [
                (
                    "Add current folder",
                    on_view(HotlistDialog::add_current_folder),
                ),
                ("Remove", on_view(HotlistDialog::remove)),
                ("Move up", on_view(|v, _, cx| v.shift(-1, cx))),
                ("Move down", on_view(|v, _, cx| v.shift(1, cx))),
                ("Cancel", cancel_button),
                ("OK", ok_button),
            ],
            5,
            cx,
        );
        focus_when_open(view.read(cx).list_focus.clone(), window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let (ok, cancel) = (ok.clone(), cancel.clone());
            dialog
                .title("Directory hotlist")
                .w(px(720.0))
                .close_button(false)
                .child(view.clone())
                .footer(buttons.clone())
                // Enter anywhere in the dialog.
                .on_ok(move |_, window, cx| ok(window, cx))
                .on_cancel(move |_, window, cx| {
                    cancel(window, cx);
                    true
                })
        });
    }
}

/// How the error box names entry `ix`: its label, or its number when it
/// has neither a name nor a path.
fn entry_name(entry: &HotlistEntry, ix: usize) -> String {
    let text = hotlist::display_label(entry).text;
    if text.trim().is_empty() {
        format!("Entry {}", ix + 1)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_without_name_or_path_is_named_by_its_number() {
        let entry = |name: &str, path: &str| HotlistEntry {
            name: name.into(),
            path: path.into(),
        };
        assert_eq!(entry_name(&entry("&Src", "src"), 0), "Src");
        assert_eq!(entry_name(&entry("", "src"), 0), "src");
        assert_eq!(entry_name(&entry("", ""), 2), "Entry 3");
    }
}
