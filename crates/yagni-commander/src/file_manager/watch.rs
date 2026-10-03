//! Feeds the panel watchers' changes to the commander as quiet reloads (see
//! core `watch` and `Commander::watch_reload`). The watchers call back on
//! their own threads; a channel brings each change to one task on the UI
//! thread, which waits without polling.

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui_kit::{App, Context, Window};
use yagni_commander_core::Side;
use yagni_commander_core::watch::PanelWatcher;

use super::FileManager;

const SIDES: [Side; 2] = [Side::Left, Side::Right];

/// One watcher per panel (left, right), each reporting its side.
pub(super) fn spawn_watchers(changes: &UnboundedSender<Side>) -> [PanelWatcher; 2] {
    SIDES.map(|side| {
        let changes = changes.clone();
        PanelWatcher::spawn(move || {
            let _ = changes.unbounded_send(side);
        })
    })
}

impl FileManager {
    /// Turns each reported change into a quiet reload, until the window
    /// closes.
    pub(super) fn receive_changes(
        mut received: UnboundedReceiver<Side>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(side) = received.next().await {
                let alive = this.update(cx, |this, cx| {
                    this.commander.update(cx, |c, cx| {
                        c.watch_reload(side);
                        cx.notify();
                    });
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Points each watcher at its panel's folder, when that changed
    /// (navigation, Ctrl-U, Alt-Z, the fallback to a parent).
    pub(super) fn watch_panels(&mut self, cx: &App) {
        for (i, side) in SIDES.into_iter().enumerate() {
            let path = self.commander.read(cx).panel(side).real_dir();
            if self.watched[i].as_deref() == Some(path) {
                continue;
            }
            self.watched[i] = Some(path.to_path_buf());
            if let Some(watchers) = &self.watchers {
                watchers[i].watch(path.to_path_buf());
            }
        }
    }
}
