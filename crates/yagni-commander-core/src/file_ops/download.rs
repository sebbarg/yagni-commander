//! [`super::Operation::Download`]: copies files and folders out of a remote
//! folder (F5 from a remote panel, F3 into a temp folder) over a session of
//! its own, so browsing goes on meanwhile.
//!
//! Names from the server are checked to be single components before they
//! are used locally. Symlinks on the server are not copied (yet).

use std::fs::{self, File, FileTimes};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::{Choice, Destination, Engine, Incoming, Step, create_new, create_temp};
use crate::remote::{self, Session};
use crate::sftp::Attrs;

const NOT_ON_HOST: &str = "not a remote file or folder on the same host";

/// A remote entry to copy: its path on the host, its panel path (for
/// messages and the log) and its own attributes.
struct Item {
    remote: Vec<u8>,
    shown: PathBuf,
    attrs: Attrs,
}

impl Engine<'_> {
    pub(super) fn download(&mut self, sources: &[PathBuf], to: &Destination) {
        self.progress.items_total = sources.len();
        let connected = match sources.first().and_then(|s| remote::parse(s)) {
            Some(location) => remote::connect(location.host)
                .map(|session| (location.host.to_owned(), session))
                .map_err(|e| e.to_string()),
            None => Err(NOT_ON_HOST.to_owned()),
        };
        let (host, mut session) = match connected {
            Ok(connected) => connected,
            Err(message) => {
                for source in sources {
                    self.fail(source, &message);
                }
                self.progress.items_done = sources.len();
                return;
            }
        };
        let mut work = Vec::new();
        for source in sources {
            let item = match remote::parse(source) {
                Some(location) if location.host == host && remote::file_name(source).is_some() => {
                    session
                        .client
                        .lstat(location.path)
                        .map(|attrs| Item {
                            remote: location.path.to_vec(),
                            shown: source.clone(),
                            attrs,
                        })
                        .map_err(|e| e.to_string())
                }
                _ => Err(NOT_ON_HOST.to_owned()),
            };
            match item {
                Ok(item) => work.push(item),
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
        for item in &work {
            if self.scan_remote(&mut session, item).is_none() {
                self.report.cancelled = true;
                return;
            }
        }
        for item in work {
            let target = match to {
                Destination::Into(dir) => {
                    dir.join(remote::file_name(&item.shown).expect("checked above"))
                }
                Destination::As(path) => path.clone(),
            };
            let step = self.download_entry(&mut session, &item, &target);
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    /// Adds the files and bytes under `item` to the progress totals.
    /// `None` if cancelled; unreadable folders are left out (the copy
    /// reports them).
    fn scan_remote(&mut self, session: &mut Session, item: &Item) -> Option<()> {
        if self.observer.is_cancelled() {
            return None;
        }
        if item.attrs.is_dir() {
            for child in self.children(session, item).unwrap_or_default() {
                self.scan_remote(session, &child)?;
            }
        } else if item.attrs.is_file() {
            self.progress.files_total += 1;
            self.progress.bytes_total += item.attrs.size.unwrap_or(0);
            self.report_progress();
        }
        Some(())
    }

    /// The entries of remote folder `item`, with only names that are
    /// single components.
    fn children(&mut self, session: &mut Session, item: &Item) -> io::Result<Vec<Item>> {
        let mut children = Vec::new();
        session
            .client
            .read_dir(&item.remote, |name| {
                if remote::valid_name(&name.name) {
                    children.push(Item {
                        remote: remote::join(&item.remote, &name.name),
                        shown: item.shown.join(std::ffi::OsStr::from_bytes(&name.name)),
                        attrs: name.attrs,
                    });
                }
            })
            .map_err(|e| session.explain(e))?;
        Ok(children)
    }

    fn download_entry(&mut self, session: &mut Session, item: &Item, target: &Path) -> Step {
        self.set_current(&item.shown);
        if item.attrs.is_dir() {
            self.download_dir(session, item, target)
        } else if item.attrs.is_file() {
            let step = self.download_file(session, item, target);
            if step != Step::Cancelled {
                self.progress.files_done += 1;
            }
            if step == Step::Done {
                self.note(format_args!(
                    "copied {} -> {}",
                    item.shown.display(),
                    target.display()
                ));
            }
            step
        } else if item.attrs.is_symlink() {
            self.fail(&item.shown, "symlinks are not copied from a remote folder")
        } else {
            self.fail(&item.shown, "only files and folders can be copied")
        }
    }

    fn download_dir(&mut self, session: &mut Session, item: &Item, target: &Path) -> Step {
        match fs::create_dir(target) {
            Ok(()) => self.note(format_args!("created directory {}", target.display())),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && target.is_dir() => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return self.fail(target, "a file with this name exists");
            }
            Err(e) => return self.fail(target, e),
        }
        let children = match self.children(session, item) {
            Ok(children) => children,
            Err(e) => return self.fail(&item.shown, e),
        };
        let mut result = Step::Done;
        for child in children {
            let name = std::ffi::OsStr::from_bytes(
                child
                    .remote
                    .rsplit(|&b| b == b'/')
                    .next()
                    .unwrap_or_default(),
            );
            match self.download_entry(session, &child, &target.join(name)) {
                Step::Cancelled => return Step::Cancelled,
                Step::Incomplete => result = Step::Incomplete,
                Step::Done => {}
            }
        }
        if let Some(mode) = item.attrs.permissions {
            let _ = fs::set_permissions(target, fs::Permissions::from_mode(mode & 0o777));
        }
        result
    }

    fn download_file(&mut self, session: &mut Session, item: &Item, target: &Path) -> Step {
        let start = self.progress.bytes_done;
        let size = item.attrs.size.unwrap_or(0);
        let step = match create_new(target) {
            Ok(out) => self.fill_from(session, item, target, out),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if target.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    self.fail(target, "a directory with this name exists")
                } else {
                    let incoming = Incoming {
                        size,
                        modified: item.attrs.mtime.map(remote::time_of),
                    };
                    match self.choose(&item.shown, target, Some(incoming)) {
                        Choice::Skip => {
                            self.note(format_args!("skipped {}: exists", target.display()));
                            self.report.skipped += 1;
                            Step::Incomplete
                        }
                        Choice::Cancel => Step::Cancelled,
                        Choice::Overwrite => match create_temp(target, create_new) {
                            Ok((temp, out)) => match self.fill_from(session, item, &temp, out) {
                                Step::Done => self.commit(&temp, target),
                                step => step,
                            },
                            Err(e) => self.fail(target, e),
                        },
                    }
                }
            }
            Err(e) => self.fail(target, e),
        };
        // Skipped and failed files count as done, so the bar reaches the end.
        self.progress.bytes_done = self.progress.bytes_done.max(start + size);
        step
    }

    /// Downloads `item` into `out`, a new file at `path`, with the remote
    /// time and permissions. On failure or cancel, `path` is removed again.
    fn fill_from(&mut self, session: &mut Session, item: &Item, path: &Path, out: File) -> Step {
        let result = self.receive(session, item, &out);
        if let Ok(true) = result {
            // Best effort, as for a local copy.
            if let Some(mtime) = item.attrs.mtime {
                let _ = out.set_times(FileTimes::new().set_modified(remote::time_of(mtime)));
            }
            if let Some(mode) = item.attrs.permissions {
                let _ = out.set_permissions(fs::Permissions::from_mode(mode & 0o777));
            }
            return Step::Done;
        }
        drop(out);
        let _ = fs::remove_file(path);
        match result {
            Err(e) => self.fail(&item.shown, e),
            _ => Step::Cancelled,
        }
    }

    /// Ok(false) if cancelled part way.
    fn receive(&mut self, session: &mut Session, item: &Item, out: &File) -> io::Result<bool> {
        let handle = session
            .client
            .open_read(&item.remote)
            .map_err(|e| session.explain(e))?;
        let result = session.client.download(&handle, out, |n| {
            self.progress.bytes_done += n;
            self.observer.progress(&self.progress);
            !self.observer.is_cancelled()
        });
        let closed = session.client.close(handle);
        match result.and_then(|done| closed.map(|()| done)) {
            Ok(done) => Ok(done),
            Err(e) => Err(session.explain(e)),
        }
    }
}
