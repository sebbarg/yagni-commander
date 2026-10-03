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

use std::fmt;
use std::fs::{self, File, FileTimes, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use crate::fs_ops::{rename_noreplace, same_file};
use crate::oplog::OperationLog;

mod archive_names;
mod extract;
mod pack;
mod safe_dir;
pub use archive_names::{Format, is_archive};

/// Bytes copied between progress reports and cancel checks.
const CHUNK: u64 = 4 << 20;
/// Minimum time between progress events sent by a [`Job`].
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Copies the sources to `to`.
    Copy {
        sources: Vec<PathBuf>,
        to: Destination,
    },
    /// Moves the sources to `to`: a rename where possible, otherwise
    /// (another filesystem) copy, then delete.
    Move {
        sources: Vec<PathBuf>,
        to: Destination,
    },
    /// Moves each source to the system trash.
    Trash { sources: Vec<PathBuf> },
    /// Deletes each source permanently, directories with their contents.
    Delete { sources: Vec<PathBuf> },
    /// Packs the sources into a new zip at `to` (Alt-F5). Entry names are
    /// relative to `base` (the panel's folder).
    Pack {
        sources: Vec<PathBuf>,
        base: PathBuf,
        to: PathBuf,
    },
    /// Extracts each archive into `into` (Alt-F6), with smart extraction.
    Extract {
        archives: Vec<PathBuf>,
        into: PathBuf,
    },
}

/// Where copied or moved entries go. Missing folders on the way are created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Into this directory, each source keeping its name.
    Into(PathBuf),
    /// To exactly this path (one source only): copy or move under a new
    /// name, or rename in place.
    As(PathBuf),
}

impl Destination {
    fn path(&self) -> &Path {
        match self {
            Destination::Into(path) | Destination::As(path) => path,
        }
    }
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
    /// Set when the new file comes from an archive.
    pub incoming: Option<Incoming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
    Cancel,
}

/// A symbolic link met while packing; the job waits for a [`LinkAnswer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkQuestion {
    /// The link on disk.
    pub link: PathBuf,
    /// Its name in the archive, e.g. `src/tools/run`.
    pub shown: PathBuf,
    /// What it points to, as stored in the link.
    pub target: PathBuf,
    pub place: LinkPlace,
}

/// Where a link points, relative to what is being packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPlace {
    Inside,
    Outside,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkChoice {
    /// Pack what the link points to, under the link's name.
    Follow,
    /// Pack the link itself.
    Store,
    LeaveOut,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkAnswer {
    pub choice: LinkChoice,
    /// "Same for the remaining links": no more questions in this job.
    pub for_all: bool,
}

/// An archive entry that would replace an existing file, for the conflict
/// prompt (it has no file on disk to describe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Incoming {
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// An encrypted zip entry and no password that fits yet; the job waits for
/// a [`PasswordAnswer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordQuestion {
    pub archive: PathBuf,
    /// The password just typed for this archive was wrong.
    pub retry: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub enum PasswordAnswer {
    Password(String),
    /// Leave this archive's encrypted entries out.
    Skip,
    Cancel,
}

/// Never prints the password (answers end up in panics and debug output).
impl fmt::Debug for PasswordAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasswordAnswer::Password(_) => f.write_str("Password(..)"),
            PasswordAnswer::Skip => f.write_str("Skip"),
            PasswordAnswer::Cancel => f.write_str("Cancel"),
        }
    }
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
    /// Links the user chose to leave out of a zip.
    pub left_out: Vec<PathBuf>,
}

pub trait Observer {
    fn progress(&mut self, progress: &Progress);
    /// A target exists. Not called once the user has answered "... all".
    fn conflict(&mut self, conflict: &Conflict) -> Answer;
    fn is_cancelled(&self) -> bool;
    /// A symlink while packing. Not called again once the user has
    /// answered with `for_all`.
    fn link(&mut self, question: &LinkQuestion) -> LinkAnswer;
    /// An encrypted zip entry and no password that fits yet.
    fn password(&mut self, question: &PasswordQuestion) -> PasswordAnswer;
}

/// How operations run, beyond what the user chose.
#[derive(Clone)]
pub struct Settings {
    /// What trash operations use. Tests pass a fake.
    pub trash: TrashFn,
    /// Where each file touched is logged, if logging is on.
    pub log: Option<Arc<OperationLog>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trash: system_trash,
            log: None,
        }
    }
}

/// Moves one entry to the trash, or says why not. [`system_trash`] in the
/// app; tests pass a fake so they never touch the user's real trash.
pub type TrashFn = fn(&Path) -> Result<(), String>;

/// The platform's trash (freedesktop on Linux, Finder on macOS).
pub fn system_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
}

/// Runs `operation` to completion (or cancellation) on the calling thread.
pub fn run(operation: &Operation, observer: &mut dyn Observer, settings: &Settings) -> Report {
    let (name, sources): (&'static str, &[PathBuf]) = match operation {
        Operation::Copy { sources, .. } => ("copy", sources),
        Operation::Move { sources, .. } => ("move", sources),
        Operation::Trash { sources } => ("trash", sources),
        Operation::Delete { sources } => ("delete", sources),
        Operation::Pack { sources, .. } => ("pack", sources),
        Operation::Extract { archives, .. } => ("extract", archives),
    };
    let to: Option<&Path> = match operation {
        Operation::Copy { to, .. } | Operation::Move { to, .. } => Some(to.path()),
        Operation::Pack { to, .. } => Some(to),
        Operation::Extract { into, .. } => Some(into),
        Operation::Trash { .. } | Operation::Delete { .. } => None,
    };
    let mut engine = Engine {
        observer,
        progress: Progress::default(),
        always: None,
        report: Report::default(),
        log: settings.log.as_deref(),
        name,
        link_choice: None,
        password: None,
    };
    match to {
        Some(to) => engine.note(format_args!(
            "start: {} entries to {}",
            sources.len(),
            to.display()
        )),
        None => engine.note(format_args!("start: {} entries", sources.len())),
    }
    for source in sources {
        engine.note(format_args!("source {}", source.display()));
    }
    match operation {
        Operation::Copy { sources, to } => engine.transfer(sources, to, false),
        Operation::Move { sources, to } => engine.transfer(sources, to, true),
        Operation::Trash { sources } => engine.trash(sources, settings.trash),
        Operation::Delete { sources } => engine.delete(sources),
        Operation::Pack { sources, base, to } => engine.pack(sources, base, to),
        Operation::Extract { archives, into } => engine.extract(archives, into),
    }
    let report = &engine.report;
    engine.note(format_args!(
        "{}: {} failed, {} skipped",
        if report.cancelled {
            "cancelled"
        } else {
            "finished"
        },
        report.failures.len(),
        report.skipped
    ));
    engine.report
}

/// `path` with symlinks resolved as far as it exists; the missing rest is
/// appended as is.
fn resolve(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
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
    log: Option<&'a OperationLog>,
    /// The operation's name in log lines ("copy", "move", ...).
    name: &'static str,
    /// Set by "Same for the remaining links".
    link_choice: Option<LinkChoice>,
    /// The last password given, tried first on every encrypted entry.
    password: Option<String>,
}

impl Engine<'_> {
    fn transfer(&mut self, sources: &[PathBuf], to: &Destination, is_move: bool) {
        self.progress.items_total = sources.len();
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
        if work.is_empty() {
            return;
        }
        let folder = match to {
            Destination::Into(dir) => dir.as_path(),
            Destination::As(path) => path.parent().unwrap_or(Path::new("/")),
        };
        if !self.make_folders(folder) {
            return;
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

    /// Where `source` goes, unless that makes no sense.
    fn target_for(&self, source: &Path, to: &Destination) -> Result<PathBuf, &'static str> {
        let Some(name) = source.file_name() else {
            return Err("a filesystem root cannot be copied or moved");
        };
        let target = match to {
            Destination::Into(dir) => dir.join(name),
            Destination::As(path) => path.clone(),
        };
        if same_file(source, &target) {
            return Err("source and target are the same");
        }
        if source.symlink_metadata().is_ok_and(|m| m.is_dir())
            && let Ok(source) = source.canonicalize()
            && resolve(&target).starts_with(&source)
        {
            return Err("a directory cannot be put inside itself");
        }
        Ok(target)
    }

    /// Creates `dir` and any missing folders above it, logging each. False
    /// (with a failure recorded) if that is not possible.
    fn make_folders(&mut self, dir: &Path) -> bool {
        if dir.is_dir() {
            return true;
        }
        let missing: Vec<&Path> = dir
            .ancestors()
            .take_while(|path| path.symlink_metadata().is_err())
            .collect();
        if missing.is_empty() {
            self.fail(dir, "the destination is not a directory");
            return false;
        }
        for path in missing.into_iter().rev() {
            if let Err(e) = fs::create_dir(path) {
                self.fail(path, e);
                return false;
            }
            self.note(format_args!("created directory {}", path.display()));
        }
        true
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
            Ok(()) => {
                self.note(format_args!(
                    "moved {} -> {}",
                    source.display(),
                    target.display()
                ));
                return Step::Done;
            }
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
                    Ok(()) => {
                        self.note(format_args!(
                            "moved {} -> {}",
                            source.display(),
                            target.display()
                        ));
                        Step::Done
                    }
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
            Ok(()) => {
                self.note(format_args!("removed {}", source.display()));
                Step::Done
            }
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
            Ok(()) => {
                self.note(format_args!("created directory {}", target.display()));
                true
            }
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
        if delete_source {
            if let Err(e) = fs::remove_dir(source) {
                return self.fail(source, e);
            }
            self.note(format_args!("removed {}", source.display()));
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
        if step == Step::Done {
            self.note(format_args!(
                "copied {} -> {}",
                source.display(),
                target.display()
            ));
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
            Ok(()) => {
                self.note(format_args!("removed {}", source.display()));
                Step::Done
            }
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
        match self.choose(source, target, None) {
            Choice::Overwrite => {
                self.note(format_args!("replacing {}", target.display()));
                None
            }
            Choice::Skip => {
                self.note(format_args!(
                    "skipped {} -> {}: exists",
                    source.display(),
                    target.display()
                ));
                self.report.skipped += 1;
                Some(Step::Incomplete)
            }
            Choice::Cancel => Some(Step::Cancelled),
        }
    }

    fn choose(&mut self, source: &Path, target: &Path, incoming: Option<Incoming>) -> Choice {
        match self.always {
            Some(true) => return Choice::Overwrite,
            Some(false) => return Choice::Skip,
            None => {}
        }
        let conflict = Conflict {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
            incoming,
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

    /// Permanent delete. Counts the files first for progress, then removes
    /// each source with [`Engine::delete_at`].
    fn delete(&mut self, sources: &[PathBuf]) {
        self.progress.items_total = sources.len();
        // Roots are rejected before the scan, which would walk everything.
        let mut work = Vec::new();
        for source in sources {
            match (source.parent(), source.file_name()) {
                (Some(parent), Some(name)) => work.push((source, parent, name)),
                _ => {
                    self.fail(source, "a filesystem root cannot be deleted");
                    self.progress.items_done += 1;
                }
            }
        }
        for (source, _, _) in &work {
            if self.scan(source).is_none() {
                self.report.cancelled = true;
                return;
            }
        }
        self.progress.bytes_total = 0; // deleting takes no time per byte
        for (source, parent, name) in work {
            let step = match open_dir(parent) {
                Ok(dir) => self.delete_at(&dir, name, source),
                Err(e) => self.fail(parent, e),
            };
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    /// Deletes `name` inside the open directory `dir` (shown as `path`).
    ///
    /// Everything is addressed relative to an open directory and never
    /// through a symlink (`O_NOFOLLOW`, `AT_SYMLINK_NOFOLLOW`), so replacing
    /// a directory with a symlink while this runs cannot make it delete
    /// anything outside the tree: the open fails instead. A symlink is
    /// deleted itself, never its target.
    #[cfg(unix)]
    fn delete_at(&mut self, dir: &nix::dir::Dir, name: &std::ffi::OsStr, path: &Path) -> Step {
        use nix::sys::stat::{FileStat, SFlag, fstatat};
        use nix::unistd::{UnlinkatFlags, unlinkat};

        if self.observer.is_cancelled() {
            return Step::Cancelled;
        }
        self.set_current(path);
        let is_dir = |stat: FileStat| {
            SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR
        };
        let stat = match fstatat(dir, name, nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(e) => return self.fail(path, io::Error::from(e)),
        };
        if !is_dir(stat) {
            self.progress.files_done += 1;
            return match unlinkat(dir, name, UnlinkatFlags::NoRemoveDir) {
                Ok(()) => {
                    self.note(format_args!("deleted {}", path.display()));
                    Step::Done
                }
                Err(e) => self.fail(path, io::Error::from(e)),
            };
        }
        let mut child = match open_dir_at(dir, name) {
            Ok(child) => child,
            Err(e) => return self.fail(path, e),
        };
        // Names first: deleting while reading the same directory is unreliable.
        let names: Vec<std::ffi::OsString> = child
            .iter()
            .filter_map(Result::ok)
            .map(|entry| {
                use std::os::unix::ffi::OsStrExt;
                std::ffi::OsStr::from_bytes(entry.file_name().to_bytes()).to_owned()
            })
            .filter(|n| n != "." && n != "..")
            .collect();
        let mut complete = true;
        for child_name in names {
            match self.delete_at(&child, &child_name, &path.join(&child_name)) {
                Step::Done => {}
                Step::Incomplete => complete = false,
                Step::Cancelled => return Step::Cancelled,
            }
        }
        if !complete {
            return Step::Incomplete;
        }
        match unlinkat(dir, name, UnlinkatFlags::RemoveDir) {
            Ok(()) => {
                self.note(format_args!("deleted {}", path.display()));
                Step::Done
            }
            Err(e) => self.fail(path, io::Error::from(e)),
        }
    }

    #[cfg(not(unix))]
    fn delete_at(&mut self, _: &(), _: &std::ffi::OsStr, path: &Path) -> Step {
        self.fail(path, "deleting is not supported on this platform")
    }

    fn trash(&mut self, sources: &[PathBuf], trash: TrashFn) {
        self.progress.items_total = sources.len();
        for source in sources {
            if self.observer.is_cancelled() {
                self.report.cancelled = true;
                return;
            }
            self.set_current(source);
            match trash(source) {
                Ok(()) => self.note(format_args!("trashed {}", source.display())),
                Err(message) => {
                    self.fail(source, message);
                }
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
        let message = message.to_string();
        self.note(format_args!("failed {}: {message}", path.display()));
        self.report.failures.push(Failure {
            path: path.to_path_buf(),
            message,
        });
        Step::Incomplete
    }

    /// Writes a line to the operation log, if logging is on.
    fn note(&self, what: std::fmt::Arguments) {
        if let Some(log) = self.log {
            log.write(format_args!("{} {what}", self.name));
        }
    }
}

/// Opens a directory for [`Engine::delete_at`]. The path itself may go
/// through symlinks: it is the directory the user is looking at.
#[cfg(unix)]
fn open_dir(path: &Path) -> io::Result<nix::dir::Dir> {
    use nix::fcntl::OFlag;
    nix::dir::Dir::open(
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::empty(),
    )
    .map_err(io::Error::from)
}

/// Opens directory `name` inside `dir`, refusing to follow a symlink.
#[cfg(unix)]
fn open_dir_at(dir: &nix::dir::Dir, name: &std::ffi::OsStr) -> io::Result<nix::dir::Dir> {
    use nix::fcntl::OFlag;
    nix::dir::Dir::openat(
        dir,
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::empty(),
    )
    .map_err(io::Error::from)
}

#[cfg(not(unix))]
fn open_dir(_: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
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
    /// The job waits until [`Job::answer_link`] is called.
    Link(LinkQuestion),
    /// The job waits until [`Job::answer_password`] is called.
    Password(PasswordQuestion),
    /// The last event.
    Finished(Report),
}

/// An operation running on a background thread. The UI polls
/// [`Job::try_event`]. Dropping the job cancels the operation.
pub struct Job {
    events: Receiver<Event>,
    answers: Sender<Answer>,
    link_answers: Sender<LinkAnswer>,
    password_answers: Sender<PasswordAnswer>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn spawn(operation: Operation, settings: Settings) -> io::Result<Self> {
        let (event_tx, events) = mpsc::channel();
        let (answers, answer_rx) = mpsc::channel();
        let (link_answers, link_rx) = mpsc::channel();
        let (password_answers, password_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut observer = ChannelObserver {
            events: event_tx,
            answers: answer_rx,
            links: link_rx,
            passwords: password_rx,
            cancel: cancel.clone(),
            last_progress: None,
        };
        std::thread::Builder::new()
            .name("file-operation".into())
            .spawn(move || {
                let report = run(&operation, &mut observer, &settings);
                let _ = observer.events.send(Event::Finished(report));
            })?;
        Ok(Self {
            events,
            answers,
            link_answers,
            password_answers,
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

    /// Answers the pending [`Event::Link`].
    pub fn answer_link(&self, answer: LinkAnswer) {
        let _ = self.link_answers.send(answer);
    }

    /// Answers the pending [`Event::Password`].
    pub fn answer_password(&self, answer: PasswordAnswer) {
        let _ = self.password_answers.send(answer);
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
    links: Receiver<LinkAnswer>,
    passwords: Receiver<PasswordAnswer>,
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

    fn password(&mut self, question: &PasswordQuestion) -> PasswordAnswer {
        if self.events.send(Event::Password(question.clone())).is_err() {
            return PasswordAnswer::Cancel;
        }
        // A dropped job closes the channel, which also means cancel.
        self.passwords.recv().unwrap_or(PasswordAnswer::Cancel)
    }

    fn link(&mut self, question: &LinkQuestion) -> LinkAnswer {
        let cancel = LinkAnswer {
            choice: LinkChoice::Cancel,
            for_all: false,
        };
        if self.events.send(Event::Link(question.clone())).is_err() {
            return cancel;
        }
        // A dropped job closes the channel, which also means cancel.
        self.links.recv().unwrap_or(cancel)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
