//! The hotlist's Configure dialog: the list, Name and Path fields for the
//! entry under the cursor, and Add current folder / Remove / Move up /
//! Move down / Cancel / OK. Edits stay here until OK, which checks every
//! path and saves the whole list (`FileManager::save_hotlist`). The user
//! commands (Ctrl-S) are edited here too: a command is held as an entry's
//! path, with a Command field, a Help button, and New for Add current folder.

use crate::file_manager::commands::themed_dialog;
use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, HighlightStyle,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, StyledText, Subscription, UnderlineStyle, WeakEntity,
    Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::config::UserCommand;
use yagni_commander_core::hotlist::{self, HotlistEntry};
use yagni_commander_core::user_commands;

use crate::actions::{HOTLIST_LIST_CONTEXT, hotlist_list as act};
use crate::button_row::{ButtonRow, OnLetter, OnPress, dialog_keys, marked_label};
use crate::config_state::CurrentConfig;
use crate::file_manager::FileManager;
use crate::file_manager::commands::{
    dialog_input, focus_when_open, select_field, show_box, show_error, text_field,
};
use crate::theme::Theme;

/// What the dialog edits.
enum Kind {
    /// The hotlist, and the folder "Add current folder" adds.
    Folders(String),
    Commands,
}

/// The user commands as the popup and the dialog hold them.
pub(crate) fn command_entries(cx: &App) -> Vec<HotlistEntry> {
    cx.global::<CurrentConfig>()
        .config
        .commands
        .iter()
        .map(|c| HotlistEntry {
            name: c.name.clone(),
            path: c.command.clone(),
        })
        .collect()
}

pub struct HotlistDialog {
    kind: Kind,
    entries: Vec<HotlistEntry>,
    /// The row the fields edit. None until a row is picked or added, and
    /// the fields are disabled meanwhile, so typing can't change a row by
    /// accident or go nowhere.
    cursor: Option<usize>,
    name: Entity<InputState>,
    path: Entity<InputState>,
    list_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl HotlistDialog {
    fn new(
        kind: Kind,
        entries: Vec<HotlistEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let path = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this, field, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let text = field.read(cx).value().to_string();
                    if let Some(entry) = this.cursor.and_then(|ix| this.entries.get_mut(ix)) {
                        entry.name = text;
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&path, window, |this, field, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let text = field.read(cx).value().to_string();
                    if let Some(entry) = this.cursor.and_then(|ix| this.entries.get_mut(ix)) {
                        entry.path = text;
                        cx.notify();
                    }
                }
            }),
        ];
        let mut this = Self {
            kind,
            entries,
            cursor: None,
            name,
            path,
            list_focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        };
        this.show_cursor_entry(window, cx);
        this
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[HotlistEntry] {
        &self.entries
    }

    /// Alt-N and Alt-P (Alt-C for a command): the field focused with its
    /// text selected, once a row is picked (until then they're disabled).
    /// Whether the letter is a field's.
    pub fn alt(&mut self, letter: char, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let path_letter = match self.kind {
            Kind::Folders(_) => 'p',
            Kind::Commands => 'c',
        };
        let field = if letter == 'n' {
            self.name.clone()
        } else if letter == path_letter {
            self.path.clone()
        } else {
            return false;
        };
        if self.cursor.is_some() {
            select_field(&field, window, cx);
        }
        true
    }

    #[cfg(test)]
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    #[cfg(test)]
    pub fn field_texts(&self, cx: &App) -> (String, String) {
        (
            self.name.read(cx).value().to_string(),
            self.path.read(cx).value().to_string(),
        )
    }

    /// Puts the cursor entry into the fields (`set_value` emits no Change).
    fn show_cursor_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.cursor.and_then(|ix| self.entries.get(ix)).cloned();
        // A disabled field (see `render`) is no stop for Tab.
        let has_entry = entry.is_some();
        let _ = self.name.focus_handle(cx).tab_stop(has_entry);
        let _ = self.path.focus_handle(cx).tab_stop(has_entry);
        let (name, path) = entry.map_or_else(Default::default, |e| (e.name, e.path));
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.path.update(cx, |s, cx| s.set_value(path, window, cx));
        cx.notify();
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.entries.len() {
            self.cursor = Some(ix);
            self.show_cursor_entry(window, cx);
        }
    }

    /// Moves the cursor by `by` rows; with no row picked yet, Down picks the
    /// first and Up the last.
    fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let ix = match self.cursor {
            Some(cursor) => (cursor as isize + by).clamp(0, last),
            None if by > 0 => 0,
            None => last,
        };
        self.select(ix as usize, window, cx);
    }

    fn remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cursor) = self.cursor else {
            return;
        };
        self.entries.remove(cursor);
        self.cursor = self
            .entries
            .len()
            .checked_sub(1)
            .map(|last| cursor.min(last));
        self.show_cursor_entry(window, cx);
    }

    /// Moves the cursor entry by one row, if it can.
    fn shift(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(cursor) = self.cursor else {
            return;
        };
        let to = cursor as isize + by;
        if to < 0 || to as usize >= self.entries.len() {
            return;
        }
        self.entries.swap(cursor, to as usize);
        self.cursor = Some(to as usize);
        cx.notify();
    }

    /// Adds the current folder, or an empty command to fill in.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entries.push(match &self.kind {
            Kind::Folders(folder) => HotlistEntry {
                name: hotlist::default_name(folder),
                path: folder.clone(),
            },
            Kind::Commands => HotlistEntry {
                name: String::new(),
                path: String::new(),
            },
        });
        self.cursor = Some(self.entries.len() - 1);
        self.show_cursor_entry(window, cx);
        self.name.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The first entry that is not acceptable, with the reason.
    fn invalid(&self) -> Option<(usize, &'static str)> {
        let check = match self.kind {
            Kind::Folders(_) => hotlist::validate,
            Kind::Commands => user_commands::validate,
        };
        self.entries
            .iter()
            .enumerate()
            .find_map(|(ix, e)| check(e.path.trim()).err().map(|why| (ix, why)))
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        // Like the find dialog's results: the border says the list has the
        // keys, also while it is empty.
        let list_focused = self.list_focus.is_focused(window);
        // Nothing to edit until a row is picked. This goes on the `Input`:
        // it overwrites `InputState`'s flag on every render.
        let no_row = self.cursor.is_none();
        let underline = HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = self.entries.iter().enumerate().map(|(ix, entry)| {
            let label = hotlist::display_label(entry);
            let at_cursor = self.cursor == Some(ix);
            div()
                .id(("hotlist-entry", ix))
                .debug_selector(move || format!("hotlist-entry-{ix}"))
                .flex()
                .gap(crate::zoom::rems_from_px(16.0))
                .px(crate::zoom::rems_from_px(8.0))
                .py(crate::zoom::rems_from_px(2.0))
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
                    div().w(crate::zoom::rems_from_px(140.0)).child(
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
            .gap(crate::zoom::rems_from_px(8.0))
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
                    .mt(crate::zoom::rems_from_px(4.0))
                    .min_h(crate::zoom::rems_from_px(120.0))
                    .border_1()
                    .border_color(if list_focused {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .bg(colors.panel_bg)
                    .when(self.entries.is_empty(), |d| {
                        d.child(
                            div()
                                .debug_selector(|| "hotlist-empty".into())
                                .p(crate::zoom::rems_from_px(8.0))
                                .text_color(colors.text_dim)
                                .child(match self.kind {
                                    Kind::Folders(_) => "No folders yet",
                                    Kind::Commands => "No commands yet. Press New to create one.",
                                }),
                        )
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(crate::zoom::rems_from_px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .w(crate::zoom::rems_from_px(84.0))
                            .whitespace_nowrap()
                            .child(marked_label("&Name")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(text_field(&self.name).disabled(no_row)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(crate::zoom::rems_from_px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .w(crate::zoom::rems_from_px(84.0))
                            .whitespace_nowrap()
                            .child(marked_label(match self.kind {
                                Kind::Folders(_) => "&Path",
                                Kind::Commands => "&Command",
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(dialog_input(text_field(&self.path).disabled(no_row))),
                    ),
            )
            .when(matches!(self.kind, Kind::Commands), |d| {
                d.child(
                    div()
                        .debug_selector(|| "commands-hint".into())
                        .text_sm()
                        .text_color(colors.text_secondary)
                        .child(
                            "Nothing is added to a command: use {active_path} to pass the file (see Help).",
                        ),
                )
            })
    }
}

impl FileManager {
    /// The hotlist's "Configure...": an OK/Cancel dialog over a copy of
    /// the list. Refused while the config file is broken.
    pub(crate) fn open_hotlist_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_refused("Hotlist not saved", window, cx) {
            return;
        }
        let entries = cx.global::<CurrentConfig>().config.hotlist.clone();
        let folder = self.current_folder_entry_path(cx);
        self.open_list_dialog(Kind::Folders(folder), entries, window, cx);
    }

    /// The user commands' "Configure...", the same dialog.
    pub(crate) fn open_commands_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_refused(COMMANDS_NOT_SAVED, window, cx) {
            return;
        }
        let entries = command_entries(cx);
        self.open_list_dialog(Kind::Commands, entries, window, cx);
    }

    fn open_list_dialog(
        &mut self,
        kind: Kind,
        entries: Vec<HotlistEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = matches!(kind, Kind::Commands);
        let (title, not_saved, add_label) = if commands {
            ("User commands", COMMANDS_NOT_SAVED, "Ne&w")
        } else {
            (
                "Directory hotlist",
                "Hotlist not saved",
                "&Add current folder",
            )
        };
        let view = cx.new(|cx| HotlistDialog::new(kind, entries, window, cx));
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
                        not_saved,
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
                    if commands {
                        let list = entries
                            .into_iter()
                            .map(|e| UserCommand {
                                name: e.name,
                                command: e.path,
                            })
                            .collect();
                        fm.save_commands(list, window, cx);
                    } else {
                        fm.save_hotlist(entries, window, cx);
                    }
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
        let [add, remove, up, down, cancel_row, ok_row] = [
            (add_label, on_view(HotlistDialog::add)),
            ("&Remove", on_view(HotlistDialog::remove)),
            ("Move &up", on_view(|v, _, cx| v.shift(-1, cx))),
            ("Move &down", on_view(|v, _, cx| v.shift(1, cx))),
            ("Cancel", cancel_button),
            ("OK", ok_button),
        ];
        let buttons = if commands {
            let refocus = view.read(cx).list_focus.clone();
            let help: OnPress =
                Rc::new(move |window, cx| show_command_help(refocus.clone(), window, cx));
            let row = ButtonRow::build(
                [("&Help", help), add, remove, up, down, cancel_row, ok_row],
                6,
                cx,
            );
            row.update(cx, |row, _| row.set_apart(1));
            row
        } else {
            ButtonRow::build([add, remove, up, down, cancel_row, ok_row], 5, cx)
        };
        focus_when_open(view.read(cx).list_focus.clone(), window, cx);
        let alt: OnLetter = Rc::new({
            let view = view.clone();
            move |letter, window, cx| view.update(cx, |v, cx| v.alt(letter, window, cx))
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let (ok, cancel) = (ok.clone(), cancel.clone());
            themed_dialog(dialog, cx)
                .title(title)
                .w(crate::zoom::dialog_width(720.0, cx))
                .close_button(false)
                .child(dialog_keys(
                    div().child(view.clone()),
                    &buttons,
                    Some(alt.clone()),
                ))
                .footer(dialog_keys(
                    div().child(buttons.clone()),
                    &buttons,
                    Some(alt.clone()),
                ))
                // Enter anywhere in the dialog.
                .on_ok(move |_, window, cx| ok(window, cx))
                .on_cancel(move |_, window, cx| {
                    cancel(window, cx);
                    true
                })
        });
    }
}

const COMMANDS_NOT_SAVED: &str = "Commands not saved";

const COMMAND_HELP: &str = "Parameters can go anywhere in a command, also inside a word. Active \
    is the panel you are in, inactive the other one. On \"..\" the entry is the folder itself. \
    Nothing is added to a command by itself, so without a parameter it gets no file. Click a parameter to copy it.";

const COMMAND_HELP_END: &str = "Commands run without a shell, so pipes and $VARIABLES don't \
    work. A path with spaces stays one argument, so no quotes are needed around a parameter.";

const COMMAND_EXAMPLES: [(&str, &str); 4] = [
    ("sqlitebrowser {active_path}", "open the active file"),
    (
        "meld {active_path} {inactive_path}",
        "compare the two cursor entries",
    ),
    ("code {active_dir}", "open the active panel's folder"),
    (
        "ffmpeg -i {active_path} {active_stem}.mp4",
        "convert, next to the file",
    ),
];

/// A column of code in the help box, `width` px wide.
fn code_cell(code: &str, width: f32) -> gpui_kit::Div {
    div()
        .flex_none()
        .w(crate::zoom::rems_from_px(width))
        .font_family(crate::fonts::MONO_FAMILY)
        .text_sm()
        .child(code.to_owned())
}

/// A row of the help box: `cells`, then `text`.
fn help_row(
    cells: impl IntoIterator<Item = impl IntoElement>,
    text: &str,
    color: gpui_kit::Rgba,
) -> gpui_kit::Div {
    div()
        .flex()
        .gap(crate::zoom::rems_from_px(12.0))
        .children(cells)
        .child(div().flex_1().text_color(color).child(text.to_owned()))
}

/// A parameter in the help box: a click copies its name, and it stays in
/// the accent color as the one last copied.
fn parameter_cell(
    name: &str,
    width: f32,
    copied: &Rc<RefCell<Option<String>>>,
    accent: gpui_kit::Rgba,
) -> impl IntoElement + use<> {
    let id = SharedString::from(name.to_owned());
    let selector = format!("help-parameter-{name}");
    let is_copied = copied.borrow().as_deref() == Some(name);
    let (name, copied) = (name.to_owned(), copied.clone());
    code_cell(&name, width)
        .id(id)
        .debug_selector(move || selector.clone())
        .cursor_pointer()
        .hover(move |d| d.text_color(accent))
        .when(is_copied, move |d| d.text_color(accent))
        .on_click(move |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
            *copied.borrow_mut() = Some(name.clone());
            window.refresh();
        })
}

/// The Help button of the user commands' dialog: every parameter, and
/// some commands that use them.
fn show_command_help(refocus: FocusHandle, window: &mut Window, cx: &mut App) {
    let list = user_commands::parameters();
    let copied = std::iter::once(COMMAND_HELP.to_owned())
        .chain(list.iter().map(|(name, text)| format!("{name}: {text}")))
        .chain(std::iter::once(COMMAND_HELP_END.to_owned()))
        .chain(
            COMMAND_EXAMPLES
                .iter()
                .map(|(code, text)| format!("{code}: {text}")),
        )
        .collect::<Vec<_>>()
        .join("\n");
    // The box's title and button, and a margin around it; the rest scrolls.
    let room = window.viewport_size().height
        - px(crate::zoom::scaled(200.0, crate::zoom::Zoom::get(cx).ui));
    let max_height = room.max(px(120.0));
    let last_copied = Rc::new(RefCell::new(None));
    let body = move |cx: &mut App| {
        let colors = Theme::get(cx).colors.clone();
        let (secondary, accent) = (colors.text_secondary, colors.accent);
        // The list holds every active parameter, then the inactive ones.
        let (active, inactive) = list.split_at(list.len() / 2);
        let parameters = active.iter().zip(inactive).map(|((a, text), (i, _))| {
            let cells = [
                parameter_cell(a, 170.0, &last_copied, accent).into_any_element(),
                parameter_cell(i, 190.0, &last_copied, accent).into_any_element(),
            ];
            help_row(cells, text, secondary)
        });
        let examples = COMMAND_EXAMPLES
            .iter()
            .map(|(code, text)| help_row([code_cell(code, 440.0)], text, secondary));
        div()
            .id("command-help-scroll")
            .debug_selector(|| "command-help".into())
            .max_h(max_height)
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(crate::zoom::rems_from_px(8.0))
            .child(COMMAND_HELP)
            .child(div().flex().flex_col().children(parameters))
            .child(COMMAND_HELP_END)
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Examples"),
            )
            .child(div().flex().flex_col().children(examples))
            .into_any_element()
    };
    show_box(
        "Command parameters",
        body,
        copied,
        740.0,
        "Close",
        Some(refocus),
        window,
        cx,
    );
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
impl HotlistDialog {
    /// The focusable controls by name, for tab-order tests.
    pub fn tab_stops(&self, cx: &App) -> Vec<(&'static str, FocusHandle)> {
        vec![
            ("list", self.list_focus.clone()),
            ("name", self.name.focus_handle(cx)),
            ("path", self.path.focus_handle(cx)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_help_examples_are_valid_commands() {
        for (command, _) in COMMAND_EXAMPLES {
            assert_eq!(user_commands::validate(command), Ok(()), "{command}");
        }
    }

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
