//! Alt-Shift-Enter: counts the size of every folder in the active panel,
//! one after the other on a thread of its own (core `info::count`: links
//! not followed, other filesystems skipped). Each size goes into the panel
//! as it arrives (core `Commander::set_dir_size`); the status line counts
//! the folders done. Escape, another count, or the panel leaving the
//! folder stops it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::{Context, SharedString, Window};
use yagni_commander_core::PanelId;
use yagni_commander_core::info::{self, Progress};

use super::FileManager;
use super::commands::OPENER_POLL;

/// The count in progress.
pub(crate) struct RunningSizes {
    id: u64,
    /// The tab and folder counted.
    panel: PanelId,
    dir: PathBuf,
    total: usize,
    done: usize,
    sizes: Receiver<(OsString, Option<u64>)>,
    cancel: Arc<AtomicBool>,
}

#[cfg(test)]
impl RunningSizes {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl FileManager {
    /// Starts counting the active panel's folders. Refused inside an
    /// archive and on a remote folder.
    pub(super) fn count_folder_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        if self.refuse_read_only(window, cx) {
            return;
        }
        self.stop_sizes(cx);
        let panel = self.active_panel(cx);
        let names = panel.dir_names();
        if names.is_empty() {
            return;
        }
        let (panel, dir) = (panel.id(), panel.path().to_path_buf());
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, sizes) = std::sync::mpsc::channel();
        let total = names.len();
        std::thread::spawn({
            let (dir, cancel) = (dir.clone(), cancel.clone());
            move || {
                for name in names {
                    let totals = info::count(&dir.join(&name), &Progress::default(), &cancel);
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    if send.send((name, totals.map(|t| t.bytes))).is_err() {
                        return;
                    }
                }
            }
        });
        self.next_sizes += 1;
        let id = self.next_sizes;
        self.sizes = Some(RunningSizes {
            id,
            panel,
            dir,
            total,
            done: 0,
            sizes,
            cancel,
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                let running = this.update(cx, |this, cx| this.poll_sizes(id, cx));
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes in the sizes counted so far. Returns whether count `id` goes
    /// on.
    fn poll_sizes(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.sizes.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        let mut arrived = Vec::new();
        // Messages sent before the thread ended come first.
        let finished = loop {
            match running.sizes.try_recv() {
                Ok(size) => arrived.push(size),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        running.done += arrived.len();
        let (panel, dir) = (running.panel, running.dir.clone());
        if !arrived.is_empty() {
            self.commander.update(cx, |c, cx| {
                for (name, bytes) in arrived {
                    // An unreadable start or a folder gone meanwhile: no size.
                    if let Some(bytes) = bytes {
                        c.set_dir_size(panel, &dir, &name, bytes);
                    }
                }
                cx.notify();
            });
        }
        if finished || !self.shows_counted(cx) {
            self.stop_sizes(cx);
            return false;
        }
        true
    }

    /// Whether the counted tab still shows the counted folder.
    fn shows_counted(&self, cx: &gpui_kit::App) -> bool {
        let Some(running) = &self.sizes else {
            return false;
        };
        let commander = self.commander.read(cx);
        [
            yagni_commander_core::Side::Left,
            yagni_commander_core::Side::Right,
        ]
        .iter()
        .flat_map(|&side| commander.tabs(side).iter())
        .any(|panel| panel.id() == running.panel && panel.path() == running.dir)
    }

    /// Stops the count, if one runs.
    pub(super) fn stop_sizes(&mut self, cx: &mut Context<Self>) {
        if let Some(running) = self.sizes.take() {
            running.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    /// The status line while counting.
    pub(super) fn sizes_text(&self) -> Option<SharedString> {
        let running = self.sizes.as_ref()?;
        Some(
            format!(
                "Counting folder sizes: {} of {}  (Escape stops)",
                running.done, running.total
            )
            .into(),
        )
    }
}
