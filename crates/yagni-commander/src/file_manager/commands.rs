//! File manager commands that need more than a core `Command`: dialogs (F2,
//! F7), launching the editor (F4) and quick search.

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, AppContext, Context, FocusHandle, Focusable, KeyDownEvent, ParentElement, SharedString,
    Window,
};
use yagni_commander_core::{EntryKind, QuickSearch, launch};

use super::FileManager;
use crate::CurrentConfig;

/// Handles the name typed into a prompt. An error keeps the dialog open.
type Submit = dyn Fn(&mut FileManager, &str, &mut Context<FileManager>) -> std::io::Result<()>;

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
            Rc::new(move |this, to, cx| {
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
        self.prompt_name(
            Prompt {
                title: "New directory",
                error_title: "Cannot create directory",
                initial: "",
                selection: 0..0,
            },
            Rc::new(|this, name, cx| {
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
        let Some(editor) = cx.global::<CurrentConfig>().0.editor.clone() else {
            show_error(
                "Cannot open editor",
                "No editor configured: set `editor` in config.toml.",
                None,
                window,
                cx,
            );
            return;
        };
        let path = self.active_panel(cx).cursor_path();
        if let Err(e) = launch::open_in_editor(&editor, &path) {
            show_error("Cannot open editor", e.to_string(), None, window, cx);
        }
    }

    /// Letters and digits typed without modifiers jump to the first matching
    /// name. Unmatched keys are ignored.
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
        if !QuickSearch::accepts(ch) {
            return;
        }
        let now = Instant::now();
        let prefix = self.quick_search.candidate(ch, now);
        let found = self.commander.update(cx, |commander, cx| {
            let found = commander.jump_to_prefix(&prefix);
            if found {
                cx.notify();
            }
            found
        });
        if found {
            self.quick_search.accept(prefix, now);
        }
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
                    .update(cx, |this, cx| submit(this, &name, cx))
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

        let title = SharedString::from(prompt.title.to_owned());
        let focus = self.focus.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm = confirm.clone();
            let focus_ok = focus.clone();
            let focus_cancel = focus.clone();
            dialog
                .title(title.clone())
                .w(gpui_kit::px(420.0))
                .child(Input::new(&input))
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                        .child(DialogAction::new().child(Button::new("ok").primary().label("OK"))),
                )
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

/// A centered, modal error box that stays until dismissed (button, Enter or
/// Escape). Focus goes to `refocus` afterwards, e.g. back into a prompt's
/// text field so the user can correct the input.
fn show_error(
    title: &'static str,
    message: impl Into<SharedString>,
    refocus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let message = message.into();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let refocus = refocus.clone();
        alert
            .title(title)
            .description(message.clone())
            .ok_text("Dismiss")
            .on_close(move |_, window, cx| {
                if let Some(focus) = refocus.clone() {
                    // After the dialog stack has finished restoring focus.
                    window.defer(cx, move |window, cx| focus.focus(window, cx));
                }
            })
    });
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
