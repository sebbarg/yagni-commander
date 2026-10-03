//! Alt-F6 / Alt-F9: extracts archives into a folder. Smart extraction: an
//! archive whose entries share one top-level folder goes in as it is,
//! anything else into a folder named after it. Every write goes through
//! [`SafeDir`], so nothing in an archive can write outside the folder.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::archive_names::{Format, components, single_top_folder, stem};
use super::safe_dir::{Kind, SafeDir};
use super::{CHUNK, Choice, Engine, Incoming, Step};

/// Longer link targets are refused (PATH_MAX is 4096 on Linux).
const MAX_LINK_TARGET: u64 = 4096;

/// What an archive entry is.
pub(super) enum What {
    Dir,
    File,
    Symlink,
    /// A tar hard link, with the name it links to.
    HardLink(Vec<u8>),
    /// A device, FIFO or anything else we don't create.
    Special,
}

/// An archive entry's metadata, read before extracting (smart extraction,
/// progress totals) and again for each entry.
pub(super) struct Listed {
    /// The name as stored.
    pub(super) name: Vec<u8>,
    pub(super) parts: Result<Vec<OsString>, &'static str>,
    pub(super) what: What,
    pub(super) size: u64,
    /// rwx bits only.
    pub(super) mode: Option<u32>,
    pub(super) modified: Option<SystemTime>,
    pub(super) encrypted: bool,
}

/// Where one archive's entries go, and what has been written so far.
pub(super) struct Sink<'d> {
    pub(super) dir: &'d SafeDir,
    pub(super) archive: &'d Path,
    /// A folder below the extraction folder (smart extraction), or empty.
    pub(super) prefix: Vec<OsString>,
    /// Folders created, with the mode and time to set after their contents.
    folders: Vec<(Vec<OsString>, Option<u32>, Option<SystemTime>)>,
    /// Files extracted in this run: the only things a hard link may name.
    written: HashSet<Vec<OsString>>,
}

impl<'d> Sink<'d> {
    pub(super) fn new(dir: &'d SafeDir, archive: &'d Path, listed: &[Listed]) -> Self {
        let valid: Vec<(Vec<OsString>, bool)> = listed
            .iter()
            .filter_map(|item| {
                let parts = item.parts.as_ref().ok()?;
                Some((parts.clone(), matches!(item.what, What::Dir)))
            })
            .collect();
        let prefix = if single_top_folder(&valid) {
            Vec::new()
        } else {
            vec![stem(archive.file_name().unwrap_or_default())]
        };
        Self {
            dir,
            archive,
            prefix,
            folders: Vec::new(),
            written: HashSet::new(),
        }
    }

    fn parts(&self, entry: &[OsString]) -> Vec<OsString> {
        self.prefix.iter().chain(entry).cloned().collect()
    }
}

/// An entry's name for messages and the summary: `<archive>: <name>`.
fn inside(archive: &Path, name: &[u8]) -> PathBuf {
    PathBuf::from(format!(
        "{}: {}",
        archive.display(),
        String::from_utf8_lossy(name)
    ))
}

/// A zip timestamp (local time) as a `SystemTime`.
fn from_zip_time(time: Option<zip::DateTime>) -> Option<SystemTime> {
    let civil = jiff::civil::DateTime::try_from(time?).ok()?;
    let zoned = civil.to_zoned(jiff::tz::TimeZone::system()).ok()?;
    Some(SystemTime::from(zoned.timestamp()))
}

impl Engine<'_> {
    pub(super) fn extract(&mut self, archives: &[PathBuf], into: &Path) {
        self.progress.items_total = archives.len();
        if !self.make_folders(into) {
            return;
        }
        let dir = match SafeDir::open(into) {
            Ok(dir) => dir,
            Err(e) => {
                self.fail(into, e);
                return;
            }
        };
        for archive in archives {
            if self.observer.is_cancelled() {
                self.report.cancelled = true;
                return;
            }
            self.set_current(archive);
            let step = match archive.file_name().and_then(Format::from_name) {
                None => self.fail(archive, "not an archive"),
                Some(Format::Zip) => self.extract_zip(&dir, archive),
                Some(format) => self.extract_tar(&dir, archive, format),
            };
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    fn extract_zip(&mut self, dir: &SafeDir, archive: &Path) -> Step {
        let file = match File::open(archive) {
            Ok(file) => file,
            Err(e) => return self.fail(archive, e),
        };
        let mut zip = match zip::ZipArchive::new(BufReader::new(file)) {
            Ok(zip) => zip,
            Err(e) => return self.fail(archive, e),
        };
        let mut listed = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let entry = match zip.by_index_raw(i) {
                Ok(entry) => entry,
                Err(e) => return self.fail(archive, e),
            };
            let name = entry.name().as_bytes().to_vec();
            let what = if entry.is_dir() {
                What::Dir
            } else if entry.is_symlink() {
                What::Symlink
            } else {
                What::File
            };
            listed.push(Listed {
                parts: components(&name, true),
                name,
                what,
                size: entry.size(),
                mode: entry.unix_mode().map(|mode| mode & 0o777),
                modified: from_zip_time(entry.last_modified()),
                encrypted: entry.encrypted(),
            });
        }
        let mut sink = Sink::new(dir, archive, &listed);
        self.count(&listed);
        let mut step = Step::Done;
        for (i, item) in listed.iter().enumerate() {
            if self.observer.is_cancelled() {
                return self.end_archive(&mut sink, Step::Cancelled);
            }
            let needs_data = !item.encrypted
                && item.parts.is_ok()
                && matches!(item.what, What::File | What::Symlink);
            let entry_step = if needs_data {
                match zip.by_index(i) {
                    Ok(mut entry) => self.put(&mut sink, item, None, &mut entry),
                    Err(e) => self.fail(&inside(archive, &item.name), e),
                }
            } else {
                self.put(&mut sink, item, None, &mut io::empty())
            };
            match entry_step {
                Step::Done => {}
                Step::Incomplete => step = Step::Incomplete,
                Step::Cancelled => return self.end_archive(&mut sink, Step::Cancelled),
            }
        }
        self.end_archive(&mut sink, step)
    }

    fn extract_tar(&mut self, dir: &SafeDir, archive: &Path, format: Format) -> Step {
        // First pass: names, kinds, sizes (smart extraction, progress). It
        // stops quietly at a read error; the second pass reports it.
        let listed = list_tar(archive, format);
        let mut sink = Sink::new(dir, archive, &listed);
        self.count(&listed);
        let reader = match tar_reader(archive, format) {
            Ok(reader) => reader,
            Err(e) => return self.fail(archive, e),
        };
        let mut tar = tar::Archive::new(reader);
        let entries = match tar.entries() {
            Ok(entries) => entries,
            Err(e) => return self.fail(archive, e),
        };
        let mut step = Step::Done;
        for entry in entries {
            if self.observer.is_cancelled() {
                return self.end_archive(&mut sink, Step::Cancelled);
            }
            let mut entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.fail(archive, e);
                    return self.end_archive(&mut sink, Step::Incomplete);
                }
            };
            if is_tar_metadata(&entry) {
                continue;
            }
            let item = listed_tar(&entry);
            let link_target = entry.link_name_bytes().map(|bytes| bytes.into_owned());
            match self.put(&mut sink, &item, link_target, &mut entry) {
                Step::Done => {}
                Step::Incomplete => step = Step::Incomplete,
                Step::Cancelled => return self.end_archive(&mut sink, Step::Cancelled),
            }
        }
        self.end_archive(&mut sink, step)
    }

    /// Adds an archive's files and bytes to the progress totals.
    pub(super) fn count(&mut self, listed: &[Listed]) {
        for item in listed.iter().filter(|item| matches!(item.what, What::File)) {
            self.progress.files_total += 1;
            self.progress.bytes_total += item.size;
        }
        self.report_progress();
    }

    /// Writes one entry. `link_target` is a tar symlink's target; a zip
    /// symlink's target is its data, read from `data`.
    pub(super) fn put(
        &mut self,
        sink: &mut Sink,
        item: &Listed,
        link_target: Option<Vec<u8>>,
        data: &mut dyn Read,
    ) -> Step {
        let shown = inside(sink.archive, &item.name);
        let parts = match &item.parts {
            Ok(parts) => sink.parts(parts),
            Err(why) => return self.fail(&shown, why),
        };
        if item.encrypted {
            return self.fail(&shown, "password-protected archives are not supported yet");
        }
        match &item.what {
            What::Dir => self.put_dir(sink, parts, item),
            What::File => self.put_file(sink, parts, item, data),
            What::Symlink => {
                let target = match link_target {
                    Some(target) => target,
                    None => {
                        // Bounded: a huge "link" entry must not fill memory.
                        let mut target = Vec::new();
                        if let Err(e) = data.take(MAX_LINK_TARGET + 1).read_to_end(&mut target) {
                            return self.fail(&shown, e);
                        }
                        if target.len() as u64 > MAX_LINK_TARGET {
                            return self.fail(&shown, "a link target that long is not allowed");
                        }
                        target
                    }
                };
                self.put_symlink(sink, &parts, &target, &shown)
            }
            What::HardLink(target) => self.put_hard_link(sink, &parts, target, &shown),
            What::Special => self.fail(&shown, "special files (devices, pipes) are not extracted"),
        }
    }

    fn put_dir(&mut self, sink: &mut Sink, parts: Vec<OsString>, item: &Listed) -> Step {
        let shown = inside(sink.archive, &item.name);
        let created = match sink.dir.kind(&parts) {
            Ok(None) => true,
            Ok(Some(Kind::Dir)) => false,
            Ok(Some(_)) => return self.fail(&shown, "a file with this name exists"),
            Err(e) => return self.fail(&shown, e),
        };
        if let Err(e) = sink.dir.dir(&parts, true) {
            return self.fail(&shown, e);
        }
        if created {
            self.note(format_args!(
                "created directory {}",
                sink.dir.path_of(&parts).display()
            ));
            sink.folders.push((parts, item.mode, item.modified));
        }
        Step::Done
    }

    fn put_file(
        &mut self,
        sink: &mut Sink,
        parts: Vec<OsString>,
        item: &Listed,
        data: &mut dyn Read,
    ) -> Step {
        let shown = inside(sink.archive, &item.name);
        let target = sink.dir.path_of(&parts);
        let start = self.progress.bytes_done;
        let step = match sink.dir.kind(&parts) {
            Err(e) => self.fail(&shown, e),
            Ok(Some(Kind::Dir)) => self.fail(&shown, "a folder with this name exists"),
            Ok(None) => match sink.dir.create_file(&parts, item.mode) {
                Ok(file) => self.fill(sink, &parts, None, file, item, data),
                Err(e) => self.fail(&shown, e),
            },
            Ok(Some(_)) => {
                let incoming = Incoming {
                    size: item.size,
                    modified: item.modified,
                };
                match self.choose(&shown, &target, Some(incoming)) {
                    Choice::Skip => {
                        self.note(format_args!("skipped {}: exists", target.display()));
                        self.report.skipped += 1;
                        Step::Incomplete
                    }
                    Choice::Cancel => Step::Cancelled,
                    Choice::Overwrite => match sink.dir.create_temp_beside(&parts, item.mode) {
                        Ok((temp, file)) => self.fill(sink, &parts, Some(&temp), file, item, data),
                        Err(e) => self.fail(&shown, e),
                    },
                }
            }
        };
        if step != Step::Cancelled {
            self.progress.files_done += 1;
        }
        // Skipped and failed files count as done, so the bar reaches the end.
        self.progress.bytes_done = self.progress.bytes_done.max(start + item.size);
        if step == Step::Done {
            sink.written.insert(parts);
        }
        step
    }

    /// Copies the entry's data into `file`, which is `parts` itself or the
    /// temp `temp` beside it (renamed over it at the end). Removes the file
    /// again on error or cancel.
    fn fill(
        &mut self,
        sink: &Sink,
        parts: &[OsString],
        temp: Option<&OsStr>,
        mut file: File,
        item: &Listed,
        data: &mut dyn Read,
    ) -> Step {
        let shown = inside(sink.archive, &item.name);
        // A truncated tar gives a short read, not an error: check the size.
        let result = self
            .copy_data(data, &mut file)
            .and_then(|copied| match copied {
                Some(total) if total < item.size => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the archive ends early",
                )),
                Some(_) => Ok(true),
                None => Ok(false),
            });
        let remove = |dir: &SafeDir| match temp {
            Some(temp) => dir.remove_temp(parts, temp),
            None => dir.remove_file(parts),
        };
        match result {
            Ok(true) => {
                if let Some(modified) = item.modified {
                    let _ = file.set_modified(modified);
                }
                drop(file);
                if let Some(temp) = temp
                    && let Err(e) = sink.dir.replace(parts, temp)
                {
                    let _ = sink.dir.remove_temp(parts, temp);
                    return self.fail(&shown, e);
                }
                self.note(format_args!(
                    "created {}",
                    sink.dir.path_of(parts).display()
                ));
                Step::Done
            }
            Ok(false) => {
                drop(file);
                let _ = remove(sink.dir);
                Step::Cancelled
            }
            Err(e) => {
                drop(file);
                let _ = remove(sink.dir);
                self.fail(&shown, e)
            }
        }
    }

    /// Copies `data` into `out` in chunks: the bytes copied, or `None` if
    /// cancelled part way.
    fn copy_data(&mut self, data: &mut dyn Read, out: &mut File) -> io::Result<Option<u64>> {
        let mut total = 0;
        loop {
            if self.observer.is_cancelled() {
                return Ok(None);
            }
            let copied = io::copy(&mut (&mut *data).take(CHUNK), out)?;
            if copied == 0 {
                return Ok(Some(total));
            }
            total += copied;
            self.progress.bytes_done += copied;
            self.report_progress();
        }
    }

    fn put_symlink(
        &mut self,
        sink: &mut Sink,
        parts: &[OsString],
        target: &[u8],
        shown: &Path,
    ) -> Step {
        let link_target = OsStr::from_bytes(target);
        match sink.dir.kind(parts) {
            Err(e) => return self.fail(shown, e),
            Ok(Some(Kind::Dir)) => return self.fail(shown, "a folder with this name exists"),
            Ok(None) => {}
            Ok(Some(_)) => {
                let incoming = Incoming {
                    size: target.len() as u64,
                    modified: None,
                };
                let path = sink.dir.path_of(parts);
                match self.choose(shown, &path, Some(incoming)) {
                    Choice::Skip => {
                        self.report.skipped += 1;
                        return Step::Incomplete;
                    }
                    Choice::Cancel => return Step::Cancelled,
                    Choice::Overwrite => {
                        if let Err(e) = sink.dir.remove_file(parts) {
                            return self.fail(shown, e);
                        }
                    }
                }
            }
        }
        match sink.dir.symlink(parts, link_target) {
            Ok(()) => {
                sink.written.remove(parts);
                self.note(format_args!(
                    "created link {}",
                    sink.dir.path_of(parts).display()
                ));
                Step::Done
            }
            Err(e) => self.fail(shown, e),
        }
    }

    fn put_hard_link(
        &mut self,
        sink: &mut Sink,
        parts: &[OsString],
        target: &[u8],
        shown: &Path,
    ) -> Step {
        let existing = match components(target, false) {
            Ok(entry) => sink.parts(&entry),
            Err(why) => return self.fail(shown, why),
        };
        if !sink.written.contains(&existing) {
            return self.fail(shown, "a hard link to something outside this archive");
        }
        if let Ok(Some(_)) = sink.dir.kind(parts) {
            return self.fail(shown, "an entry with this name exists");
        }
        match sink.dir.hard_link(&existing, parts) {
            Ok(()) => {
                sink.written.insert(parts.to_vec());
                self.note(format_args!(
                    "created link {}",
                    sink.dir.path_of(parts).display()
                ));
                Step::Done
            }
            Err(e) => self.fail(shown, e),
        }
    }

    /// Sets the folders' modes and times (deepest first, after their
    /// contents) and logs the archive.
    pub(super) fn end_archive(&mut self, sink: &mut Sink, step: Step) -> Step {
        sink.folders
            .sort_by_key(|(parts, ..)| std::cmp::Reverse(parts.len()));
        for (parts, mode, modified) in sink.folders.drain(..) {
            sink.dir.set_dir_mode_and_time(&parts, mode, modified);
        }
        if step != Step::Cancelled {
            self.note(format_args!(
                "extracted {} -> {}",
                sink.archive.display(),
                sink.dir.path_of(&sink.prefix).display()
            ));
        }
        step
    }
}

/// The tar stream inside `path`, decompressed.
fn tar_reader(path: &Path, format: Format) -> io::Result<Box<dyn Read>> {
    let file = BufReader::new(File::open(path)?);
    Ok(match format {
        Format::Tar => Box::new(file),
        Format::TarGz => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Format::TarBz2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        Format::TarXz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(file)),
        Format::TarZst => Box::new(zstd::stream::read::Decoder::new(file)?),
        Format::Zip => return Err(io::Error::other("a zip is not a tar")),
    })
}

/// A tar's entries, up to the first read error.
fn list_tar(path: &Path, format: Format) -> Vec<Listed> {
    let Ok(reader) = tar_reader(path, format) else {
        return Vec::new();
    };
    let mut tar = tar::Archive::new(reader);
    let Ok(entries) = tar.entries() else {
        return Vec::new();
    };
    entries
        .map_while(Result::ok)
        .filter(|entry| !is_tar_metadata(entry))
        .map(|entry| listed_tar(&entry))
        .collect()
}

/// A PAX global header (`git archive` writes one) or a GNU volume label:
/// about the archive, not a file in it. The tar crate yields them as entries.
fn is_tar_metadata<R: Read>(entry: &tar::Entry<R>) -> bool {
    let kind = entry.header().entry_type();
    kind.is_pax_global_extensions() || kind.as_byte() == b'V'
}

fn listed_tar<R: Read>(entry: &tar::Entry<R>) -> Listed {
    use tar::EntryType;
    let header = entry.header();
    let name = entry.path_bytes().into_owned();
    let kind = header.entry_type();
    let what = if kind.is_dir() {
        What::Dir
    } else if kind.is_file() || matches!(kind, EntryType::Continuous | EntryType::GNUSparse) {
        What::File
    } else if kind.is_symlink() {
        What::Symlink
    } else if kind.is_hard_link() {
        What::HardLink(
            entry
                .link_name_bytes()
                .map(|b| b.into_owned())
                .unwrap_or_default(),
        )
    } else {
        What::Special
    };
    Listed {
        parts: components(&name, false),
        name,
        what,
        size: entry.size(),
        mode: header.mode().ok().map(|mode| mode & 0o777),
        modified: header
            .mtime()
            .ok()
            .map(|secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        encrypted: false,
    }
}
