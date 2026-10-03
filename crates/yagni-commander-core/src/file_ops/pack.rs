//! Alt-F5: packs sources into a new zip. It is written to a hidden temp
//! file next to the target and renamed over it at the end, so a cancelled
//! or failed pack leaves no zip, and an old zip stays intact until then.
//! Symlinks are never followed without asking ([`Observer::link`]).

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::{CHUNK, Engine, LinkChoice, LinkPlace, LinkQuestion, Step, create_new, create_temp};

/// A file or folder identity (device, inode).
type Id = (u64, u64);

struct Packer {
    zip: ZipWriter<File>,
    /// The temp zip and the zip it replaces, never packed into the new one.
    skip: Vec<Id>,
    /// Folders on the way down, to catch link loops when following links.
    stack: Vec<Id>,
    /// The sources, canonical, to tell whether a link points inside them.
    roots: Vec<PathBuf>,
}

impl Engine<'_> {
    pub(super) fn pack(&mut self, sources: &[PathBuf], base: &Path, to: &Path) {
        self.progress.items_total = sources.len();
        let folder = to.parent().unwrap_or(Path::new("/"));
        if !self.make_folders(folder) {
            return;
        }
        for source in sources {
            if self.scan(source).is_none() {
                self.report.cancelled = true;
                return;
            }
        }
        let (temp, file) = match create_temp(to, create_new) {
            Ok(created) => created,
            Err(e) => {
                self.fail(to, e);
                return;
            }
        };
        let skip = [file.metadata(), to.symlink_metadata()]
            .into_iter()
            .filter_map(Result::ok)
            .filter(Metadata::is_file)
            .map(|m| (m.dev(), m.ino()))
            .collect();
        let mut packer = Packer {
            zip: ZipWriter::new(file),
            skip,
            stack: Vec::new(),
            roots: sources
                .iter()
                .filter_map(|s| s.canonicalize().ok())
                .collect(),
        };
        for source in sources {
            if self.pack_entry(&mut packer, source, &entry_name(base, source)) == Step::Cancelled {
                drop(packer);
                let _ = fs::remove_file(&temp);
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
        let written = packer
            .zip
            .finish()
            .map_err(io::Error::other)
            .and_then(|_| fs::rename(&temp, to));
        match written {
            Ok(()) => {
                let what = if sources.len() == 1 {
                    "entry"
                } else {
                    "entries"
                };
                self.note(format_args!(
                    "packed {} ({} {what})",
                    to.display(),
                    sources.len()
                ));
            }
            Err(e) => {
                let _ = fs::remove_file(&temp);
                self.fail(to, e);
            }
        }
    }

    fn pack_entry(&mut self, packer: &mut Packer, path: &Path, name: &str) -> Step {
        if self.observer.is_cancelled() {
            return Step::Cancelled;
        }
        self.set_current(path);
        let meta = match path.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) => return self.fail(path, e),
        };
        if meta.is_file() && packer.skip.contains(&(meta.dev(), meta.ino())) {
            return Step::Done;
        }
        if meta.is_symlink() {
            return self.pack_link(packer, path, name, &meta);
        }
        self.pack_resolved(packer, path, name, &meta)
    }

    /// A file or folder; a followed link arrives here with its target's
    /// metadata, under the link's name.
    fn pack_resolved(
        &mut self,
        packer: &mut Packer,
        path: &Path,
        name: &str,
        meta: &Metadata,
    ) -> Step {
        if meta.is_dir() {
            self.pack_dir(packer, path, name, meta)
        } else if meta.is_file() {
            self.pack_file(packer, path, name, meta)
        } else {
            self.fail(path, "only files, folders and symbolic links can be packed")
        }
    }

    fn pack_dir(&mut self, packer: &mut Packer, path: &Path, name: &str, meta: &Metadata) -> Step {
        let id = (meta.dev(), meta.ino());
        if packer.stack.contains(&id) {
            return self.fail(path, "a symbolic link loop");
        }
        if let Err(e) = packer.zip.add_directory(format!("{name}/"), options(meta)) {
            return self.fail(path, e);
        }
        let mut children: Vec<_> = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect(),
            Err(e) => return self.fail(path, e),
        };
        children.sort();
        packer.stack.push(id);
        let mut step = Step::Done;
        for child in children {
            let child_name = format!("{name}/{}", child.to_string_lossy());
            match self.pack_entry(packer, &path.join(&child), &child_name) {
                Step::Done => {}
                Step::Incomplete => step = Step::Incomplete,
                Step::Cancelled => {
                    step = Step::Cancelled;
                    break;
                }
            }
        }
        packer.stack.pop();
        step
    }

    fn pack_file(&mut self, packer: &mut Packer, path: &Path, name: &str, meta: &Metadata) -> Step {
        let start = self.progress.bytes_done;
        let options = options(meta).large_file(meta.len() >= u64::from(u32::MAX));
        let result = match packer.zip.start_file(name, options) {
            Ok(()) => self.pack_contents(&mut packer.zip, path).inspect_err(|_| {
                // Drops this entry; without a started entry it would drop
                // the previous one.
                let _ = packer.zip.abort_file();
            }),
            Err(e) => Err(io::Error::other(e)),
        };
        // Skipped and failed files count as done, so the bar reaches the end;
        // a followed link's file was not in the totals.
        self.progress.files_done += 1;
        self.progress.bytes_done = self.progress.bytes_done.max(start + meta.len());
        self.progress.files_total = self.progress.files_total.max(self.progress.files_done);
        self.progress.bytes_total = self.progress.bytes_total.max(self.progress.bytes_done);
        match result {
            Ok(true) => {
                self.note(format_args!("added {}", path.display()));
                Step::Done
            }
            Ok(false) => Step::Cancelled,
            Err(e) => self.fail(path, e),
        }
    }

    /// Copies `path` into the zip entry just started. False if cancelled.
    fn pack_contents(&mut self, zip: &mut ZipWriter<File>, path: &Path) -> io::Result<bool> {
        let mut input = File::open(path)?;
        loop {
            if self.observer.is_cancelled() {
                return Ok(false);
            }
            let copied = io::copy(&mut (&mut input).take(CHUNK), zip)?;
            if copied == 0 {
                return Ok(true);
            }
            self.progress.bytes_done += copied;
            self.report_progress();
        }
    }

    fn pack_link(&mut self, packer: &mut Packer, path: &Path, name: &str, meta: &Metadata) -> Step {
        let target = match fs::read_link(path) {
            Ok(target) => target,
            Err(e) => return self.fail(path, e),
        };
        let choice = match self.link_choice {
            Some(choice) => choice,
            None => {
                let question = LinkQuestion {
                    link: path.to_path_buf(),
                    shown: PathBuf::from(name),
                    target: target.clone(),
                    place: link_place(path, &packer.roots),
                };
                let answer = self.observer.link(&question);
                if answer.for_all {
                    self.link_choice = Some(answer.choice);
                }
                answer.choice
            }
        };
        match choice {
            LinkChoice::Cancel => Step::Cancelled,
            LinkChoice::LeaveOut => {
                self.note(format_args!("left out {}", path.display()));
                self.report.left_out.push(path.to_path_buf());
                Step::Done
            }
            LinkChoice::Store => {
                match packer
                    .zip
                    .add_symlink(name, target.to_string_lossy(), options(meta))
                {
                    Ok(()) => {
                        self.note(format_args!("added link {}", path.display()));
                        Step::Done
                    }
                    Err(e) => self.fail(path, e),
                }
            }
            LinkChoice::Follow => match fs::metadata(path) {
                Ok(resolved) => self.pack_resolved(packer, path, name, &resolved),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    self.fail(path, "the link's target does not exist")
                }
                Err(e) => self.fail(path, e),
            },
        }
    }
}

/// `path`'s name in the zip: relative to `base`, `/`-separated (zip names
/// are text, so non-UTF-8 bytes become U+FFFD).
fn entry_name(base: &Path, path: &Path) -> String {
    let relative = match path.strip_prefix(base) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_path_buf(),
        _ => PathBuf::from(path.file_name().unwrap_or_default()),
    };
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn link_place(link: &Path, roots: &[PathBuf]) -> LinkPlace {
    match link.canonicalize() {
        Err(_) => LinkPlace::Missing,
        Ok(real) if roots.iter().any(|root| real.starts_with(root)) => LinkPlace::Inside,
        Ok(_) => LinkPlace::Outside,
    }
}

fn options(meta: &Metadata) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(meta.permissions().mode() & 0o777)
        .last_modified_time(zip_time(meta.modified()))
}

/// A zip timestamp (local time, 1980 to 2107); out of range gives 1980.
pub(super) fn zip_time(time: io::Result<SystemTime>) -> zip::DateTime {
    time.ok()
        .and_then(|t| jiff::Timestamp::try_from(t).ok())
        .map(|ts| ts.to_zoned(jiff::tz::TimeZone::system()).datetime())
        .and_then(|dt| zip::DateTime::try_from(dt).ok())
        .unwrap_or_default()
}
