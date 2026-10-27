//! Ctrl-F: the Connect dialog ([`ConnectDialog`]) and what its choice does.

use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::{App, AppContext, Context, Focusable, ParentElement, WeakEntity, Window};
use yagni_commander_core::remote;

use super::FileManager;
use super::commands::{focus_when_open, show_error, themed_dialog};
use crate::button_row::{ButtonRow, OnPress};
use crate::connect_dialog::ConnectDialog;

impl FileManager {
    /// Ctrl-F: the Connect dialog over the hosts of `~/.ssh/config`.
    pub(crate) fn open_connect_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let home = self
            .ssh_home
            .clone()
            .unwrap_or_else(|| self.commander.read(cx).home().to_path_buf());
        let hosts = yagni_commander_core::ssh_config::hosts(&home);
        let view = cx.new(|cx| ConnectDialog::new(hosts, window, cx));
        self.connect_dialog = Some(view.clone());
        let me: WeakEntity<FileManager> = cx.entity().downgrade();
        let focus = self.focus.clone();

        // Connect: go to the host's login folder. Returns whether it closed.
        let connect = {
            let (view, me, focus) = (view.clone(), me.clone(), focus.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| -> bool {
                let Some(host) = view.read(cx).target() else {
                    return false;
                };
                if !remote::valid_host(&host) {
                    let field = view.read(cx).field.focus_handle(cx);
                    show_error(
                        "Cannot connect",
                        format!("“{host}” is not a host name."),
                        Some(field),
                        window,
                        cx,
                    );
                    return false;
                }
                let _ = me.update(cx, |fm, cx| {
                    fm.connect_dialog = None;
                    fm.commander.update(cx, |c, cx| {
                        c.go_to(remote::path_of(&host, b""));
                        cx.notify();
                    });
                });
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
                true
            })
        };
        let cancel = {
            let (me, focus) = (me.clone(), focus.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |fm, _| fm.connect_dialog = None);
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
            })
        };
        let connect_button: OnPress = Rc::new({
            let connect = connect.clone();
            move |window, cx| {
                if connect(window, cx) {
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
            [("Cancel", cancel_button), ("Connect", connect_button)],
            1,
            cx,
        );
        focus_when_open(view.read(cx).field.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let (connect, cancel) = (connect.clone(), cancel.clone());
            themed_dialog(dialog, cx)
                .title("Connect")
                .w(crate::zoom::dialog_width(560.0, cx))
                .close_button(false)
                .child(view.clone())
                .footer(buttons.clone())
                // Enter anywhere in the dialog, a double click on a row.
                .on_ok(move |_, window, cx| connect(window, cx))
                .on_cancel(move |_, window, cx| {
                    cancel(window, cx);
                    true
                })
        });
    }
}
