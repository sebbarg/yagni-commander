//! Runs the commander's directory reads off the UI thread (see
//! `Requirements.md`, Directory loading). One plain thread per read, not
//! gpui's background pool: a read on a dead network mount may never return,
//! and must not hold a pool thread. A timer collects the results while any
//! read runs: every 10 ms at first, every 100 ms once a read is slow (as
//! often as the indicator's count changes), so a hung read costs little.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use gpui_kit::{Context, Window};
use yagni_commander_core::{Listing, LoadRequest, read_listing};

use super::FileManager;

const FAST_POLL: Duration = Duration::from_millis(10);
const SLOW_POLL: Duration = Duration::from_millis(100);
/// How long a read runs before the "Loading..." indicator shows; polling
/// slows down then too.
const SHOW_AFTER: Duration = Duration::from_millis(150);
/// A quiet re-read that took this long makes the next one of that folder
/// wait as long again (see [`rest`]).
const SLOW_READ: Duration = Duration::from_millis(200);

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
    /// A quiet re-read of this folder: its time is kept for [`rest`].
    quiet: Option<PathBuf>,
    state: State,
    /// Whether the indicator was asked for.
    indicated: bool,
    /// The count the indicator last showed.
    shown: Option<usize>,
}

enum State {
    /// Held back until then (see [`rest`]).
    Waiting(LoadRequest, Instant),
    Running(Receiver<io::Result<Listing>>, Instant),
}

/// When a quiet re-read of a folder may start. A watcher reloads a busy
/// folder about once a second; if reading it takes longer, the re-reads
/// would run back to back. So after a slow quiet read, the next one waits
/// as long as that one took: at most half the time is spent reading.
fn rest(last: Option<(Instant, Duration)>, now: Instant) -> Instant {
    match last {
        Some((ended, took)) if took >= SLOW_READ => (ended + took).max(now),
        _ => now,
    }
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
        let now = cx.background_executor().now();
        for request in requests {
            let quiet = request.quiet.then(|| request.path.clone());
            let start = match &quiet {
                Some(path) => rest(self.quiet_reads.get(path).copied(), now),
                None => now,
            };
            let id = request.id;
            let state = if start <= now {
                State::Running(load(request), now)
            } else {
                State::Waiting(request, start)
            };
            self.loads.push(RunningLoad {
                id,
                quiet,
                state,
                indicated: false,
                shown: None,
            });
        }
        if self.polling_loads {
            return;
        }
        self.polling_loads = true;
        cx.spawn_in(window, async move |this, cx| {
            let mut wait = FAST_POLL;
            loop {
                cx.background_executor().timer(wait).await;
                let next = this.update(cx, |this, cx| this.poll_loads(cx));
                match next {
                    Ok(Some(next)) => wait = next,
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// Starts held-back reads that are due and hands finished ones to the
    /// commander. Returns when to poll again, or `None` once no read runs.
    fn poll_loads(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let commander = self.commander.clone();
        let now = cx.background_executor().now();
        let load = self.load.unwrap_or(spawn_load);
        let mut changed = false;
        let mut finished_quiet = Vec::new();
        self.loads.retain_mut(|running| {
            if !commander.read(cx).is_pending(running.id) {
                return false; // cancelled or replaced: its thread ends on its own
            }
            if let State::Waiting(request, at) = &running.state {
                if now < *at {
                    return true;
                }
                running.state = State::Running(load(request.clone()), now);
            }
            let State::Running(rx, started) = &running.state else {
                unreachable!("started above");
            };
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Disconnected) => {
                    Err(io::Error::other("the directory read stopped"))
                }
                Err(TryRecvError::Empty) => {
                    // Redraw only for news: the indicator appearing, or a new
                    // count (at most every 100 ms). A hung read stays quiet.
                    if now.saturating_duration_since(*started) >= SHOW_AFTER {
                        if !running.indicated {
                            running.indicated = true;
                            commander.update(cx, |c, _| c.show_loading(running.id));
                        }
                        let count = commander.read(cx).panel_loading_count(running.id);
                        if count != running.shown {
                            running.shown = count;
                            changed = true;
                        }
                    }
                    return true;
                }
            };
            if let Some(path) = running.quiet.take() {
                finished_quiet.push((path, (now, now.saturating_duration_since(*started))));
            }
            commander.update(cx, |c, _| c.finish_load(running.id, result));
            changed = true;
            false
        });
        self.quiet_reads.extend(finished_quiet);
        if changed {
            commander.update(cx, |_, cx| cx.notify());
        }
        self.polling_loads = !self.loads.is_empty();
        self.polling_loads.then(|| self.next_poll(now))
    }

    /// Fast while a read is young (most finish then), slow after; never
    /// past a held-back read's start.
    fn next_poll(&self, now: Instant) -> Duration {
        self.loads
            .iter()
            .map(|running| match running.state {
                State::Running(_, started)
                    if now.saturating_duration_since(started) < SHOW_AFTER =>
                {
                    FAST_POLL
                }
                State::Running(..) => SLOW_POLL,
                State::Waiting(_, at) => at
                    .saturating_duration_since(now)
                    .clamp(FAST_POLL, SLOW_POLL),
            })
            .min()
            .unwrap_or(SLOW_POLL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_quiet_read_makes_the_next_one_wait_as_long() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(rest(None, t0), t0);
        assert_eq!(rest(Some((t0, ms(50))), t0), t0, "a fast folder");
        assert_eq!(rest(Some((t0, ms(800))), t0 + ms(100)), t0 + ms(800));
        assert_eq!(rest(Some((t0, ms(800))), t0 + ms(900)), t0 + ms(900));
    }
}
