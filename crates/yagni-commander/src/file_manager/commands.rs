//! File manager commands that need more than a core `Command`: dialogs (F2,
//! F7, Shift-F4), launching the editor (F4) and quick search.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent, ParentElement,
    SharedString, Window,
};
use yagni_commander_core::{Command, Commander, EntryKind, Outcome, launch};

use super::FileManager;
use crate::button_row::{ButtonRow, OnPress};
use crate::config_state::CurrentConfig;

/// How often a started opener (Enter on a file) is checked for failure.
const OPENER_POLL: std::time::Duration = std::time::Duration::from_millis(100);

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
    /// Dialog width in pixels.
    pub width: f32,
}

/// Width of a prompt for a name.
pub(super) const PROMPT_WIDTH: f32 = 420.0;

/// Submits the prompt; returns whether the dialog should close.
type Confirm = dyn Fn(&mut Window, &mut App) -> bool;

impl FileManager {
    /// F2: rename the entry under the cursor.
    pub(super) fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let panel = self.active_panel(cx);
        let Some(entry) = panel.cursor_entry().filter(|e| e.kind != EntryKind::Parent) else {
            return;
        };
        let from = entry.name.clone();
        let label = entry.label.clone();
        let dir = panel.path().to_path_buf();
        let stem = stem_range(&label, entry.kind == EntryKind::Dir);
        self.prompt_name(
            Prompt {
                title: "Rename",
                error_title: "Rename failed",
                initial: &label,
                selection: stem,
                width: PROMPT_WIDTH,
            },
            Rc::new(move |this, to, _, cx| {
                this.commander.update(cx, |commander, cx| {
                    let result = commander
                        .check_dir(&dir)
                        .and_then(|()| commander.rename(&from, to));
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
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let dir = self.active_panel(cx).path().to_path_buf();
        self.prompt_name(
            Prompt {
                title: "New directory",
                error_title: "Cannot create directory",
                initial: "",
                selection: 0..0,
                width: PROMPT_WIDTH,
            },
            Rc::new(move |this, name, _, cx| {
                this.commander.update(cx, |commander, cx| {
                    let result = commander
                        .check_dir(&dir)
                        .and_then(|()| commander.make_directory(name));
                    cx.notify();
                    result
                })
            }),
            window,
            cx,
        );
    }

    /// The hotlist's "Add current folder": asks for a name (the folder's
    /// name, preselected) and appends the entry.
    pub(super) fn add_current_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_refused(window, cx) {
            return;
        }
        let path = self.current_folder_entry_path(cx);
        let name = yagni_commander_core::hotlist::default_name(&path);
        self.prompt_name(
            Prompt {
                title: "Add to hotlist",
                error_title: "Cannot add folder",
                initial: &name,
                selection: 0..name.len(),
                width: PROMPT_WIDTH,
            },
            Rc::new(move |this, name, window, cx| {
                let mut list = cx.global::<CurrentConfig>().config.hotlist.clone();
                list.push(yagni_commander_core::config::HotlistEntry {
                    name: name.to_owned(),
                    path: path.clone(),
                });
                this.save_hotlist(list, window, cx);
                Ok(())
            }),
            window,
            cx,
        );
    }

    /// F4: open the entry under the cursor (or the directory, on "..") in
    /// the configured editor.
    pub(super) fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let Some(editor) = configured_editor(window, cx) else {
            return;
        };
        let path = self.active_panel(cx).cursor_path();
        if let Err(e) = launch::open_in_editor(&editor, &path) {
            show_error("Cannot open editor", e.to_string(), None, window, cx);
        }
    }

    /// Ctrl-C/Ctrl-Ins (also Cmd-C on macOS): copy the full path of the
    /// entry under the cursor (or the directory, on "..") as text.
    pub(super) fn copy_path(&mut self, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let path = self.active_panel(cx).cursor_path();
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
    }

    /// Enter (or a double-click): enters a folder, runs a program, or hands
    /// any other file to the system's opener, which picks the application.
    pub(super) fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        let outcome = self.commander.update(cx, |commander, cx| {
            let outcome = commander.execute(Command::Activate);
            cx.notify();
            outcome
        });
        if let Outcome::OpenFile(path) = outcome {
            self.open_file(&path, window, cx);
        }
    }

    /// A program (see `launch::is_program`) runs in its folder; anything
    /// else goes to the opener. Decided on a thread of its own, since
    /// reading the file's first bytes can hang on a dead mount. Errors come
    /// back through a channel polled like file operations: a wake-up from
    /// another thread would bypass gpui's executor.
    fn open_file(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let refocus = Some(self.focus.clone());
        let (failed, failure) = std::sync::mpsc::channel::<String>();
        let (path, opener) = (path.to_path_buf(), self.opener.clone());
        std::thread::spawn(move || {
            let started = if launch::is_program(&path) {
                let dir = path.parent().unwrap_or(&path);
                launch::run_program(&path, dir)
            } else {
                let failed = failed.clone();
                launch::open_with(&opener, &path, move |message| {
                    let _ = failed.send(message);
                })
            };
            if let Err(e) = started {
                let _ = failed.send(e.to_string());
            }
        });
        cx.spawn_in(window, async move |_, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                match failure.try_recv() {
                    Ok(message) => {
                        let _ = cx.update(|window, cx| {
                            show_error("Cannot open file", message, refocus, window, cx)
                        });
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        })
        .detach();
    }

    /// Shift-F4: create a file (or pick an existing one) and open it in the
    /// editor. Asks for the name only if an editor is configured.
    pub(super) fn edit_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let Some(editor) = configured_editor(window, cx) else {
            return;
        };
        let dir = self.active_panel(cx).path().to_path_buf();
        self.prompt_name(
            Prompt {
                title: "Edit new file",
                error_title: "Cannot edit file",
                initial: "",
                selection: 0..0,
                width: PROMPT_WIDTH,
            },
            Rc::new(move |this, name, _, cx| {
                let path = this.commander.update(cx, |commander, cx| {
                    let result = commander
                        .check_dir(&dir)
                        .and_then(|()| commander.create_file(name));
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
        if self.active_loading(cx) {
            return;
        }
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
        let width = prompt.width;
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm = confirm.clone();
            let focus_ok = focus.clone();
            let focus_cancel = focus.clone();
            dialog
                .title(title.clone())
                .w(gpui_kit::px(width))
                .child(dialog_field(&input))
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

/// The `editor` setting, or an error box saying it is missing.
fn configured_editor(window: &mut Window, cx: &mut App) -> Option<String> {
    let editor = cx.global::<CurrentConfig>().config.editor.clone();
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
pub(crate) fn show_error(
    title: &'static str,
    message: impl Into<SharedString>,
    refocus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    show_message(title, message, "Dismiss", refocus, window, cx);
}

/// A centered, modal message box with one `button`, like [`show_error`].
pub(super) fn show_message(
    title: &'static str,
    message: impl Into<SharedString>,
    button: &'static str,
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
    let buttons = ButtonRow::build([(button, dismiss)], 0, cx);
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

/// A one-line text field. gpui-component's default input is 32 px tall
/// with 8 px vertical padding and a 1 px border, which leaves 14 px for a
/// 20 px text line and cuts off descenders (g, j, p, q, y). 5 px padding
/// leaves exactly one line.
pub(crate) fn text_field(state: &gpui_kit::Entity<InputState>) -> Input {
    use gpui_kit::Styled;
    Input::new(state).py(gpui_kit::px(5.0))
}

/// A [`text_field`] at the top or bottom of a dialog body. The body clips
/// its children and has no vertical padding, and gpui-component draws the
/// focus ring 3 px outside the field, so the field needs room above and
/// below or the ring is cut off.
pub(crate) fn dialog_field(state: &gpui_kit::Entity<InputState>) -> gpui_kit::Div {
    use gpui_kit::{ParentElement, Styled};
    gpui_kit::div()
        .py(gpui_kit::px(4.0))
        .child(text_field(state))
}

/// The About box's text.
pub(super) fn about_text() -> String {
    format!(
        "yagni-commander {}\nA dual-pane file manager.",
        env!("CARGO_PKG_VERSION")
    )
}

impl FileManager {
    /// The menu's About item.
    pub(super) fn about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let refocus = Some(self.focus.clone());
        show_message(
            "About yagni-commander",
            about_text(),
            "OK",
            refocus,
            window,
            cx,
        );
    }
}

/// Opening a dialog moves focus to the dialog itself; this moves it on to
/// `handle` (typically the dialog's button row) once the dialog is open.
pub(crate) fn focus_when_open(handle: FocusHandle, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| handle.focus(window, cx));
}

/// The part of a name to preselect for renaming: everything before the last
/// extension for files (like TC), the whole name for directories and for
/// names like `.bashrc` that are all extension.
pub(super) fn stem_range(name: &str, is_dir: bool) -> Range<usize> {
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
