//! Shift-Enter: Open with. The applications are looked up on a thread of
//! their own (reading the file's first bytes could hang on a dead mount),
//! the result comes back over a std channel polled like the opener's errors,
//! and the popup opens if the cursor is still on that file and nothing else
//! opened meanwhile.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;

use gpui_kit::component::WindowExt;
use gpui_kit::{
    AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Styled, Subscription,
    Window, anchored, deferred, div, px,
};
use yagni_commander_core::apps::{self, App, Found};
use yagni_commander_core::{EntryKind, Side};

use super::FileManager;
use super::commands::{OPENER_POLL, report_failure, show_error, show_message};
use crate::open_with_popup::{OpenWithEvent, OpenWithPopup};
use crate::zoom::rems_from_px;

/// The open popup and the side it belongs to.
pub(crate) struct OpenApps {
    pub side: Side,
    pub popup: Entity<OpenWithPopup>,
    _subscriptions: Vec<Subscription>,
}

/// Finds the applications for a file; app tests replace the system's.
pub(crate) type Lookup = Arc<dyn Fn(&Path) -> io::Result<Found> + Send + Sync>;

impl FileManager {
    /// Shift-Enter: looks up the applications for the file under the
    /// cursor. Folders and ".." do nothing; inside an archive it is refused.
    pub(super) fn open_with(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        if self
            .active_panel(cx)
            .cursor_entry()
            .is_none_or(|e| e.kind != EntryKind::File)
        {
            return;
        }
        if self.refuse_read_only(window, cx) {
            return;
        }
        let side = self.commander.read(cx).active();
        let path = self.active_panel(cx).cursor_path();
        self.next_lookup += 1;
        let id = self.next_lookup;
        let lookup = self.apps_lookup.clone();
        let (sender, received) = std::sync::mpsc::channel();
        std::thread::spawn({
            let path = path.clone();
            move || {
                let found = match lookup {
                    Some(lookup) => lookup(&path),
                    None => apps::apps_for(&path),
                };
                let _ = sender.send(found);
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                match received.try_recv() {
                    Ok(found) => {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.found_apps(id, side, &path, found, window, cx)
                        });
                        break;
                    }
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }
            }
        })
        .detach();
    }

    /// The lookup `id` for `path` on `side` finished.
    fn found_apps(
        &mut self,
        id: u64,
        side: Side,
        path: &Path,
        found: io::Result<Found>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commander = self.commander.read(cx);
        let still_there = id == self.next_lookup
            && commander.active() == side
            && commander.panel(side).loading().is_none()
            && commander.panel(side).cursor_path() == path;
        let busy = self.hotlist.is_some()
            || self.mounts.is_some()
            || self.open_with.is_some()
            || self.menu_open(cx)
            || window.has_active_dialog(cx)
            || !self.focus.is_focused(window);
        if !still_there || busy {
            return;
        }
        let refocus = Some(self.focus.clone());
        let found = match found {
            Ok(found) => found,
            Err(e) => {
                show_error("Cannot open with", e.to_string(), refocus, window, cx);
                return;
            }
        };
        if found.apps.is_empty() {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            );
            let kind = found.kind.map(|k| format!(" ({k})")).unwrap_or_default();
            let text = format!("No application is registered for “{name}”{kind}.");
            show_message("Open with", text, "OK", refocus, window, cx);
            return;
        }
        let popup = cx.new(|cx| OpenWithPopup::new(found.apps, cx));
        let picked = cx.subscribe_in(&popup, window, |this, _, event, window, cx| {
            this.on_open_with(event, window, cx)
        });
        let focus = popup.focus_handle(cx);
        // Like the other popups: focus went elsewhere, just drop it.
        let blurred = cx.on_focus_out(&focus, window, |this, _, _, cx| {
            if this.open_with.take().is_some() {
                cx.notify();
            }
        });
        focus.focus(window, cx);
        self.open_with = Some(OpenApps {
            side,
            popup,
            _subscriptions: vec![picked, blurred],
        });
        cx.notify();
    }

    /// Closes the popup; focus goes back to the panels.
    pub(super) fn close_open_with(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_with.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn on_open_with(&mut self, event: &OpenWithEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.close_open_with(window, cx);
        if let OpenWithEvent::Pick(app) = event {
            self.start_app(app.clone(), window, cx);
        }
    }

    /// Starts `app` on a thread (its folder could be on a dead mount).
    fn start_app(&mut self, app: App, window: &mut Window, cx: &mut Context<Self>) {
        let (failed, failure) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            if let Err(e) = apps::start(&app) {
                let _ = failed.send(e.to_string());
            }
        });
        let refocus = Some(self.focus.clone());
        report_failure(failure, "Cannot open file", refocus, window, cx);
    }

    /// The popup, under the cursor row of `side`.
    pub(super) fn open_with_overlay(
        &self,
        side: Side,
        cx: &gpui_kit::App,
    ) -> Option<impl IntoElement + use<>> {
        let open = self.open_with.as_ref().filter(|open| open.side == side)?;
        let view = match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        };
        let top = view.read(cx).below_cursor(cx);
        Some(
            div()
                .absolute()
                .top(px(top))
                .left(rems_from_px(16.0))
                .child(deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.0))
                        .child(open.popup.clone()),
                )),
        )
    }
}

/// The file a lookup was for (tests).
#[cfg(test)]
pub(crate) fn lookup_from(
    apps: Vec<App>,
    seen: Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
) -> Lookup {
    Arc::new(move |path: &Path| {
        seen.lock().unwrap().push(path.to_path_buf());
        Ok(Found {
            apps: apps.clone(),
            kind: Some("text/plain".into()),
        })
    })
}
