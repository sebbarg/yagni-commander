//! File manager commands that need more than a core `Command`: dialogs (F2,
//! F7, Shift-F4), launching the editor (F4) and quick search.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, AppContext, Context, FocusHandle, Focusable, KeyDownEvent, ParentElement, SharedString,
    Window,
};
use yagni_commander_core::{Commander, EntryKind, launch};

use super::FileManager;
use crate::CurrentConfig;
use crate::button_row::{ButtonRow, OnPress};

/// Handles the name typed into a prompt. An error keeps the dialog open.
type Submit =
    dyn Fn(&mut FileManager, &str, &mut Window, &mut Context<FileManager>) -> std::io::Result<()>;

/// Text of a name prompt.
pub(super) struct Prompt<'a> {
    pub title: &'a str,
    /// Title of the error box shown when submitting fails.
    pub error_title: &'static str,
    pub initial: &'a str,
    /// Byte range of `initial` to preselect.
    pub selection: Range<usize>,
}

/// Submits the prompt; returns whether the dialog should close.
type Confirm = dyn Fn(&mut Window, &mut App) -> bool;

impl FileManager {
    /// F2: rename the entry under the cursor.
    pub(super) fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let panel = self.active_panel(cx);
        let Some(entry) = panel.cursor_entry().filter(|e| e.kind != EntryKind::Parent) else {
            return;
        };
        let from = entry.name.clone();
        let label = entry.label.clone();
        let stem = stem_range(&label, entry.kind == EntryKind::Dir);
        self.prompt_name(
            Prompt {
                title: "Rename",
                error_title: "Rename failed",
                initial: &label,
                selection: stem,
            },
            Rc::new(move |this, to, _, cx| {
                this.commander.update(cx, |commander, cx| {
                    let result = commander.rename(&from, to);
                    cx.notify();
                    result
                })
            }),
            window,
            cx,
        );
    }

    /// F7: create a directory (nested paths like `a/b` allowed).
    pub(super) fn make_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        self.prompt_name(
            Prompt {
                title: "New directory",
                error_title: "Cannot create directory",
                initial: "",
                selection: 0..0,
            },
            Rc::new(|this, name, _, cx| {
                this.commander.update(cx, |commander, cx| {
                    let result = commander.make_directory(name);
                    cx.notify();
                    result
                })
            }),
            window,
            cx,
        );
    }

    /// F4: open the entry under the cursor (or the directory, on "..") in
    /// the configured editor.
    pub(super) fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let Some(editor) = configured_editor(window, cx) else {
            return;
        };
        let path = self.active_panel(cx).cursor_path();
        if let Err(e) = launch::open_in_editor(&editor, &path) {
            show_error("Cannot open editor", e.to_string(), None, window, cx);
        }
    }

    /// Shift-F4: create a file (or pick an existing one) and open it in the
    /// editor. Asks for the name only if an editor is configured.
    pub(super) fn edit_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let Some(editor) = configured_editor(window, cx) else {
            return;
        };
        self.prompt_name(
            Prompt {
                title: "Edit new file",
                error_title: "Cannot edit file",
                initial: "",
                selection: 0..0,
            },
            Rc::new(move |this, name, _, cx| {
                let path = this.commander.update(cx, |commander, cx| {
                    let result = commander.create_file(name);
                    cx.notify();
                    result
                })?;
                launch::open_in_editor(&editor, &path)
            }),
            window,
            cx,
        );
    }

    /// F3: view the file under the cursor in a new window. Directories and
    /// ".." do nothing; files that can't be viewed (FIFOs, devices,
    /// unreadable files) show an error.
    pub(super) fn view_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let panel = self.active_panel(cx);
        if panel
            .cursor_entry()
            .is_none_or(|e| e.kind != EntryKind::File)
        {
            return;
        }
        let path = panel.cursor_path();
        let main = window.window_bounds();
        if let Err(e) = crate::viewer_view::open(path, main, cx) {
            show_error(
                "Cannot view file",
                e.to_string(),
                Some(self.focus.clone()),
                window,
                cx,
            );
        }
    }

    /// Printable characters typed without modifiers start or extend a quick
    /// search: the cursor jumps to the first name starting with the typed
    /// text, shown in a box in the panel footer. Unmatched keys are ignored.
    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        let mut chars = keystroke.key_char.as_deref().unwrap_or_default().chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        if !Commander::search_accepts(ch) {
            return;
        }
        self.notice = None;
        self.commander.update(cx, |commander, cx| {
            if commander.search_type(ch) {
                cx.notify();
            }
        });
        cx.stop_propagation();
    }

    /// A dialog with a text field. Enter or OK submits; Escape or Cancel
    /// closes. Focus returns to the panels when the dialog closes. Enter needs
    /// no handling here: the dialog maps it to OK.
    pub(super) fn prompt_name(
        &mut self,
        prompt: Prompt,
        submit: Rc<Submit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(prompt.initial.to_owned()));
        input.update(cx, |state, cx| {
            state.set_selected_range(prompt.selection, cx)
        });

        let this = cx.entity().downgrade();
        let error_title = prompt.error_title;
        let confirm: Rc<Confirm> = Rc::new({
            let input = input.clone();
            move |window, cx| {
                let name = input.read(cx).value().to_string();
                let result = this
                    .update(cx, |this, cx| submit(this, &name, window, cx))
                    .unwrap_or(Ok(()));
                match result {
                    Ok(()) => true,
                    Err(e) => {
                        let field = input.read(cx).focus_handle(cx);
                        show_error(error_title, e.to_string(), Some(field), window, cx);
                        false
                    }
                }
            }
        });

        // Opening the dialog moves focus to it, so focus the field afterwards.
        window.defer(cx, {
            let input = input.clone();
            move |window, cx| input.update(cx, |state, cx| state.focus(window, cx))
        });

        let focus = self.focus.clone();
        let ok: OnPress = Rc::new({
            let (confirm, focus) = (confirm.clone(), focus.clone());
            move |window, cx| {
                if confirm(window, cx) {
                    window.close_dialog(cx);
                    focus.focus(window, cx);
                }
            }
        });
        let cancel: OnPress = Rc::new({
            let focus = focus.clone();
            move |window, cx| {
                window.close_dialog(cx);
                focus.focus(window, cx);
            }
        });
        let buttons = ButtonRow::build([("Cancel", cancel), ("OK", ok)], 1, cx);

        let title = SharedString::from(prompt.title.to_owned());
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm = confirm.clone();
            let focus_ok = focus.clone();
            let focus_cancel = focus.clone();
            dialog
                .title(title.clone())
                .w(gpui_kit::px(420.0))
                .child(Input::new(&input))
                .footer(buttons.clone())
                // Enter in the text field.
                .on_ok(move |_, window, cx| {
                    let done = confirm(window, cx);
                    if done {
                        focus_ok.focus(window, cx);
                    }
                    done
                })
                .on_cancel(move |_, window, cx| {
                    focus_cancel.focus(window, cx);
                    true
                })
        });
    }
}

impl FileManager {
    /// F12 (temporary): a dialog with a text field and six buttons, to try
    /// the button row's keyboard handling. The status line shows what closed it.
    pub(super) fn button_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value("Tab to the buttons"));
        let this = cx.entity().downgrade();
        let report = move |what: String, cx: &mut App| {
            let _ = this.update(cx, |this, cx| {
                this.notice = Some(what.into());
                cx.notify();
            });
        };
        let focus = self.focus.clone();
        let button = |label: &'static str| -> (&'static str, OnPress) {
            let (report, focus, input) = (report.clone(), focus.clone(), input.clone());
            let on_press: OnPress = Rc::new(move |window, cx| {
                let text = input.read(cx).value().to_string();
                window.close_dialog(cx);
                focus.focus(window, cx);
                report(format!("Button test: pressed “{label}”, text “{text}”"), cx);
            });
            (label, on_press)
        };
        let buttons = ButtonRow::build(
            [
                button("One"),
                button("Two"),
                button("Three"),
                button("Four"),
                button("Five"),
                button("Six"),
            ],
            2,
            cx,
        );
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let (report, focus) = (report.clone(), focus.clone());
            dialog
                .title("Button test")
                .w(gpui_kit::px(560.0))
                .close_button(false)
                .child("Left/Right, Tab/Shift-Tab, Enter/Space, Escape. \"Three\" starts selected.")
                .child(Input::new(&input))
                .footer(buttons.clone())
                .on_cancel(move |_, window, cx| {
                    focus.focus(window, cx);
                    report("Button test: Escape".into(), cx);
                    true
                })
        });
    }
}

/// The `editor` setting, or an error box saying it is missing.
fn configured_editor(window: &mut Window, cx: &mut App) -> Option<String> {
    let editor = cx.global::<CurrentConfig>().0.editor.clone();
    if editor.is_none() {
        show_error(
            "Cannot open editor",
            "No editor configured: set `editor` in config.toml.",
            None,
            window,
            cx,
        );
    }
    editor
}

/// A centered, modal error box that stays until dismissed (button, Enter or
/// Escape). Focus goes to `refocus` afterwards, e.g. back into a prompt's
/// text field so the user can correct the input.
pub(super) fn show_error(
    title: &'static str,
    message: impl Into<SharedString>,
    refocus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let message = message.into();
    // After the dialog stack has finished restoring focus.
    let restore = move |refocus: &Option<FocusHandle>, window: &mut Window, cx: &mut App| {
        if let Some(focus) = refocus.clone() {
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        }
    };
    let dismiss: OnPress = Rc::new({
        let refocus = refocus.clone();
        move |window, cx| {
            window.close_dialog(cx);
            restore(&refocus, window, cx);
        }
    });
    let buttons = ButtonRow::build([("Dismiss", dismiss)], 0, cx);
    focus_when_open(buttons.focus_handle(cx), window, cx);
    window.open_dialog(cx, move |dialog, _, _| {
        let refocus = refocus.clone();
        dialog
            .title(title)
            .w(gpui_kit::px(420.0))
            .close_button(false)
            .child(message.clone())
            .footer(buttons.clone())
            .on_cancel(move |_, window, cx| {
                restore(&refocus, window, cx);
                true
            })
    });
}

/// Opening a dialog moves focus to the dialog itself; this moves it on to
/// `handle` (typically the dialog's button row) once the dialog is open.
pub(super) fn focus_when_open(handle: FocusHandle, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| handle.focus(window, cx));
}

/// The part of a name to preselect for renaming: everything before the last
/// extension for files (like TC), the whole name for directories and for
/// names like `.bashrc` that are all extension.
fn stem_range(name: &str, is_dir: bool) -> Range<usize> {
    match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 => 0..dot,
        _ => 0..name.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::stem_range;

    #[test]
    fn stem_excludes_last_extension_of_files_only() {
        assert_eq!(stem_range("report.final.pdf", false), 0..12);
        assert_eq!(stem_range("Makefile", false), 0..8);
        assert_eq!(stem_range(".bashrc", false), 0..7);
        assert_eq!(stem_range("archive.d", true), 0..9);
        assert_eq!(stem_range("æble.txt", false), 0.."æble".len());
    }
}
