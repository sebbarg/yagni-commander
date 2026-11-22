//! The shared clipboard (Ctrl-K Ctrl-C, Ctrl-K Ctrl-V; core
//! [`shared_clip`]). Every step on the host runs on a `std::thread` through
//! `remote::with_session`, so the connection, its questions and the shared
//! session are those of remote folders; the result comes back over a std
//! channel polled every `OPENER_POLL`, like Open with's lookup. The items
//! show in a dialog: the list ([`SharedClipList`]) and View, Delete,
//! Close, Copy.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;

use gpui_kit::component::WindowExt;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, ClipboardItem, Context, Entity, Focusable, ParentElement,
    SharedString, Window, div,
};
use yagni_commander_core::sftp::Client;
use yagni_commander_core::{remote, shared_clip};

use super::FileManager;
use super::commands::{OPENER_POLL, focus_when_open, show_error, show_message, themed_dialog};
use crate::button_row::{ButtonRow, OnPress, dialog_keys};
use crate::config_state::CurrentConfig;
use crate::shared_clip_dialog::{SharedClipEvent, SharedClipList};

const TITLE: &str = "Shared clipboard";

#[derive(Default)]
pub(crate) struct SharedClip {
    /// The folder on the host in place of the one in the login home (app
    /// tests).
    pub dir: Option<Vec<u8>>,
    /// The machine named in pushed items in place of this computer's (app
    /// tests).
    pub machine: Option<String>,
    /// Number of the last list asked for; an older one's result is dropped.
    next_list: u64,
    /// The status line's word on the last push or pick, until the next
    /// command.
    pub status: Option<SharedString>,
    /// What runs on the host now ("Pushing to box..."), shown before
    /// `status` and kept through other commands until the step ends.
    pub progress: Option<SharedString>,
    /// The open dialog's list.
    pub dialog: Option<Entity<SharedClipList>>,
}

impl FileManager {
    /// The host in Settings, or why there is none (an error box's text).
    fn shared_host(cx: &gpui_kit::App) -> Result<String, String> {
        let host = cx.global::<CurrentConfig>().config.shared_clipboard.clone();
        match host {
            None => Err("Set a host for the shared clipboard in Settings.".into()),
            Some(host) if !remote::valid_host(&host) => {
                Err(format!("“{host}” is not a host name."))
            }
            Some(host) => Ok(host),
        }
    }

    /// Runs `step` on the shared clipboard's folder on `host` on a thread,
    /// then `done` with its result in `window`.
    fn on_host<T: Send + 'static>(
        &self,
        host: &str,
        mut step: impl FnMut(&mut Client, &[u8]) -> io::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, io::Result<T>, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (sender, received) = std::sync::mpsc::channel();
        let dir = self.shared.dir.clone();
        let host = host.to_owned();
        std::thread::spawn(move || {
            let result = remote::with_session(&host, |client| {
                let dir = match &dir {
                    Some(dir) => dir.clone(),
                    None => shared_clip::folder(client)?,
                };
                step(client, &dir)
            });
            let _ = sender.send(result);
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                match received.try_recv() {
                    Ok(result) => {
                        let _ =
                            this.update_in(cx, |this, window, cx| done(this, result, window, cx));
                        break;
                    }
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }
            }
        })
        .detach();
    }

    /// Shows `text` while a step runs on the host.
    fn start_progress(&mut self, text: String, cx: &mut Context<Self>) -> SharedString {
        let text: SharedString = text.into();
        self.shared.progress = Some(text.clone());
        cx.notify();
        text
    }

    /// The step that showed `text` ended: it goes, unless a newer step's
    /// replaced it.
    fn end_progress(&mut self, text: &SharedString, cx: &mut Context<Self>) {
        if self.shared.progress.as_ref() == Some(text) {
            self.shared.progress = None;
            cx.notify();
        }
    }

    /// An error box in `target` (the window the keys were pressed in), or
    /// in this one, deferred: that window may be dispatching.
    fn shared_error(
        &self,
        message: String,
        target: Option<AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let main = window.window_handle();
        let target = target.unwrap_or(main);
        let refocus = (target == main).then(|| self.focus.clone());
        cx.defer(move |cx| {
            let _ = target.update(cx, |_, window, cx| {
                show_error(TITLE, message, refocus, window, cx)
            });
        });
    }

    /// Ctrl-K Ctrl-C, from any window (`target`, where errors show): the
    /// system clipboard's text to the host.
    pub(crate) fn push_shared(
        &mut self,
        target: Option<AnyWindowHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = match Self::shared_host(cx) {
            Ok(host) => host,
            Err(why) => return self.shared_error(why, target, window, cx),
        };
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return self.shared_error("The clipboard holds no text.".into(), target, window, cx);
        };
        if text.len() as u64 > shared_clip::ITEM_CAP {
            let why = "The text is over 1 MiB, too large for the shared clipboard.";
            return self.shared_error(why.into(), target, window, cx);
        }
        let machine = self
            .shared
            .machine
            .clone()
            .unwrap_or_else(shared_clip::machine);
        let label = host.clone();
        let progress = self.start_progress(format!("Pushing to {host}..."), cx);
        self.on_host(
            &host,
            move |client, dir| shared_clip::push(client, dir, &text, &machine),
            move |this, result, window, cx| {
                this.end_progress(&progress, cx);
                match result {
                    Ok(_) => {
                        this.shared.status = Some(format!("Pushed to {label}").into());
                        cx.notify();
                    }
                    Err(e) => this.shared_error(e.to_string(), target, window, cx),
                }
            },
            window,
            cx,
        );
    }

    /// Ctrl-K Ctrl-V: reads the host's items, then shows them in a
    /// dialog, unless a newer list was asked for or something else opened
    /// meanwhile.
    pub(super) fn open_shared(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let host = match Self::shared_host(cx) {
            Ok(host) => host,
            Err(why) => return self.shared_error(why, None, window, cx),
        };
        self.shared.next_list += 1;
        let id = self.shared.next_list;
        let label = host.clone();
        let progress =
            self.start_progress(format!("Reading the shared clipboard on {host}..."), cx);
        self.on_host(
            &host,
            shared_clip::list,
            move |this, result, window, cx| {
                this.end_progress(&progress, cx);
                this.listed(id, label, result, window, cx)
            },
            window,
            cx,
        );
    }

    fn listed(
        &mut self,
        id: u64,
        host: String,
        result: io::Result<Vec<shared_clip::Item>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let busy = self.popup_open()
            || self.menu_open(cx)
            || window.has_active_dialog(cx)
            || !self.focus.is_focused(window);
        if id != self.shared.next_list || busy {
            return;
        }
        let items = match result {
            Ok(items) => items,
            Err(e) => return self.shared_error(e.to_string(), None, window, cx),
        };
        if items.is_empty() {
            let refocus = Some(self.focus.clone());
            let text = "The shared clipboard is empty";
            show_message(TITLE, text, "OK", refocus, window, cx);
            return;
        }
        let list = cx.new(|cx| SharedClipList::new(items, cx));
        self.shared.dialog = Some(list.clone());
        let me = cx.entity().downgrade();
        let focus = self.focus.clone();

        let close = {
            let (me, focus) = (me.clone(), focus.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |fm, _| fm.shared.dialog = None);
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
            })
        };
        // Copy: the highlighted item to the system clipboard. Returns
        // whether the dialog closes.
        let copy = {
            let (me, list, close, host) = (me.clone(), list.clone(), close.clone(), host.clone());
            Rc::new(move |window: &mut Window, cx: &mut App| -> bool {
                let Some(name) = list.read(cx).current() else {
                    return false;
                };
                close(window, cx);
                let host = host.clone();
                let _ = me.update(cx, |fm, cx| fm.copy_shared(&host, name, window, cx));
                true
            })
        };
        let view_button: OnPress = Rc::new({
            let (me, list, host) = (me.clone(), list.clone(), host.clone());
            move |window, cx| {
                if let Some(name) = list.read(cx).current() {
                    let _ = me.update(cx, |fm, cx| fm.view_shared(&host, name, window, cx));
                }
            }
        });
        let delete_button: OnPress = Rc::new({
            let list = list.clone();
            move |_, cx| list.update(cx, |list, cx| list.remove(cx))
        });
        let close_button: OnPress = Rc::new({
            let close = close.clone();
            move |window, cx| {
                close(window, cx);
                window.close_dialog(cx);
            }
        });
        let copy_button: OnPress = Rc::new({
            let copy = copy.clone();
            move |window, cx| {
                if copy(window, cx) {
                    window.close_dialog(cx);
                }
            }
        });
        let buttons = ButtonRow::build(
            [
                ("&View", view_button),
                ("&Delete", delete_button),
                ("Close", close_button),
                ("Copy", copy_button),
            ],
            3,
            cx,
        );
        let events = cx.subscribe_in(&list, window, {
            let (buttons, copy) = (buttons.clone(), copy.clone());
            move |this, list, event: &SharedClipEvent, window, cx| match event {
                SharedClipEvent::View(name) => this.view_shared(&host, name.clone(), window, cx),
                SharedClipEvent::Remove(name) => {
                    this.remove_shared(&host, name.clone(), window, cx);
                    if list.read(cx).len() == 0 {
                        buttons.update(cx, |row, cx| {
                            for button in [0, 1, 3] {
                                row.set_enabled(button, false);
                            }
                            cx.notify();
                        });
                    }
                }
                // Deferred: `copy` updates the file manager, which this
                // handler is already updating.
                SharedClipEvent::Pick => {
                    let copy = copy.clone();
                    window.defer(cx, move |window, cx| {
                        if copy(window, cx) {
                            window.close_dialog(cx);
                        }
                    });
                }
            }
        });
        // Dropped with the list, when the dialog closes.
        list.update(cx, |list, _| list.keep(events));
        focus_when_open(list.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let (copy, close) = (copy.clone(), close.clone());
            themed_dialog(dialog, cx)
                .title(TITLE)
                .w(crate::zoom::dialog_width(640.0, cx))
                .close_button(false)
                // Alt-V and Alt-D from the list too.
                .child(dialog_keys(div().child(list.clone()), &buttons, None))
                .footer(buttons.clone())
                // Enter in the list.
                .on_ok(move |_, window, cx| copy(window, cx))
                .on_cancel(move |_, window, cx| {
                    close(window, cx);
                    true
                })
        });
    }

    /// Fetches item `name` and puts it on the system clipboard.
    fn copy_shared(
        &mut self,
        host: &str,
        name: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = host.to_owned();
        let progress = self.start_progress(format!("Copying from {host}..."), cx);
        self.on_host(
            host,
            move |client, dir| shared_clip::fetch(client, dir, &name),
            move |this, result, window, cx| {
                this.end_progress(&progress, cx);
                match result {
                    Ok(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        this.shared.status = Some(format!("Copied from {label}").into());
                        cx.notify();
                    }
                    Err(e) => this.shared_error(e.to_string(), None, window, cx),
                }
            },
            window,
            cx,
        );
    }

    /// F3 or View: fetches item `name` into a private folder and opens the
    /// viewer on it, titled with the item's place on the host. The dialog
    /// stays open.
    fn view_shared(
        &mut self,
        host: &str,
        name: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let progress = self.start_progress(format!("Fetching from {host}..."), cx);
        let label = host.to_owned();
        let file_name = name.clone();
        self.on_host(
            host,
            move |client, dir| {
                let text = shared_clip::fetch(client, dir, &name)?;
                Ok((dir.to_vec(), text))
            },
            move |this, result, window, cx| {
                this.end_progress(&progress, cx);
                let opened = result.and_then(|(dir, text)| {
                    let root = this
                        .temp_dir
                        .clone()
                        .ok_or_else(|| io::Error::other("no folder for temporary files"))?;
                    yagni_commander_core::storage::private_dir(&root)?;
                    let temp = tempfile::Builder::new().prefix("view-").tempdir_in(&root)?;
                    let file = temp.path().join(std::ffi::OsStr::from_bytes(&file_name));
                    std::fs::write(&file, text)?;
                    let shown = remote::path_of(&label, &[&dir[..], b"/", &file_name].concat());
                    crate::viewer_view::open(
                        file,
                        &shown,
                        window.window_bounds(),
                        Some(temp),
                        None,
                        cx,
                    )
                });
                if let Err(e) = opened {
                    this.shared_error(e.to_string(), None, window, cx);
                }
            },
            window,
            cx,
        );
    }

    /// Deletes item `name` on the host; a failure shows a box.
    fn remove_shared(
        &mut self,
        host: &str,
        name: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_host(
            host,
            move |client, dir| shared_clip::remove(client, dir, &name),
            |this, result, window, cx| {
                if let Err(e) = result {
                    this.shared_error(e.to_string(), None, window, cx);
                }
            },
            window,
            cx,
        );
    }
}
