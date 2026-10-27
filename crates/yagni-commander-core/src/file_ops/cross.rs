//! Copy, move and delete with a remote end (upload, download, between
//! hosts, on a host), over the file systems of [`super::vfs`]. Works like
//! the local engine: totals first, a conflict asks (with the incoming
//! file's size and time), a replacement is written beside the target and
//! renamed over it, a failure is recorded and the rest goes on, a move
//! deletes each original only once its copy is complete.

use std::io;
use std::path::{Path, PathBuf};

use super::vfs::{self, Fs, Kind, Meta};
use super::{Choice, Destination, Engine, Incoming, Step};

/// Path components a copy goes down to (a local path is at most 4096
/// bytes, so a real tree is far shallower).
const MAX_DEPTH: usize = 1024;

/// A file system and the paths on it, for messages when it can't be
/// reached.
fn open_or_fail(engine: &mut Engine<'_>, at: &Path, paths: &[PathBuf]) -> Option<Box<dyn Fs>> {
    match vfs::open(at) {
        Ok(fs) => Some(fs),
        Err(e) => {
            for path in paths {
                engine.fail(path, &e);
            }
            None
        }
    }
}

/// Whether `path` is on `fs`.
fn on(fs: &dyn Fs, path: &Path) -> bool {
    match fs.host() {
        Some(host) => crate::remote::parse(path).is_some_and(|l| l.host == host),
        None => !crate::remote::is_remote(path),
    }
}

impl Engine<'_> {
    /// F5/F6 with a remote source or destination.
    pub(super) fn transfer_across(&mut self, sources: &[PathBuf], to: &Destination, is_move: bool) {
        self.progress.items_total = sources.len();
        let finish_all = |engine: &mut Engine<'_>| engine.progress.items_done = sources.len();
        let Some(first) = sources.first() else { return };
        let Some(mut src) = open_or_fail(self, first, sources) else {
            return finish_all(self);
        };
        let folder = match to {
            Destination::Into(dir) => dir.clone(),
            Destination::As(path) => vfs::parent(path).unwrap_or_else(|| path.clone()),
        };
        let Some(mut dst) = open_or_fail(self, &folder, sources) else {
            return finish_all(self);
        };
        // A typed path that is an existing folder means into it, as for a
        // local destination (the UI can't look on a host without waiting).
        let into;
        let (to, folder) = match to {
            Destination::As(path) if dst.lstat(path).is_ok_and(|m| m.kind == Kind::Dir) => {
                into = Destination::Into(path.clone());
                (&into, path.clone())
            }
            _ => (to, folder),
        };
        let same_fs = src.host() == dst.host();
        let mut work = Vec::new();
        for source in sources {
            match self.target_across(src.as_mut(), source, to, same_fs) {
                Ok(item) => work.push(item),
                Err(message) => {
                    self.fail(source, message);
                    self.progress.items_done += 1;
                }
            }
        }
        if work.is_empty() || !self.make_folders_across(dst.as_mut(), &folder) {
            return;
        }
        let rename = is_move && same_fs;
        if !rename {
            for (source, meta, _) in &work {
                if self.scan_across(src.as_mut(), source, meta).is_none() {
                    self.report.cancelled = true;
                    return;
                }
            }
        }
        for (source, meta, target) in work {
            let step = if rename {
                self.rename_across(src.as_mut(), &source, &meta, &target)
            } else {
                self.copy_across(src.as_mut(), dst.as_mut(), &source, &meta, &target, is_move)
            };
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    /// Where `source` goes, with its attributes, unless that makes no sense.
    fn target_across(
        &mut self,
        src: &mut dyn Fs,
        source: &Path,
        to: &Destination,
        same_fs: bool,
    ) -> Result<(PathBuf, Meta, PathBuf), String> {
        if !on(src, source) {
            return Err("all sources must be in one place".into());
        }
        let name = vfs::file_name(source).ok_or("a filesystem root cannot be copied or moved")?;
        let target = match to {
            Destination::Into(dir) => dir.join(&name),
            Destination::As(path) => path.clone(),
        };
        if same_fs && target == source {
            return Err("source and target are the same".into());
        }
        let meta = src.lstat(source).map_err(|e| e.to_string())?;
        if same_fs && meta.kind == Kind::Dir && target.starts_with(source) {
            return Err("a directory cannot be put inside itself".into());
        }
        Ok((source.to_path_buf(), meta, target))
    }

    /// Creates `dir` and the folders above it that are missing.
    fn make_folders_across(&mut self, fs: &mut dyn Fs, dir: &Path) -> bool {
        match fs.lstat(dir) {
            Ok(meta) if meta.kind == Kind::Dir => return true,
            Ok(_) => {
                self.fail(dir, "the destination is not a directory");
                return false;
            }
            Err(_) => {}
        }
        if let Some(parent) = vfs::parent(dir)
            && parent != dir
            && !self.make_folders_across(fs, &parent)
        {
            return false;
        }
        match fs.make_dir(dir, 0o755) {
            Ok(()) => {
                self.note(format_args!("created directory {}", dir.display()));
                true
            }
            Err(e) => {
                self.fail(dir, e);
                false
            }
        }
    }

    /// Adds the files and bytes under `path` to the totals; `None` if
    /// cancelled. Unreadable folders are left out (the copy reports them).
    fn scan_across(&mut self, fs: &mut dyn Fs, path: &Path, meta: &Meta) -> Option<()> {
        if self.observer.is_cancelled() {
            return None;
        }
        // As the copy will: a listing's folder that is a link is a link.
        let kind = match meta.kind {
            Kind::Dir => fs.lstat(path).map_or(Kind::Dir, |m| m.kind),
            kind => kind,
        };
        match kind {
            Kind::Dir => {
                for (name, child) in fs.read_dir(path).unwrap_or_default() {
                    self.scan_across(fs, &path.join(name), &child)?;
                }
            }
            Kind::File | Kind::Symlink => {
                self.progress.files_total += 1;
                if kind == Kind::File {
                    self.progress.bytes_total += meta.size;
                }
                self.report_progress();
            }
            Kind::Other => {}
        }
        Some(())
    }

    fn copy_across(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        target: &Path,
        delete_source: bool,
    ) -> Step {
        self.set_current(source);
        // A listing's folder is checked (a server may describe a symlink as
        // its target): a link is copied or moved as a link, never followed.
        let kind = match meta.kind {
            Kind::Dir => src.lstat(source).map_or(Kind::Dir, |m| m.kind),
            kind => kind,
        };
        let step = match kind {
            Kind::Dir => {
                return self.copy_dir_across(src, dst, source, meta, target, delete_source);
            }
            Kind::File => self.copy_file_across(src, dst, source, meta, target),
            Kind::Symlink => self.copy_link_across(src, dst, source, target),
            Kind::Other => {
                return self.fail(source, "only files, directories and symlinks can be copied");
            }
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
            if delete_source {
                return self.remove_moved(src, dst, source, target, false);
            }
        }
        step
    }

    /// After a move's copy: deletes the original (a file, a symlink or an
    /// emptied folder), unless it is the target itself under another name
    /// (two names for one host, or a host that is this machine), which only
    /// the file systems can tell. It is moved aside first, under a short
    /// name (a long one could exceed the name limit): if the target went
    /// with it, it is put back.
    fn remove_moved(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        target: &Path,
        folder: bool,
    ) -> Step {
        let cannot =
            |e: &dyn std::fmt::Display| format!("copied, but cannot delete the original: {e}");
        let parent = vfs::parent(source).unwrap_or_default();
        let mut aside = None;
        for n in 0..100 {
            let candidate = parent.join(format!(".yagni-moving-{n}"));
            match src.rename(source, &candidate) {
                Ok(()) => {
                    aside = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return self.fail(source, cannot(&e)),
            }
        }
        let Some(aside) = aside else {
            return self.fail(source, cannot(&"no free temporary name"));
        };
        // Put back where it was; if even that fails, say where it is.
        let restore = |src: &mut dyn Fs, why: String| match src.rename(&aside, source) {
            Ok(()) => why,
            Err(e) => format!("{why}; it is left as {} ({e})", aside.display()),
        };
        // Not found, or not even readable now: keep the original, to be safe.
        if dst.lstat(target).is_err() {
            let why = restore(
                src,
                "the source and the target are the same file under two names".to_owned(),
            );
            return self.fail(source, why);
        }
        let removed = if folder {
            src.remove_dir(&aside)
        } else {
            src.remove_file(&aside)
        };
        match removed {
            Ok(()) => {
                self.note(format_args!("removed {}", source.display()));
                Step::Done
            }
            Err(e) => {
                let why = restore(src, cannot(&e));
                self.fail(source, why)
            }
        }
    }

    fn copy_dir_across(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        target: &Path,
        delete_source: bool,
    ) -> Step {
        // A folder copied into itself under another name (two aliases of
        // one host) would never end.
        if target.components().count() > MAX_DEPTH {
            return self.fail(source, "folders nested too deep (copied into itself?)");
        }
        // A move deletes what it goes into: a listing's "folder" must be one
        // (a server may describe a symlink as its target).
        if delete_source && !src.lstat(source).is_ok_and(|m| m.kind == Kind::Dir) {
            return self.fail(source, "not a folder any more (a symlink?): left alone");
        }
        // A move from a local folder stops at mount points, as the local
        // engine's does: their contents are not this folder's to delete.
        if delete_source && src.host().is_none() && self.mounts.is_some() {
            use std::os::unix::fs::MetadataExt;
            let dev = std::fs::symlink_metadata(source).map(|m| m.dev());
            match (super::place(source), dev) {
                (Some((real, parent_dev)), Ok(dev))
                    if self.is_mount_point(&real, dev, parent_dev) =>
                {
                    return self.fail(source, super::MOUNT_POINT);
                }
                (Some(_), Ok(_)) => {}
                _ => return self.fail(source, "cannot tell whether this is a mount point"),
            }
        }
        match dst.make_dir(target, 0o700) {
            Ok(()) => self.note(format_args!("created directory {}", target.display())),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if dst.lstat(target).is_ok_and(|m| m.kind != Kind::Dir) {
                    return self.fail(target, "a file with this name exists");
                }
            }
            Err(e) => return self.fail(target, e),
        }
        let children = match src.read_dir(source) {
            Ok(children) => children,
            Err(e) => return self.fail(source, e),
        };
        let mut result = Step::Done;
        for (name, child) in children {
            let step = self.copy_across(
                src,
                dst,
                &source.join(&name),
                &child,
                &target.join(&name),
                delete_source,
            );
            match step {
                Step::Cancelled => return Step::Cancelled,
                Step::Incomplete => result = Step::Incomplete,
                Step::Done => {}
            }
        }
        if let Some(mode) = meta.mode {
            let _ = dst.set_mode(target, mode & 0o777);
        }
        if delete_source && result == Step::Done {
            return self.remove_moved(src, dst, source, target, true);
        }
        result
    }

    fn copy_file_across(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        target: &Path,
    ) -> Step {
        let start = self.progress.bytes_done;
        let step = match dst.create(target) {
            Ok(writer) => self.fill_across(src, dst, source, meta, target, writer),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                match self.conflict_across(dst, source, meta, target) {
                    Some(step) => step,
                    None => match self.temp_across(dst, target, |dst, temp| dst.create(temp)) {
                        Ok((temp, writer)) => {
                            match self.fill_across(src, dst, source, meta, &temp, writer) {
                                Step::Done => self.commit_across(dst, &temp, target),
                                step => step,
                            }
                        }
                        Err(e) => self.fail(target, e),
                    },
                }
            }
            Err(e) => self.fail(target, e),
        };
        // Skipped and failed files count as done, so the bar reaches the end.
        self.progress.bytes_done = self.progress.bytes_done.max(start + meta.size);
        step
    }

    /// `target` exists: the step to end with, or `None` to overwrite.
    fn conflict_across(
        &mut self,
        dst: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        target: &Path,
    ) -> Option<Step> {
        let there = dst.lstat(target).ok();
        if there.is_some_and(|m| m.kind == Kind::Dir) {
            return Some(self.fail(target, "a directory with this name exists"));
        }
        let incoming = Incoming {
            size: meta.size,
            modified: meta.modified(),
        };
        // The prompt can't look on a host: it is told.
        let existing = there.filter(|_| dst.host().is_some()).map(|m| Incoming {
            size: m.size,
            modified: m.modified(),
        });
        match self.choose_between(source, target, Some(incoming), existing) {
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

    /// A hidden temporary sibling of `target` made with `create`.
    fn temp_across<T>(
        &mut self,
        dst: &mut dyn Fs,
        target: &Path,
        mut create: impl FnMut(&mut dyn Fs, &Path) -> io::Result<T>,
    ) -> io::Result<(PathBuf, T)> {
        let name = vfs::file_name(target).unwrap_or_default();
        let folder = vfs::parent(target).unwrap_or_default();
        for n in 0..100 {
            let temp = folder.join(format!(".{}.yagni-{n}.tmp", name.to_string_lossy()));
            match create(dst, &temp) {
                Ok(made) => return Ok((temp, made)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }

    /// Renames the finished replacement `temp` over `target`.
    fn commit_across(&mut self, dst: &mut dyn Fs, temp: &Path, target: &Path) -> Step {
        match dst.rename_over(temp, target) {
            Ok(()) => Step::Done,
            Err(e) => {
                let _ = dst.remove_file(temp);
                self.fail(target, e)
            }
        }
    }

    /// Copies `source`'s contents into `writer` (a new file at `path`) with
    /// its time and permissions; on failure or cancel `path` is removed.
    fn fill_across(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        path: &Path,
        mut writer: vfs::Writer,
    ) -> Step {
        let result = {
            let progress = &mut self.progress;
            let observer = &mut *self.observer;
            src.read(source, &mut |offset, piece| {
                dst.write(&mut writer, offset, piece)?;
                progress.bytes_done += piece.len() as u64;
                observer.progress(progress);
                Ok(!observer.is_cancelled())
            })
        };
        match result {
            Ok(true) => match dst.finish(
                writer,
                meta.mtime,
                meta.mode.map(|m| m & 0o777),
                self.sync_copies,
            ) {
                Ok(()) => Step::Done,
                Err(e) => {
                    let _ = dst.remove_file(path);
                    self.fail(source, e)
                }
            },
            Ok(false) => {
                dst.abandon(writer, path);
                Step::Cancelled
            }
            Err(e) => {
                dst.abandon(writer, path);
                self.fail(source, e)
            }
        }
    }

    fn copy_link_across(
        &mut self,
        src: &mut dyn Fs,
        dst: &mut dyn Fs,
        source: &Path,
        target: &Path,
    ) -> Step {
        let link = match src.read_link(source) {
            Ok(link) => link,
            Err(e) => return self.fail(source, e),
        };
        match dst.symlink(&link, target) {
            Ok(()) => return Step::Done,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return self.fail(target, e),
        }
        let meta = Meta {
            kind: Kind::Symlink,
            size: 0,
            mtime: None,
            mode: None,
        };
        if let Some(step) = self.conflict_across(dst, source, &meta, target) {
            return step;
        }
        match self.temp_across(dst, target, |dst, temp| dst.symlink(&link, temp)) {
            Ok((temp, ())) => self.commit_across(dst, &temp, target),
            Err(e) => self.fail(target, e),
        }
    }

    /// A move on one host: a rename. An existing file asks; an existing
    /// folder is merged by copying into it and deleting the original.
    fn rename_across(
        &mut self,
        fs: &mut dyn Fs,
        source: &Path,
        meta: &Meta,
        target: &Path,
    ) -> Step {
        self.set_current(source);
        match fs.rename(source, target) {
            Ok(()) => {
                self.note(format_args!(
                    "moved {} -> {}",
                    source.display(),
                    target.display()
                ));
                return Step::Done;
            }
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return self.fail(source, e),
            Err(_) => {}
        }
        let target_is_dir = fs.lstat(target).is_ok_and(|m| m.kind == Kind::Dir);
        if meta.kind == Kind::Dir && target_is_dir {
            // Merge: the rest is a copy within the host, deleting as it goes.
            let Some(mut other) = open_or_fail(self, target, &[source.to_path_buf()]) else {
                return Step::Incomplete;
            };
            if self.scan_across(fs, source, meta).is_none() {
                return Step::Cancelled;
            }
            return self.copy_dir_across(fs, other.as_mut(), source, meta, target, true);
        }
        if let Some(step) = self.conflict_across(fs, source, meta, target) {
            return step;
        }
        match fs.rename_over(source, target) {
            Ok(()) => {
                self.note(format_args!(
                    "moved {} -> {}",
                    source.display(),
                    target.display()
                ));
                Step::Done
            }
            Err(e) => self.fail(source, e),
        }
    }

    /// Shift-F8 (and F8, no trash there) on a remote folder: deletes each
    /// source, folders with their contents. Symlinks are removed, never
    /// followed.
    pub(super) fn delete_across(&mut self, sources: &[PathBuf]) {
        self.progress.items_total = sources.len();
        let Some(first) = sources.first() else { return };
        let Some(mut fs) = open_or_fail(self, first, sources) else {
            self.progress.items_done = sources.len();
            return;
        };
        let mut work = Vec::new();
        for source in sources {
            let meta = if !on(fs.as_ref(), source) {
                Err("all sources must be in one place".to_owned())
            } else if vfs::file_name(source).is_none() {
                Err("a filesystem root cannot be deleted".to_owned())
            } else {
                fs.lstat(source).map_err(|e| e.to_string())
            };
            match meta {
                Ok(meta) => work.push((source, meta)),
                Err(message) => {
                    self.fail(source, message);
                    self.progress.items_done += 1;
                }
            }
        }
        for (source, meta) in &work {
            if self.scan_across(fs.as_mut(), source, meta).is_none() {
                self.report.cancelled = true;
                return;
            }
        }
        for (source, meta) in work {
            if self.delete_one_across(fs.as_mut(), source, &meta) == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    pub(super) fn delete_one_across(&mut self, fs: &mut dyn Fs, path: &Path, meta: &Meta) -> Step {
        if self.observer.is_cancelled() {
            return Step::Cancelled;
        }
        self.set_current(path);
        // A server may describe a symlink as what it points to in a
        // listing: only a real folder is gone into, never a link's target.
        let is_dir = if meta.kind == Kind::Dir {
            match fs.lstat(path) {
                Ok(real) => real.kind == Kind::Dir,
                Err(e) => return self.fail(path, e),
            }
        } else {
            false
        };
        if is_dir {
            let children = match fs.read_dir(path) {
                Ok(children) => children,
                Err(e) => return self.fail(path, e),
            };
            let mut result = Step::Done;
            for (name, child) in children {
                match self.delete_one_across(fs, &path.join(name), &child) {
                    Step::Cancelled => return Step::Cancelled,
                    Step::Incomplete => result = Step::Incomplete,
                    Step::Done => {}
                }
            }
            if result != Step::Done {
                return result;
            }
            return match fs.remove_dir(path) {
                Ok(()) => {
                    self.note(format_args!("deleted {}", path.display()));
                    Step::Done
                }
                Err(e) => self.fail(path, e),
            };
        }
        let result = fs.remove_file(path);
        self.progress.files_done += 1;
        match result {
            Ok(()) => {
                self.note(format_args!("deleted {}", path.display()));
                Step::Done
            }
            Err(e) => self.fail(path, e),
        }
    }

    /// F2 on a remote folder: renames `from` to `to` on the host (never
    /// over an existing entry).
    pub(super) fn rename_remote(&mut self, from: &Path, to: &Path) {
        self.progress.items_total = 1;
        if let Some(mut fs) = open_or_fail(self, from, &[from.to_path_buf()]) {
            match fs.rename(from, to) {
                Ok(()) => self.note(format_args!(
                    "renamed {} -> {}",
                    from.display(),
                    to.display()
                )),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let name = vfs::file_name(to).unwrap_or_default();
                    self.fail(to, format!("“{}” exists", name.to_string_lossy()));
                }
                Err(e) => {
                    self.fail(from, e);
                }
            }
        }
        self.progress.items_done = 1;
    }

    /// F7 and Shift-F4 on a remote folder: a new folder, or an empty file
    /// unless one exists (Shift-F4 then edits it). Missing folders above
    /// `path` are made too (`a/b`).
    pub(super) fn make_remote(&mut self, path: &Path, folder: bool) {
        self.progress.items_total = 1;
        self.make_one_remote(path, folder);
        self.progress.items_done = 1;
    }

    fn make_one_remote(&mut self, path: &Path, folder: bool) {
        let Some(mut fs) = open_or_fail(self, path, &[path.to_path_buf()]) else {
            return;
        };
        let exists = fs.lstat(path).ok();
        if let Some(meta) = exists
            && (folder || meta.kind == Kind::Dir)
        {
            let name = vfs::file_name(path).unwrap_or_default();
            self.fail(path, format!("“{}” exists", name.to_string_lossy()));
            return;
        }
        if exists.is_some() {
            return; // Shift-F4 on an existing file: it is edited as is.
        }
        if folder {
            if self.make_folders_across(fs.as_mut(), path) {
                let _ = fs.set_mode(path, 0o755);
            }
            return;
        }
        let parent = vfs::parent(path).unwrap_or_default();
        if !self.make_folders_across(fs.as_mut(), &parent) {
            return;
        }
        match fs
            .create(path)
            .and_then(|writer| fs.finish(writer, None, Some(0o644), false))
        {
            Ok(()) => self.note(format_args!("created {}", path.display())),
            Err(e) => {
                self.fail(path, e);
            }
        }
    }
}
