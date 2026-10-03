//! Watches a panel's folder for changes made outside the app (see
//! `Requirements.md`, File operations). One plain thread per panel, never
//! gpui's pool: watching a folder on a dead network mount can hang, and must
//! hang only that panel's watcher.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecursiveMode, Watcher};

/// From the first change after a quiet spell to the reload.
pub const FIRST_DELAY: Duration = Duration::from_millis(100);
/// Least time between two reloads while changes keep coming.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// When to reload, given the changes seen: [`FIRST_DELAY`] after the first
/// change, then at most once per [`MIN_INTERVAL`]. Every change is followed
/// by a reload within [`MIN_INTERVAL`]. Time is passed in, for the tests.
#[derive(Debug, Default)]
pub struct Throttle {
    due: Option<Instant>,
    last_fire: Option<Instant>,
}

impl Throttle {
    /// A relevant change arrived.
    pub fn event(&mut self, now: Instant) {
        let at = match self.last_fire {
            Some(last) if now < last + MIN_INTERVAL => last + MIN_INTERVAL,
            _ => now + FIRST_DELAY,
        };
        self.due = Some(self.due.map_or(at, |due| due.min(at)));
    }

    /// When the next reload is due, if one is.
    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    /// The reload was sent.
    pub fn fire(&mut self, now: Instant) {
        self.due = None;
        self.last_fire = Some(now);
    }

    /// A new folder: nothing seen yet.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Whether `event` changes the listing of `dir` (canonical): a change to
/// `dir` itself or one of its entries. Access events never count; our own
/// directory read opens `dir`. A kernel queue overflow (rescan) always does.
pub fn is_relevant(event: &Event, dir: &Path) -> bool {
    if event.need_rescan() {
        return true;
    }
    let changes = matches!(
        event.kind,
        EventKind::Any
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Other
    );
    changes
        && event
            .paths
            .iter()
            .any(|p| p == dir || p.parent() == Some(dir))
}

enum Msg {
    /// Watch this folder instead. The sender, if any, hears when the watch
    /// is in place (tests).
    Watch(PathBuf, Option<Sender<()>>),
    Fs(notify::Result<Event>),
    Stop,
}

/// Forwards a notify watcher's events to the watcher thread.
type Forward = Box<dyn FnMut(notify::Result<Event>) + Send>;

/// Watches one panel's folder on its own thread and calls `on_change`
/// (throttled, see [`Throttle`]) when its listing changes. Only that thread
/// ever calls notify's `watch`, which can hang on a dead mount.
#[derive(Debug)]
pub struct PanelWatcher {
    tx: Sender<Msg>,
}

impl PanelWatcher {
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> Self {
        Self::start(notify::recommended_watcher, on_change)
    }

    /// `make` builds the notify watcher on the new thread, from a handler
    /// that forwards its events to that thread.
    fn start<W: Watcher>(
        make: impl FnOnce(Forward) -> notify::Result<W> + Send + 'static,
        on_change: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        // If the thread can't start, `rx` is dropped with the closure and
        // `watch` sends go nowhere: the panel is simply not watched.
        let _ = std::thread::Builder::new()
            .name("dir-watch".into())
            .spawn(move || {
                let forward: Forward = Box::new(move |event| {
                    let _ = events.send(Msg::Fs(event));
                });
                // No watcher (e.g. inotify instance limit): nothing to do.
                if let Ok(watcher) = make(forward) {
                    serve(&rx, watcher, on_change);
                }
            });
        Self { tx }
    }

    /// Watches `path` from now on, instead of the previous folder.
    pub fn watch(&self, path: PathBuf) {
        let _ = self.tx.send(Msg::Watch(path, None));
    }
}

impl Drop for PanelWatcher {
    fn drop(&mut self) {
        // The notify handler holds a sender too, so dropping ours alone
        // would not end the thread.
        let _ = self.tx.send(Msg::Stop);
    }
}

/// The watcher thread: follows `Watch` messages, throttles relevant events,
/// calls `on_change` when a reload is due.
fn serve<W: Watcher>(rx: &Receiver<Msg>, mut watcher: W, on_change: impl Fn()) {
    let mut dir: Option<PathBuf> = None;
    let mut throttle = Throttle::default();
    // The folder itself was deleted or moved: its watch died with it (inotify
    // watches the inode). `rm -rf dist && mkdir dist` gives a new folder at
    // the same path, which the panel keeps showing, so watch the path again.
    let mut lost = false;
    // When to try that again, while the path doesn't exist.
    let mut retry: Option<Instant> = None;
    loop {
        // Checked before receiving, so an endless stream of events can't
        // hold the reload back.
        let now = Instant::now();
        if throttle.deadline().is_some_and(|due| now >= due) {
            if lost && let Some(dir) = &dir {
                lost = !rewatch(&mut watcher, dir);
                retry = lost.then(|| now + MIN_INTERVAL);
            }
            throttle.fire(now);
            on_change();
        }
        if retry.is_some_and(|at| now >= at)
            && let Some(dir) = &dir
        {
            lost = !rewatch(&mut watcher, dir);
            retry = lost.then(|| now + MIN_INTERVAL);
            if !lost {
                on_change(); // the folder is back, maybe with entries
            }
        }
        let wake = [throttle.deadline(), retry].into_iter().flatten().min();
        let msg = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Watch(path, done)) => {
                if let Some(old) = dir.take() {
                    let _ = watcher.unwatch(&old);
                }
                throttle.reset();
                (lost, retry) = (false, None);
                // Events name the real path (macOS: /private/tmp for /tmp).
                let path = path.canonicalize().unwrap_or(path);
                if watcher.watch(&path, RecursiveMode::NonRecursive).is_ok() {
                    dir = Some(path);
                }
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
            Ok(Msg::Fs(Ok(event))) => {
                if let Some(dir) = dir.as_deref().filter(|dir| is_relevant(&event, dir)) {
                    throttle.event(Instant::now());
                    lost |= is_self_loss(&event, dir);
                }
            }
            Ok(Msg::Fs(Err(_))) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Whether `event` says `dir` itself was deleted or moved away.
fn is_self_loss(event: &Event, dir: &Path) -> bool {
    matches!(
        event.kind,
        EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
    ) && event.paths.iter().any(|p| p == dir)
}

/// Watches `dir` again after its watch died. Returns whether that worked.
fn rewatch<W: Watcher>(watcher: &mut W, dir: &Path) -> bool {
    let _ = watcher.unwatch(dir);
    watcher.watch(dir, RecursiveMode::NonRecursive).is_ok()
}

#[cfg(test)]
impl PanelWatcher {
    /// Like [`PanelWatcher::watch`], but returns once the watch is in place.
    fn watch_and_wait(&self, path: PathBuf) {
        let (done, rx) = mpsc::channel();
        self.tx.send(Msg::Watch(path, Some(done))).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    /// A watcher that sees no real events; feed it with `send_event`.
    fn with_null_watcher(on_change: impl Fn() + Send + 'static) -> Self {
        Self::start(|_| Ok(notify::NullWatcher), on_change)
    }

    fn send_event(&self, event: Event) {
        self.tx.send(Msg::Fs(Ok(event))).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode,
    };

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn the_first_event_fires_after_the_short_delay() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        assert_eq!(t.deadline(), None);
        t.event(t0);
        assert_eq!(t.deadline(), Some(at(t0, 100)));
    }

    #[test]
    fn a_burst_gives_one_reload() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.event(t0);
        t.event(at(t0, 30));
        t.event(at(t0, 90));
        assert_eq!(t.deadline(), Some(at(t0, 100)), "the first deadline holds");
        t.fire(at(t0, 100));
        assert_eq!(t.deadline(), None);
    }

    #[test]
    fn continuous_events_fire_once_a_second() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.event(t0);
        t.fire(at(t0, 100));
        t.event(at(t0, 150));
        assert_eq!(t.deadline(), Some(at(t0, 1100)));
        t.event(at(t0, 900));
        assert_eq!(t.deadline(), Some(at(t0, 1100)));
        t.fire(at(t0, 1100));
        t.event(at(t0, 1200));
        assert_eq!(t.deadline(), Some(at(t0, 2100)));
    }

    #[test]
    fn the_last_event_is_followed_within_a_second() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.event(t0);
        t.fire(at(t0, 100));
        t.event(at(t0, 1150)); // after the interval: short delay again
        assert_eq!(t.deadline(), Some(at(t0, 1250)));
        t.fire(at(t0, 1250));
        t.event(at(t0, 1260));
        let due = t.deadline().unwrap();
        assert!(due <= at(t0, 1260) + MIN_INTERVAL);
    }

    #[test]
    fn quiet_after_a_reload_brings_back_the_short_delay() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.event(t0);
        t.fire(at(t0, 100));
        t.event(at(t0, 5000));
        assert_eq!(t.deadline(), Some(at(t0, 5100)));
    }

    #[test]
    fn reset_forgets_everything() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.event(t0);
        t.fire(at(t0, 100));
        t.event(at(t0, 200));
        t.reset();
        assert_eq!(t.deadline(), None);
        t.event(at(t0, 300));
        assert_eq!(t.deadline(), Some(at(t0, 400)), "no interval after a reset");
    }

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn changes_to_children_and_the_folder_itself_count() {
        let dir = Path::new("/w");
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(RemoveKind::Any),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Any,
            EventKind::Other,
        ] {
            assert!(is_relevant(&event(kind, "/w/f"), dir), "{kind:?}");
        }
        assert!(is_relevant(
            &event(EventKind::Remove(RemoveKind::Folder), "/w"),
            dir
        ));
    }

    #[test]
    fn access_events_never_count() {
        // Our own read opens the folder: counting that would loop forever.
        let open = EventKind::Access(AccessKind::Open(AccessMode::Any));
        assert!(!is_relevant(&event(open, "/w"), Path::new("/w")));
        assert!(!is_relevant(&event(open, "/w/f"), Path::new("/w")));
    }

    #[test]
    fn other_folders_and_grandchildren_do_not_count() {
        let create = EventKind::Create(CreateKind::File);
        let dir = Path::new("/w");
        assert!(!is_relevant(&event(create, "/w/sub/f"), dir));
        assert!(!is_relevant(&event(create, "/elsewhere/f"), dir));
        assert!(!is_relevant(&event(create, "/w2"), dir));
    }

    #[test]
    fn a_rescan_always_counts() {
        let overflow = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert!(is_relevant(&overflow, Path::new("/w")));
    }

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};

    const WAIT: Duration = Duration::from_secs(5);

    /// A real watcher whose changes arrive on the receiver.
    fn watcher() -> (PanelWatcher, Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let w = PanelWatcher::spawn(move || {
            let _ = tx.send(());
        });
        (w, rx)
    }

    /// Waits for one change, then lets the throttle's interval pass and
    /// drains, so the next check starts clean.
    fn changed(rx: &Receiver<()>) -> bool {
        let got = rx.recv_timeout(WAIT).is_ok();
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(200));
        while rx.try_recv().is_ok() {}
        got
    }

    fn quiet(rx: &Receiver<()>) -> bool {
        rx.recv_timeout(Duration::from_millis(600)).is_err()
    }

    #[test]
    fn creating_deleting_and_renaming_a_file_fire() {
        let tmp = tempfile::tempdir().unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        std::fs::write(tmp.path().join("f"), b"").unwrap();
        assert!(changed(&rx), "create");
        std::fs::rename(tmp.path().join("f"), tmp.path().join("g")).unwrap();
        assert!(changed(&rx), "rename");
        std::fs::remove_file(tmp.path().join("g")).unwrap();
        assert!(changed(&rx), "delete");
    }

    #[test]
    fn removing_the_folder_fires() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(dir.clone());
        std::fs::remove_dir(&dir).unwrap();
        assert!(changed(&rx));
    }

    #[test]
    fn reading_the_folder_fires_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f"), b"x").unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        let request = crate::LoadRequest {
            id: 1,
            path: tmp.path().to_path_buf(),
            fallback: None,
            progress: Arc::default(),
            cancel: Arc::default(),
            archive: None,
        };
        crate::read_listing(&request).unwrap();
        std::fs::read(tmp.path().join("f")).unwrap(); // F3 opens files too
        assert!(quiet(&rx));
    }

    #[test]
    fn changes_in_subfolders_fire_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        std::fs::write(tmp.path().join("sub/f"), b"").unwrap();
        assert!(quiet(&rx));
    }

    #[test]
    fn watching_a_new_folder_ignores_the_old_one() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(one.path().to_path_buf());
        w.watch_and_wait(two.path().to_path_buf());
        std::fs::write(one.path().join("f"), b"").unwrap();
        assert!(quiet(&rx), "the old folder");
        std::fs::write(two.path().join("f"), b"").unwrap();
        assert!(changed(&rx), "the new folder");
    }

    #[test]
    fn a_folder_watched_through_a_symlink_still_fires() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().join("link"));
        std::fs::write(tmp.path().join("real/f"), b"").unwrap();
        assert!(changed(&rx));
    }

    #[test]
    fn a_missing_folder_is_simply_not_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().join("gone"));
        std::fs::create_dir(tmp.path().join("gone")).unwrap();
        std::fs::write(tmp.path().join("gone/f"), b"").unwrap();
        assert!(quiet(&rx));
        w.watch_and_wait(tmp.path().to_path_buf()); // still alive
        std::fs::write(tmp.path().join("f"), b"").unwrap();
        assert!(changed(&rx));
    }

    #[test]
    fn dropping_the_watcher_ends_its_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        drop(w);
        // The thread owned the only sender: it is gone once the thread ends.
        assert_eq!(
            rx.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn a_flood_of_events_fires_about_once_a_second() {
        let count = Arc::new(AtomicUsize::new(0));
        let w = PanelWatcher::with_null_watcher({
            let count = count.clone();
            move || {
                count.fetch_add(1, Ordering::SeqCst);
            }
        });
        let dir = tempfile::tempdir().unwrap();
        w.watch_and_wait(dir.path().to_path_buf());
        let dir = dir.path().canonicalize().unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1500) {
            for _ in 0..50 {
                w.send_event(
                    Event::new(EventKind::Create(CreateKind::File)).add_path(dir.join("f")),
                );
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(1300));
        let fired = count.load(Ordering::SeqCst);
        // About 0.1 s, 1.1 s, then the final one by 2.5 s.
        assert!((2..=4).contains(&fired), "fired {fired} times");
    }

    #[test]
    fn a_folder_recreated_at_once_is_watched_again() {
        // `rm -rf dist && mkdir dist`: the old watch died with the old inode.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(dir.clone());
        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(changed(&rx), "the delete");
        std::fs::write(dir.join("f"), b"").unwrap();
        assert!(changed(&rx), "a change in the new folder");
    }

    #[test]
    fn a_folder_recreated_later_is_watched_again() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("d");
        std::fs::create_dir(&dir).unwrap();
        let (w, rx) = watcher();
        w.watch_and_wait(dir.clone());
        std::fs::remove_dir(&dir).unwrap();
        assert!(changed(&rx), "the delete");
        std::fs::create_dir(&dir).unwrap();
        assert!(changed(&rx), "the folder is back");
        std::fs::write(dir.join("f"), b"").unwrap();
        assert!(changed(&rx), "a change in the new folder");
    }
}
