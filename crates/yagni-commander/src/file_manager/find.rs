//! Alt-F7: opens the find dialog (`crate::find_dialog`) and carries out
//! what it asks for: go to a result, view it, or feed the results to the
//! active panel.

use crate::file_manager::commands::themed_dialog;
use gpui_kit::component::WindowExt;
use gpui_kit::{AppContext, Context, ParentElement, Styled, Window, div};

use crate::button_row::{OnLetter, dialog_keys};
use std::rc::Rc;

use super::FileManager;
use super::commands::show_error;
use crate::find_dialog::{FindDialog, FindEvent};

impl FileManager {
    /// Alt-F7: the find dialog, with the last search. Ignored while the
    /// active panel is loading.
    pub(super) fn find_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        if self.active_panel(cx).is_remote() && self.refuse_read_only(window, cx) {
            return;
        }
        let view = match &self.find {
            Some(view) => view.clone(),
            None => {
                let home = self.commander.read(cx).home().to_path_buf();
                let view = cx.new(|cx| FindDialog::new(home, window, cx));
                cx.subscribe_in(&view, window, |this, _, event: &FindEvent, window, cx| {
                    this.on_find(event, window, cx)
                })
                .detach();
                self.find = Some(view.clone());
                view
            }
        };
        let folder = self.active_panel(cx).real_dir().to_path_buf();
        view.update(cx, |v, cx| v.reopen(folder, window, cx));
        let buttons = view.read(cx).buttons.clone();
        let masks = view.read(cx).masks_focus(cx);
        let focus = self.focus.clone();
        let alt: OnLetter = Rc::new({
            let view = view.clone();
            move |letter, window, cx| view.update(cx, |v, cx| v.alt(letter, window, cx))
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let (search, cancel, focus) = (view.clone(), view.clone(), focus.clone());
            let width = view.read(cx).width(cx);
            themed_dialog(dialog, cx)
                .title("Find files")
                .w(width)
                .close_button(false)
                .child(dialog_keys(
                    div().child(view.clone()),
                    &buttons,
                    Some(alt.clone()),
                ))
                .footer(
                    dialog_keys(div(), &buttons, Some(alt.clone()))
                        .relative()
                        .child(buttons.clone())
                        .child(crate::find_dialog::grip(&view, cx)),
                )
                // Enter in a field: search; the dialog stays.
                .on_ok(move |_, window, cx| {
                    search.update(cx, |v, cx| v.search(window, cx));
                    false
                })
                // Escape stops a running search, else closes.
                .on_cancel(move |_, window, cx| {
                    if cancel.read(cx).running() {
                        cancel.update(cx, |v, cx| v.stop(cx));
                        return false;
                    }
                    let focus = focus.clone();
                    window.defer(cx, move |window, cx| focus.focus(window, cx));
                    true
                })
        });
        window.defer(cx, move |window, cx| masks.focus(window, cx));
    }

    fn on_find(&mut self, event: &FindEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            FindEvent::GoTo(path) => {
                self.close_find(window, cx);
                self.commander.update(cx, |c, cx| {
                    c.go_to_file(path);
                    cx.notify();
                });
            }
            FindEvent::View(path, text) => {
                let main = window.window_bounds();
                let text = text.clone();
                if let Err(e) = crate::viewer_view::open(path.clone(), main, None, text, cx) {
                    let back = window.focused(cx);
                    show_error("Cannot view file", e.to_string(), back, window, cx);
                }
            }
            FindEvent::Feed(results) => {
                self.close_find(window, cx);
                let results = results.clone();
                self.commander.update(cx, |c, cx| {
                    c.feed(results);
                    cx.notify();
                });
            }
            FindEvent::Closed => self.close_find(window, cx),
        }
    }

    /// Closes the find dialog (stopping its search) and gives the panels
    /// the keys back.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.find {
            view.update(cx, |v, cx| v.stop(cx));
        }
        window.close_dialog(cx);
        let focus = self.focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }
}
