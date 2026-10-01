//! Runs the commander's directory reads off the UI thread (see
//! `Requirements.md`, Directory loading). One plain thread per read, not
//! gpui's background pool: a read on a dead network mount may never return,
//! and must not hold a pool thread. A 10 ms timer collects the results while
//! any read runs.

use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use gpui_kit::{Context, Window};
use yagni_commander_core::{Listing, LoadRequest, read_listing};

use super::FileManager;

const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Polls before the "Loading..." indicator shows (about 150 ms).
const SHOW_AFTER_POLLS: u32 = 15;
/// Polls between updates of the indicator's count (about 100 ms).
const COUNT_EVERY_POLLS: u32 = 10;

/// Starts one read; its result arrives on the receiver.
pub(crate) type LoadFn = fn(LoadRequest) -> Receiver<io::Result<Listing>>;

/// The real loader: one thread per read.
pub(crate) fn spawn_load(request: LoadRequest) -> Receiver<io::Result<Listing>> {
    let (tx, rx) = mpsc::channel();
    // If the thread can't start, `tx` is dropped with the closure and the
    // poll reports the read as failed.
    let _ = std::thread::Builder::new()
        .name("dir-load".into())
        .spawn(move || {
            let _ = tx.send(read_listing(&request));
        });
    rx
}

pub(crate) struct RunningLoad {
    id: u64,
    rx: Receiver<io::Result<Listing>>,
    polls: u32,
    /// The count the indicator last showed.
    shown: Option<usize>,
}

impl FileManager {
    /// Starts the reads the commander asked for. Without a loader (tests),
    /// reads them right here.
    pub(super) fn start_loads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let requests = self.commander.update(cx, |c, _| c.take_requests());
        if requests.is_empty() {
            return;
        }
        let Some(load) = self.load else {
            self.commander.update(cx, |c, cx| {
                for request in requests {
                    let result = read_listing(&request);
                    c.finish_load(request.id, result);
                }
                c.run_loads_now();
                cx.notify();
            });
            return;
        };
        for request in requests {
            let id = request.id;
            self.loads.push(RunningLoad {
                id,
                rx: load(request),
                polls: 0,
                shown: None,
            });
        }
        if self.polling_loads {
            return;
        }
        self.polling_loads = true;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let polling = this.update(cx, |this, cx| this.poll_loads(cx));
                if !matches!(polling, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Hands finished reads to the commander. Returns whether any still run.
    fn poll_loads(&mut self, cx: &mut Context<Self>) -> bool {
        let commander = self.commander.clone();
        let mut changed = false;
        self.loads.retain_mut(|load| {
            if !commander.read(cx).is_pending(load.id) {
                return false; // cancelled or replaced: its thread ends on its own
            }
            let result = match load.rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Disconnected) => {
                    Err(io::Error::other("the directory read stopped"))
                }
                Err(TryRecvError::Empty) => {
                    // Redraw only for news: the indicator appearing, or a new
                    // count (at most every 100 ms). A hung read stays quiet.
                    load.polls += 1;
                    if load.polls == SHOW_AFTER_POLLS {
                        commander.update(cx, |c, _| c.show_loading(load.id));
                    }
                    let due = load.polls >= SHOW_AFTER_POLLS
                        && (load.polls - SHOW_AFTER_POLLS).is_multiple_of(COUNT_EVERY_POLLS);
                    if due {
                        let count = commander.read(cx).panel_loading_count(load.id);
                        if count != load.shown {
                            load.shown = count;
                            changed = true;
                        }
                    }
                    return true;
                }
            };
            commander.update(cx, |c, _| c.finish_load(load.id, result));
            changed = true;
            false
        });
        if changed {
            commander.update(cx, |_, cx| cx.notify());
        }
        self.polling_loads = !self.loads.is_empty();
        self.polling_loads
    }
}
