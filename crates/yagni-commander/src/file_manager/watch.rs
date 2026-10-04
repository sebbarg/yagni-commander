//! Feeds the folder watchers' reports to the commander as quiet reloads
//! (see core `watch`, `Commander::watch_changed` and
//! `Commander::watch_started`). The watchers call back on their own
//! threads; a channel brings each report to one task on the UI thread,
//! which waits without polling.

use std::path::PathBuf;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui_kit::{App, Context, Window};
use yagni_commander_core::Side;
use yagni_commander_core::watch::{PanelWatcher, Report};

use super::FileManager;

/// Two watchers, enough for the two front tabs. Each report names its
/// folder, so a watcher isn't tied to a side.
pub(super) fn spawn_watchers(reports: &UnboundedSender<Report>) -> [PanelWatcher; 2] {
    [(), ()].map(|_| {
        let reports = reports.clone();
        PanelWatcher::spawn(move |report| {
            let _ = reports.unbounded_send(report);
        })
    })
}

impl FileManager {
    /// Turns each report into quiet reloads, until the window closes.
    pub(super) fn receive_changes(
        mut received: UnboundedReceiver<Report>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(report) = received.next().await {
                let alive = this.update(cx, |this, cx| {
                    this.commander.update(cx, |c, cx| {
                        match report {
                            Report::Changed(dir) => c.watch_changed(&dir),
                            Report::Started(dir, modified) => c.watch_started(&dir, modified),
                        }
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

    /// Points the watchers at the front tabs' folders, when those changed
    /// (navigation, a tab switch, the fallback to a parent). A watcher
    /// already on a needed folder keeps it, so Ctrl-U moves no watch and
    /// loses no change; a folder shown on both sides is watched once.
    pub(super) fn watch_panels(&mut self, cx: &App) {
        let commander = self.commander.read(cx);
        let mut needed: Vec<PathBuf> = Vec::new();
        for side in [Side::Left, Side::Right] {
            let panel = commander.panel(side);
            // Not before its first listing (a restored tab): the folder may
            // be on a dead mount, and the read decides where the tab goes.
            if !panel.is_loaded() {
                continue;
            }
            let dir = panel.real_dir();
            if !needed.iter().any(|d| d == dir) {
                needed.push(dir.to_path_buf());
            }
        }
        let mut claimed = [false; 2];
        let mut unwatched = Vec::new();
        for dir in needed {
            match (0..2).find(|&i| !claimed[i] && self.watched[i].as_ref() == Some(&dir)) {
                Some(i) => claimed[i] = true,
                None => unwatched.push(dir),
            }
        }
        for dir in unwatched {
            let Some(i) = (0..2).find(|&i| !claimed[i]) else {
                break;
            };
            claimed[i] = true;
            if let Some(watchers) = &mut self.watchers {
                watchers[i].watch(dir.clone());
            }
            self.watched[i] = Some(dir);
        }
    }
}
