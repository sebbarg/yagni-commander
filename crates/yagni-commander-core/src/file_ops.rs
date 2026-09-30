//! Copy, move and trash (F5, F6, F8).
//!
//! [`run`] is the synchronous engine. It talks to its caller through an
//! [`Observer`]: progress, "the target exists, what now?", and "has the user
//! cancelled?". [`Job`] runs it on a background thread and turns those calls
//! into [`Event`]s for the UI.
//!
//! An error on one file is recorded in the [`Report`] and the rest of the
//! operation continues. Existing files are never partly overwritten: a
//! replacement is written next to the target and renamed over it at the end.

use std::fs::{self, File, FileTimes, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::fs_ops::{rename_noreplace, same_file};

/// Bytes copied between progress reports and cancel checks.
const CHUNK: u64 = 4 << 20;
/// Minimum time between progress events sent by a [`Job`].
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Copies each source into the directory `to`, keeping its name.
    Copy { sources: Vec<PathBuf>, to: PathBuf },
    /// Moves each source into the directory `to`: a rename where possible,
    /// otherwise (another filesystem) copy, then delete.
    Move { sources: Vec<PathBuf>, to: PathBuf },
    /// Moves each source to the system trash.
    Trash { sources: Vec<PathBuf> },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Top-level sources finished (done, skipped or failed).
    pub items_done: usize,
    pub items_total: usize,
    /// Files (and symlinks) and their bytes copied. A copy knows the totals
    /// up front; a move adds to them only when it has to copy (another
    /// filesystem). Trash leaves them at 0.
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The entry being worked on.
    pub current: PathBuf,
}

/// A file (or symlink) would replace an existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: PathBuf,
    pub message: String,
}

/// What happened, for the summary at the end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub failures: Vec<Failure>,
    /// Files left alone because their target existed and the user chose Skip.
    pub skipped: usize,
    pub cancelled: bool,
}

pub trait Observer {
    fn progress(&mut self, progress: &Progress);
    /// A target exists. Not called once the user has answered "... all".
    fn conflict(&mut self, conflict: &Conflict) -> Answer;
    fn is_cancelled(&self) -> bool;
}

/// Runs `operation` to completion (or cancellation) on the calling thread.
pub fn run(operation: &Operation, observer: &mut dyn Observer) -> Report {
    run_with(operation, observer, system_trash)
}

fn system_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}

/// [`run`] with a replaceable trash function, so tests never touch the
/// user's real trash.
fn run_with(
    operation: &Operation,
    observer: &mut dyn Observer,
    trash: fn(&Path) -> Result<(), String>,
) -> Report {
    let mut engine = Engine {
        observer,
        progress: Progress::default(),
        always: None,
        report: Report::default(),
    };
    match operation {
        Operation::Copy { sources, to } => engine.transfer(sources, to, false),
        Operation::Move { sources, to } => engine.transfer(sources, to, true),
        Operation::Trash { sources } => engine.trash(sources, trash),
    }
    engine.report
}

/// Outcome of one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Done,
    /// Skipped or failed (already recorded), or a directory where some
    /// entries were.
    Incomplete,
    Cancelled,
}

enum Choice {
    Overwrite,
    Skip,
    Cancel,
}

struct Engine<'a> {
    observer: &'a mut dyn Observer,
    progress: Progress,
    /// Set by "Overwrite all" (true) or "Skip all" (false).
    always: Option<bool>,
    report: Report,
}

impl Engine<'_> {
    fn transfer(&mut self, sources: &[PathBuf], to: &Path, is_move: bool) {
        self.progress.items_total = sources.len();
        if !to.is_dir() {
            self.fail(to, "the destination is not a directory");
            return;
        }
        // Rejected sources are finished right away, so a bad one (like "/")
        // is never scanned.
        let mut work = Vec::new();
        for source in sources {
            match self.target_for(source, to) {
                Ok(target) => work.push((source, target)),
                Err(message) => {
                    self.fail(source, message);
                    self.progress.items_done += 1;
                }
            }
        }
        if !is_move {
            for (source, _) in &work {
                if self.scan(source).is_none() {
                    self.report.cancelled = true;
                    return;
                }
            }
        }
        for (source, target) in work {
            let step = if is_move {
                self.move_entry(source, &target)
            } else {
                self.copy_entry(source, &target, false)
            };
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    /// Where `source` goes in `to`, unless that makes no sense.
    fn target_for(&self, source: &Path, to: &Path) -> Result<PathBuf, &'static str> {
        let Some(name) = source.file_name() else {
            return Err("a filesystem root cannot be copied or moved");
        };
        let target = to.join(name);
        if same_file(source, &target) {
            return Err("source and target are the same");
        }
        if source.symlink_metadata().is_ok_and(|m| m.is_dir())
            && let (Ok(source), Ok(to)) = (source.canonicalize(), to.canonicalize())
            && to.starts_with(&source)
        {
            return Err("a directory cannot be put inside itself");
        }
        Ok(target)
    }

    /// Adds the files and bytes under `path` to the progress totals.
    /// Returns `None` if cancelled. Unreadable entries are left out; the copy
    /// itself reports them.
    fn scan(&mut self, path: &Path) -> Option<()> {
        if self.observer.is_cancelled() {
            return None;
        }
        let Ok(meta) = path.symlink_metadata() else {
            return Some(());
        };
        if !meta.is_dir() {
            self.progress.files_total += 1;
            if meta.is_file() {
                self.progress.bytes_total += meta.len();
            }
            return Some(());
        }
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            self.scan(&entry.path())?;
        }
        Some(())
    }

    fn move_entry(&mut self, source: &Path, target: &Path) -> Step {
        if self.observer.is_cancelled() {
            return Step::Cancelled;
        }
        self.set_current(source);
        let error = match rename_noreplace(source, target) {
            Ok(()) => return Step::Done,
            Err(e) => e,
        };
        match error.kind() {
            io::ErrorKind::CrossesDevices => {
                if self.scan(source).is_none() {
                    return Step::Cancelled;
                }
                return self.copy_entry(source, target, true);
            }
            io::ErrorKind::AlreadyExists => {}
            _ => return self.fail(source, error),
        }
        let meta = match source.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) => return self.fail(source, e),
        };
        let target_is_dir = target.symlink_metadata().is_ok_and(|m| m.is_dir());
        match (meta.is_dir(), target_is_dir) {
            (true, true) => self.merge_move(source, target),
            (true, false) => self.fail(target, "a file with this name exists"),
            (false, true) => self.fail(target, "a directory with this name exists"),
            (false, false) => {
                if let Some(step) = self.on_conflict(source, target) {
                    return step;
                }
                match fs::rename(source, target) {
                    Ok(()) => Step::Done,
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                        self.progress.files_total += 1;
                        self.progress.bytes_total += meta.len();
                        let step = self.copy_leaf(source, target, &meta, true);
                        self.remove_source(source, step)
                    }
                    Err(e) => self.fail(source, e),
                }
            }
        }
    }

    /// Moves the contents of directory `source` into the existing directory
    /// `target`, then removes `source` if everything moved.
    fn merge_move(&mut self, source: &Path, target: &Path) -> Step {
        let entries = match fs::read_dir(source) {
            Ok(entries) => entries,
            Err(e) => return self.fail(source, e),
        };
        let mut complete = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.fail(source, e);
                    complete = false;
                    continue;
                }
            };
            match self.move_entry(&entry.path(), &target.join(entry.file_name())) {
                Step::Done => {}
                Step::Incomplete => complete = false,
                Step::Cancelled => return Step::Cancelled,
            }
        }
        if !complete {
            return Step::Incomplete;
        }
        match fs::remove_dir(source) {
            Ok(()) => Step::Done,
            Err(e) => self.fail(source, e),
        }
    }

    /// Copies `source` to `target`. With `delete_source` (a move to another
    /// filesystem), each entry is deleted once it has been copied.
    fn copy_entry(&mut self, source: &Path, target: &Path, delete_source: bool) -> Step {
        if self.observer.is_cancelled() {
            return Step::Cancelled;
        }
        self.set_current(source);
        let meta = match source.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) => return self.fail(source, e),
        };
        if meta.is_dir() {
            return self.copy_dir(source, target, &meta, delete_source);
        }
        let step = self.copy_leaf(source, target, &meta, false);
        if delete_source {
            self.remove_source(source, step)
        } else {
            step
        }
    }

    fn copy_dir(
        &mut self,
        source: &Path,
        target: &Path,
        meta: &Metadata,
        delete_source: bool,
    ) -> Step {
        let created = match fs::create_dir(target) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if !target.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    return self.fail(target, "a file with this name exists");
                }
                false // merge into the existing directory
            }
            Err(e) => return self.fail(target, e),
        };
        let entries = match fs::read_dir(source) {
            Ok(entries) => entries,
            Err(e) => return self.fail(source, e),
        };
        let mut complete = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.fail(source, e);
                    complete = false;
                    continue;
                }
            };
            let child = target.join(entry.file_name());
            match self.copy_entry(&entry.path(), &child, delete_source) {
                Step::Done => {}
                Step::Incomplete => complete = false,
                Step::Cancelled => return Step::Cancelled,
            }
        }
        // Only for directories we made, and after their contents, which
        // would otherwise change the time (or be blocked by read-only mode).
        if created {
            if let Ok(modified) = meta.modified() {
                let _ = File::open(target).and_then(|dir| dir.set_modified(modified));
            }
            let _ = fs::set_permissions(target, meta.permissions());
        }
        if !complete {
            return Step::Incomplete;
        }
        if delete_source && let Err(e) = fs::remove_dir(source) {
            return self.fail(source, e);
        }
        Step::Done
    }

    /// Copies a file or symlink. With `replace`, the user already agreed to
    /// overwrite `target`.
    fn copy_leaf(&mut self, source: &Path, target: &Path, meta: &Metadata, replace: bool) -> Step {
        let step = if meta.is_symlink() {
            self.copy_symlink(source, target, replace)
        } else if meta.is_file() {
            self.copy_file(source, target, meta, replace)
        } else {
            self.fail(source, "only files, directories and symlinks can be copied")
        };
        if step != Step::Cancelled {
            self.progress.files_done += 1;
        }
        step
    }

    fn copy_file(&mut self, source: &Path, target: &Path, meta: &Metadata, replace: bool) -> Step {
        let start = self.progress.bytes_done;
        let step = if replace {
            self.replace_file(source, target, meta)
        } else {
            match create_new(target) {
                Ok(out) => self.write_file(source, target, out, meta),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    match self.on_conflict(source, target) {
                        Some(step) => step,
                        None => self.replace_file(source, target, meta),
                    }
                }
                Err(e) => self.fail(target, e),
            }
        };
        // Skipped and failed files count as done, so the bar reaches the end.
        self.progress.bytes_done = self.progress.bytes_done.max(start + meta.len());
        step
    }

    fn replace_file(&mut self, source: &Path, target: &Path, meta: &Metadata) -> Step {
        let (temp, out) = match create_temp(target, create_new) {
            Ok(created) => created,
            Err(e) => return self.fail(target, e),
        };
        match self.write_file(source, &temp, out, meta) {
            Step::Done => self.commit(&temp, target),
            step => step,
        }
    }

    /// Copies `source`'s contents into `out` (a new file at `path`) with its
    /// modification time and permissions. On failure or cancel, `path` is
    /// removed again.
    fn write_file(&mut self, source: &Path, path: &Path, out: File, meta: &Metadata) -> Step {
        let result = self.copy_contents(source, &out);
        if let Ok(true) = result {
            // Best effort: filesystems like FAT can't store everything.
            if let Ok(modified) = meta.modified() {
                let _ = out.set_times(FileTimes::new().set_modified(modified));
            }
            let _ = out.set_permissions(meta.permissions());
            return Step::Done;
        }
        drop(out);
        let _ = fs::remove_file(path);
        match result {
            Err(e) => self.fail(source, e),
            _ => Step::Cancelled,
        }
    }

    /// Returns false if cancelled part way.
    fn copy_contents(&mut self, source: &Path, mut out: &File) -> io::Result<bool> {
        let input = File::open(source)?;
        loop {
            if self.observer.is_cancelled() {
                return Ok(false);
            }
            // `Take<&File>` to `&File` still uses copy_file_range on Linux.
            let copied = io::copy(&mut (&input).take(CHUNK), &mut out)?;
            if copied == 0 {
                return Ok(true);
            }
            self.progress.bytes_done += copied;
            self.report_progress();
        }
    }

    #[cfg(unix)]
    fn copy_symlink(&mut self, source: &Path, target: &Path, replace: bool) -> Step {
        use std::os::unix::fs::symlink;
        let link = match fs::read_link(source) {
            Ok(link) => link,
            Err(e) => return self.fail(source, e),
        };
        if !replace {
            match symlink(&link, target) {
                Ok(()) => return Step::Done,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if let Some(step) = self.on_conflict(source, target) {
                        return step;
                    }
                }
                Err(e) => return self.fail(target, e),
            }
        }
        match create_temp(target, |temp| symlink(&link, temp)) {
            Ok((temp, ())) => self.commit(&temp, target),
            Err(e) => self.fail(target, e),
        }
    }

    #[cfg(not(unix))]
    fn copy_symlink(&mut self, source: &Path, _: &Path, _: bool) -> Step {
        self.fail(source, "symlinks are not supported on this platform")
    }

    /// Renames the finished replacement `temp` over `target`.
    fn commit(&mut self, temp: &Path, target: &Path) -> Step {
        match fs::rename(temp, target) {
            Ok(()) => Step::Done,
            Err(e) => {
                let _ = fs::remove_file(temp);
                self.fail(target, e)
            }
        }
    }

    /// After a leaf was copied for a move: deletes the original.
    fn remove_source(&mut self, source: &Path, step: Step) -> Step {
        if step != Step::Done {
            return step;
        }
        match fs::remove_file(source) {
            Ok(()) => Step::Done,
            Err(e) => self.fail(
                source,
                format!("copied, but cannot delete the original: {e}"),
            ),
        }
    }

    /// `target` exists. Returns the step to end with, or `None` to overwrite.
    fn on_conflict(&mut self, source: &Path, target: &Path) -> Option<Step> {
        if same_file(source, target) {
            return Some(self.fail(source, "source and target are the same"));
        }
        if target.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            return Some(self.fail(target, "a directory with this name exists"));
        }
        match self.choose(source, target) {
            Choice::Overwrite => None,
            Choice::Skip => {
                self.report.skipped += 1;
                Some(Step::Incomplete)
            }
            Choice::Cancel => Some(Step::Cancelled),
        }
    }

    fn choose(&mut self, source: &Path, target: &Path) -> Choice {
        match self.always {
            Some(true) => return Choice::Overwrite,
            Some(false) => return Choice::Skip,
            None => {}
        }
        let conflict = Conflict {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
        };
        match self.observer.conflict(&conflict) {
            Answer::Overwrite => Choice::Overwrite,
            Answer::Skip => Choice::Skip,
            Answer::OverwriteAll => {
                self.always = Some(true);
                Choice::Overwrite
            }
            Answer::SkipAll => {
                self.always = Some(false);
                Choice::Skip
            }
            Answer::Cancel => Choice::Cancel,
        }
    }

    fn trash(&mut self, sources: &[PathBuf], trash: fn(&Path) -> Result<(), String>) {
        self.progress.items_total = sources.len();
        for source in sources {
            if self.observer.is_cancelled() {
                self.report.cancelled = true;
                return;
            }
            self.set_current(source);
            if let Err(message) = trash(source) {
                self.fail(source, message);
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    fn set_current(&mut self, path: &Path) {
        path.clone_into(&mut self.progress.current);
        self.report_progress();
    }

    fn report_progress(&mut self) {
        self.observer.progress(&self.progress);
    }

    fn fail(&mut self, path: &Path, message: impl ToString) -> Step {
        self.report.failures.push(Failure {
            path: path.to_path_buf(),
            message: message.to_string(),
        });
        Step::Incomplete
    }
}

fn create_new(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Creates a hidden temporary sibling of `target` with `create`, which must
/// fail with `AlreadyExists` if its path is taken.
fn create_temp<T>(
    target: &Path,
    mut create: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    for n in 0..100 {
        let temp = target.with_file_name(format!(".{name}.yagni-{n}.tmp"));
        match create(&temp) {
            Ok(created) => return Ok((temp, created)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// What a [`Job`] reports to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Sent at most every 50 ms.
    Progress(Progress),
    /// The job waits until [`Job::answer`] is called.
    Conflict(Conflict),
    /// The last event.
    Finished(Report),
}

/// An operation running on a background thread. The UI polls
/// [`Job::try_event`]. Dropping the job cancels the operation.
pub struct Job {
    events: Receiver<Event>,
    answers: Sender<Answer>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn spawn(operation: Operation) -> io::Result<Self> {
        Self::spawn_with(operation, system_trash)
    }

    fn spawn_with(
        operation: Operation,
        trash: fn(&Path) -> Result<(), String>,
    ) -> io::Result<Self> {
        let (event_tx, events) = mpsc::channel();
        let (answers, answer_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut observer = ChannelObserver {
            events: event_tx,
            answers: answer_rx,
            cancel: cancel.clone(),
            last_progress: None,
        };
        std::thread::Builder::new()
            .name("file-operation".into())
            .spawn(move || {
                let report = run_with(&operation, &mut observer, trash);
                let _ = observer.events.send(Event::Finished(report));
            })?;
        Ok(Self {
            events,
            answers,
            cancel,
        })
    }

    /// The next event, if one is waiting.
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Answers the pending [`Event::Conflict`].
    pub fn answer(&self, answer: Answer) {
        let _ = self.answers.send(answer);
    }

    /// Stops at the next file or chunk. A pending conflict must still be
    /// answered (with [`Answer::Cancel`]).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct ChannelObserver {
    events: Sender<Event>,
    answers: Receiver<Answer>,
    cancel: Arc<AtomicBool>,
    last_progress: Option<Instant>,
}

impl Observer for ChannelObserver {
    fn progress(&mut self, progress: &Progress) {
        if self
            .last_progress
            .is_some_and(|last| last.elapsed() < PROGRESS_INTERVAL)
        {
            return;
        }
        self.last_progress = Some(Instant::now());
        let _ = self.events.send(Event::Progress(progress.clone()));
    }

    fn conflict(&mut self, conflict: &Conflict) -> Answer {
        if self.events.send(Event::Conflict(conflict.clone())).is_err() {
            return Answer::Cancel;
        }
        // A dropped job closes the channel, which also means cancel.
        self.answers.recv().unwrap_or(Answer::Cancel)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
