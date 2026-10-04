//! Watches a panel's folder for changes made outside the app (see
//! `Requirements.md`, File operations). One plain thread per panel, never
//! gpui's pool: watching a folder on a dead network mount can hang, and must
//! hang only that panel's watcher.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use notify::{Event, EventKind, RecursiveMode, Watcher};

/// From the first change after a quiet spell to the reload.
pub const FIRST_DELAY: Duration = Duration::from_millis(100);
/// Least time between two reloads while changes keep coming.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// A `watch` or `unwatch` call running this long is taken as hung (a dead
/// mount): the next folder gets a new thread.
pub const HUNG_AFTER: Duration = Duration::from_secs(2);

/// What a [`PanelWatcher`] reports. The folder is named as it was given
/// to [`PanelWatcher::watch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The folder's listing changed (throttled, see [`Throttle`]).
    Changed(PathBuf),
    /// The watch is in place, and the folder was last modified at this
    /// time, read just after. A listing read before the watch began may
    /// miss a change made in between; its folder time tells.
    Started(PathBuf, Option<SystemTime>),
}

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
            .any(|p| same_path(p, dir) || p.parent().is_some_and(|p| same_path(p, dir)))
}

/// macOS folders are usually case-insensitive, and FSEvents may name a
/// path in its on-disk case while the panel's differs. A false match costs
/// a re-read; a missed one, a stale panel.
fn same_path(a: &Path, b: &Path) -> bool {
    same_path_ignoring_case(a, b, cfg!(target_os = "macos"))
}

fn same_path_ignoring_case(a: &Path, b: &Path, ignore_case: bool) -> bool {
    a == b
        || ignore_case && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
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

/// Shared with a watcher thread.
#[derive(Default)]
struct Busy {
    /// Since when the thread has been inside a `watch` or `unwatch` call,
    /// if it is.
    since: Mutex<Option<Instant>>,
    /// Given up as hung: if its call ever returns, it reports nothing.
    abandoned: std::sync::atomic::AtomicBool,
}

type Shared = Arc<Busy>;

/// Starts a watcher thread.
type Spawn = Box<dyn Fn() -> (Sender<Msg>, Shared) + Send>;

/// Watches one panel's folder on its own thread and calls `on_report` when
/// its listing changes (throttled) or a watch begins. Only that thread ever
/// calls notify's `watch`, which can hang on a dead mount; a thread hung
/// there is left behind and the next folder gets a new one.
pub struct PanelWatcher {
    tx: Sender<Msg>,
    busy: Shared,
    spawn: Spawn,
}

impl std::fmt::Debug for PanelWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelWatcher").finish_non_exhaustive()
    }
}

impl PanelWatcher {
    pub fn spawn(on_report: impl Fn(Report) + Send + Sync + 'static) -> Self {
        Self::start(notify::recommended_watcher, on_report)
    }

    /// `make` builds the notify watcher on each new thread, from a handler
    /// that forwards its events to that thread.
    fn start<W: Watcher>(
        make: impl Fn(Forward) -> notify::Result<W> + Send + Sync + 'static,
        on_report: impl Fn(Report) + Send + Sync + 'static,
    ) -> Self {
        let make = Arc::new(make);
        let on_report = Arc::new(on_report);
        let spawn: Spawn = Box::new(move || {
            let (tx, rx) = mpsc::channel();
            let busy = Shared::default();
            let events = tx.clone();
            let (make, on_report, thread_busy) = (make.clone(), on_report.clone(), busy.clone());
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
                        serve(&rx, watcher, &thread_busy, |report| {
                            if !thread_busy.abandoned.load(Ordering::SeqCst) {
                                on_report(report);
                            }
                        });
                    }
                });
            (tx, busy)
        });
        let (tx, busy) = spawn();
        Self { tx, busy, spawn }
    }

    /// Watches `path` from now on, instead of the previous folder.
    pub fn watch(&mut self, path: PathBuf) {
        self.send_watch(path, None);
    }

    fn send_watch(&mut self, path: PathBuf, done: Option<Sender<()>>) {
        let hung = self
            .busy
            .since
            .lock()
            .map(|since| since.is_some_and(|since| since.elapsed() >= HUNG_AFTER))
            .unwrap_or(true);
        if hung {
            // It stops, silently, if its call ever returns.
            self.busy.abandoned.store(true, Ordering::SeqCst);
            let _ = self.tx.send(Msg::Stop);
            (self.tx, self.busy) = (self.spawn)();
        }
        let _ = self.tx.send(Msg::Watch(path, done));
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
/// reports a change when a reload is due. `busy` holds when a `watch` or
/// `unwatch` call began, while one runs.
fn serve<W: Watcher>(rx: &Receiver<Msg>, mut watcher: W, busy: &Busy, report: impl Fn(Report)) {
    let calling = |f: &mut dyn FnMut()| {
        if let Ok(mut since) = busy.since.lock() {
            *since = Some(Instant::now());
        }
        f();
        if let Ok(mut since) = busy.since.lock() {
            *since = None;
        }
    };
    // The folder as given (what reports name) and as watched (canonical).
    let mut given: Option<PathBuf> = None;
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
                calling(&mut || lost = !rewatch(&mut watcher, dir));
                retry = lost.then(|| now + MIN_INTERVAL);
            }
            throttle.fire(now);
            if let Some(given) = &given {
                report(Report::Changed(given.clone()));
            }
        }
        if retry.is_some_and(|at| now >= at)
            && let (Some(dir), Some(given)) = (&dir, &given)
        {
            calling(&mut || lost = !rewatch(&mut watcher, dir));
            retry = lost.then(|| now + MIN_INTERVAL);
            if !lost {
                // The folder is back, maybe with entries.
                report(Report::Changed(given.clone()));
            }
        }
        let wake = [throttle.deadline(), retry].into_iter().flatten().min();
        let msg = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Watch(path, done)) => {
                given = None;
                if let Some(old) = dir.take() {
                    calling(&mut || {
                        let _ = watcher.unwatch(&old);
                    });
                }
                throttle.reset();
                (lost, retry) = (false, None);
                // Events name the real path (macOS: /private/tmp for /tmp).
                let real = path.canonicalize().unwrap_or_else(|_| path.clone());
                let mut watching = false;
                calling(&mut || {
                    watching = watcher.watch(&real, RecursiveMode::NonRecursive).is_ok()
                });
                if watching {
                    // After the watch: a later change is seen, an earlier
                    // one shows in this time.
                    let modified = crate::listing::dir_modified(&real);
                    report(Report::Started(path.clone(), modified));
                    dir = Some(real);
                    given = Some(path);
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
    ) && event.paths.iter().any(|p| same_path(p, dir))
}

/// Watches `dir` again after its watch died. Returns whether that worked.
fn rewatch<W: Watcher>(watcher: &mut W, dir: &Path) -> bool {
    let _ = watcher.unwatch(dir);
    watcher.watch(dir, RecursiveMode::NonRecursive).is_ok()
}

#[cfg(test)]
impl PanelWatcher {
    /// Like [`PanelWatcher::watch`], but returns once the watch is in place.
    fn watch_and_wait(&mut self, path: PathBuf) {
        let (done, rx) = mpsc::channel();
        self.send_watch(path, Some(done));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    /// A watcher that sees no real events; feed it with `send_event`.
    fn with_null_watcher(on_report: impl Fn(Report) + Send + Sync + 'static) -> Self {
        Self::start(|_| Ok(notify::NullWatcher), on_report)
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{self, Receiver};

    const WAIT: Duration = Duration::from_secs(5);

    /// A real watcher whose changes arrive on the receiver.
    fn watcher() -> (PanelWatcher, Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let w = PanelWatcher::spawn(move |report| {
            if matches!(report, Report::Changed(_)) {
                let _ = tx.send(());
            }
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
        let (mut w, rx) = watcher();
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
        let (mut w, rx) = watcher();
        w.watch_and_wait(dir.clone());
        std::fs::remove_dir(&dir).unwrap();
        assert!(changed(&rx));
    }

    #[test]
    fn reading_the_folder_fires_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f"), b"x").unwrap();
        let (mut w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        let request = crate::LoadRequest {
            id: 1,
            path: tmp.path().to_path_buf(),
            fallback: None,
            progress: Arc::default(),
            cancel: Arc::default(),
            archive: None,
            results: None,
            up_if_missing: false,
            quiet: false,
        };
        crate::read_listing(&request).unwrap();
        std::fs::read(tmp.path().join("f")).unwrap(); // F3 opens files too
        assert!(quiet(&rx));
    }

    #[test]
    fn changes_in_subfolders_fire_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        let (mut w, rx) = watcher();
        w.watch_and_wait(tmp.path().to_path_buf());
        std::fs::write(tmp.path().join("sub/f"), b"").unwrap();
        assert!(quiet(&rx));
    }

    #[test]
    fn watching_a_new_folder_ignores_the_old_one() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let (mut w, rx) = watcher();
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
        let (mut w, rx) = watcher();
        w.watch_and_wait(tmp.path().join("link"));
        std::fs::write(tmp.path().join("real/f"), b"").unwrap();
        assert!(changed(&rx));
    }

    #[test]
    fn a_missing_folder_is_simply_not_watched() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, rx) = watcher();
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
        let (mut w, rx) = watcher();
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
        let mut w = PanelWatcher::with_null_watcher({
            let count = count.clone();
            move |report| {
                if matches!(report, Report::Changed(_)) {
                    count.fetch_add(1, Ordering::SeqCst);
                }
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
        let (mut w, rx) = watcher();
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
        let (mut w, rx) = watcher();
        w.watch_and_wait(dir.clone());
        std::fs::remove_dir(&dir).unwrap();
        assert!(changed(&rx), "the delete");
        std::fs::create_dir(&dir).unwrap();
        assert!(changed(&rx), "the folder is back");
        std::fs::write(dir.join("f"), b"").unwrap();
        assert!(changed(&rx), "a change in the new folder");
    }

    #[test]
    fn reports_name_the_folder_as_given() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let link = tmp.path().join("link");
        let (tx, rx) = mpsc::channel();
        let mut w = PanelWatcher::spawn(move |report| {
            let _ = tx.send(report);
        });
        w.watch_and_wait(link.clone());
        assert!(format!("{w:?}").starts_with("PanelWatcher"));
        let modified = crate::listing::dir_modified(&link);
        assert!(modified.is_some());
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Report::Started(link.clone(), modified)
        );
        std::fs::write(tmp.path().join("real/f"), b"").unwrap();
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Report::Changed(link));
    }

    #[test]
    fn a_folder_that_cannot_be_watched_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut w = PanelWatcher::spawn(move |report| {
            let _ = tx.send(report);
        });
        w.watch_and_wait(tmp.path().join("gone"));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    /// Hangs in `watch` on any folder named `hang`, until `release` is set.
    struct HangingWatcher {
        release: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Watcher for HangingWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            unreachable!("built by the test")
        }

        fn watch(&mut self, path: &Path, _: RecursiveMode) -> notify::Result<()> {
            while path.ends_with("hang") && !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }

        fn unwatch(&mut self, _: &Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    #[test]
    fn a_watch_hung_on_a_dead_mount_gets_a_new_thread_for_the_next_folder() {
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let mut w = PanelWatcher::start(
            {
                let (release, threads) = (release.clone(), threads.clone());
                move |_| {
                    threads.fetch_add(1, Ordering::SeqCst);
                    Ok(HangingWatcher {
                        release: release.clone(),
                    })
                }
            },
            move |report| {
                let _ = tx.send(report);
            },
        );
        w.watch(PathBuf::from("/dead/hang"));
        // Soon after: still taken as busy, not hung, so no new thread yet.
        std::thread::sleep(Duration::from_millis(100));
        let ok = PathBuf::from("/ok");
        w.watch(ok.clone());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        std::thread::sleep(HUNG_AFTER);
        w.watch(ok.clone());
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Report::Started(ok, None));
        assert_eq!(threads.load(Ordering::SeqCst), 2);
        // The old thread gets to its queued folder, but says nothing.
        release.store(true, Ordering::SeqCst);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    #[test]
    fn paths_match_ignoring_case_only_where_asked() {
        let (a, b) = (Path::new("/Users/Me/Src"), Path::new("/users/me/src"));
        assert!(same_path_ignoring_case(a, b, true));
        assert!(!same_path_ignoring_case(a, b, false));
        assert!(same_path_ignoring_case(a, a, false));
        assert!(!same_path_ignoring_case(a, Path::new("/users/me"), true));
    }
}
