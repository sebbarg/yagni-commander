//! Alt-F6 / Alt-F9: extracts archives into a folder. Smart extraction: an
//! archive whose entries share one top-level folder goes in as it is,
//! anything else into a folder named after it. Every write goes through
//! [`SafeDir`], so nothing in an archive can write outside the folder.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::archive_names::{Format, components, single_top_folder, stem};
use super::safe_dir::{Kind, SafeDir};
use super::{CHUNK, Choice, Destination, Engine, Incoming, PasswordAnswer, PasswordQuestion, Step};

/// Longer link targets are refused (PATH_MAX is 4096 on Linux).
const MAX_LINK_TARGET: u64 = 4096;

/// What an archive entry is.
pub(crate) enum What {
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
pub(crate) struct Listed {
    /// The name as stored.
    pub(crate) name: Vec<u8>,
    pub(crate) parts: Result<Vec<OsString>, &'static str>,
    pub(crate) what: What,
    pub(crate) size: u64,
    /// rwx bits only.
    pub(crate) mode: Option<u32>,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) encrypted: bool,
    /// `user:group` as a tar stores them; zips have none.
    pub(crate) owner: Option<String>,
}

/// Which entries an F5 or F3 out of an archive takes, and where they go.
pub(super) struct Pick {
    /// The folder inside the archive that `names` are in.
    inner: Vec<OsString>,
    names: Vec<OsString>,
    /// [`Destination::As`]: the one entry's new name.
    rename: Option<OsString>,
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
    /// Only these entries, without smart extraction.
    pick: Option<&'d Pick>,
    /// With a pick: entries (by their place in the listing) that a later
    /// entry of the same name replaces, as the panel shows only the last.
    superseded: HashSet<usize>,
}

impl<'d> Sink<'d> {
    pub(super) fn new(
        dir: &'d SafeDir,
        archive: &'d Path,
        listed: &[Listed],
        pick: Option<&'d Pick>,
    ) -> Self {
        let valid: Vec<(Vec<OsString>, bool)> = listed
            .iter()
            .filter_map(|item| {
                let parts = item.parts.as_ref().ok()?;
                Some((parts.clone(), matches!(item.what, What::Dir)))
            })
            .collect();
        let prefix = if pick.is_some() || single_top_folder(&valid) {
            Vec::new()
        } else {
            vec![stem(archive.file_name().unwrap_or_default())]
        };
        let mut sink = Self {
            dir,
            archive,
            prefix,
            folders: Vec::new(),
            written: HashSet::new(),
            pick,
            superseded: HashSet::new(),
        };
        if pick.is_some() {
            let mut last = std::collections::HashMap::new();
            for (i, item) in listed.iter().enumerate() {
                if let Some(parts) = item.parts.as_ref().ok().and_then(|p| sink.parts(p))
                    && let Some(earlier) = last.insert(parts, i)
                {
                    sink.superseded.insert(earlier);
                }
            }
        }
        sink
    }

    /// Where an entry goes below the extraction folder, or `None` when
    /// this extraction doesn't take it.
    fn parts(&self, entry: &[OsString]) -> Option<Vec<OsString>> {
        let Some(pick) = self.pick else {
            return Some(self.prefix.iter().chain(entry).cloned().collect());
        };
        let rest = entry.strip_prefix(pick.inner.as_slice())?;
        let (top, below) = rest.split_first()?;
        if !pick.names.contains(top) {
            return None;
        }
        let top = pick.rename.clone().unwrap_or_else(|| top.clone());
        Some(std::iter::once(top).chain(below.iter().cloned()).collect())
    }

    /// Whether this extraction takes `item`, the `i`th entry listed. An
    /// unsafe name is taken only without a pick, so that it is reported.
    pub(super) fn takes(&self, i: usize, item: &Listed) -> bool {
        match &item.parts {
            Ok(parts) => self.parts(parts).is_some() && !self.superseded.contains(&i),
            Err(_) => self.pick.is_none(),
        }
    }

    /// Picked names that no entry has (the panel's index is out of date).
    fn missing(&self, listed: &[Listed]) -> Vec<Vec<u8>> {
        let Some(pick) = self.pick else {
            return Vec::new();
        };
        let found: HashSet<&OsString> = listed
            .iter()
            .filter_map(|item| item.parts.as_ref().ok())
            .filter_map(|parts| parts.strip_prefix(pick.inner.as_slice())?.first())
            .collect();
        pick.names
            .iter()
            .filter(|name| !found.contains(name))
            .map(|name| {
                let mut path = PathBuf::new();
                path.extend(&pick.inner);
                path.push(name);
                path.into_os_string().into_vec()
            })
            .collect()
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

/// An encrypted entry's data. A read error there almost always means a
/// wrong password that passed the zip's quick check (1 in 256 for
/// ZipCrypto), so it says that instead of "Invalid checksum".
struct Decrypted<R>(R);

impl<R: Read> Read for Decrypted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .read(buf)
            .map_err(|_| io::Error::other(WRONG_PASSWORD))
    }
}

/// The failure for an encrypted entry whose data doesn't check out.
const WRONG_PASSWORD: &str = "wrong password or a damaged entry";

/// PBKDF2 rounds of zip's AES encryption (WinZip's AE-1/AE-2 format).
const AES_ROUNDS: u32 = 1000;
/// Length of the authentication code at the end of an AES entry.
const AES_CODE_LEN: u64 = 10;

/// Whether an AES entry's authentication code matches `password`; true for
/// entries this check isn't needed for. zip checks the code only when the
/// encrypted data is read to its end, which decompression may stop short
/// of, and AE-2 entries (CRC 0) have no CRC to fall back on: without this,
/// a wrong password passing the 2-byte check (1 in 65536) could leave a
/// garbage file. Stored entries are always read to the end.
fn aes_authentic<R: Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    index: usize,
    password: &[u8],
) -> io::Result<bool> {
    use hmac::{KeyInit, Mac, SimpleHmac};
    let Some(info) = zip
        .get_aes_verification_key_and_salt(index)
        .map_err(io::Error::other)?
    else {
        return Ok(true);
    };
    let mut raw = zip.by_index_raw(index).map_err(io::Error::other)?;
    if raw.crc32() != 0 || raw.compression() == zip::CompressionMethod::Stored {
        return Ok(true);
    }
    let key_len = info.aes_mode.key_length();
    let mut derived = vec![0; 2 * key_len + 2];
    pbkdf2::pbkdf2::<SimpleHmac<sha1::Sha1>>(password, &info.salt, AES_ROUNDS, &mut derived)
        .map_err(io::Error::other)?;
    let mut mac = SimpleHmac::<sha1::Sha1>::new_from_slice(&derived[key_len..2 * key_len])
        .map_err(io::Error::other)?;
    // The raw data: salt, 2 verification bytes, the encrypted data, the code.
    let header = info.salt.len() as u64 + 2;
    let data_len = raw
        .compressed_size()
        .checked_sub(header + AES_CODE_LEN)
        .ok_or_else(|| io::Error::other(WRONG_PASSWORD))?;
    io::copy(&mut (&mut raw).take(header), &mut io::sink())?;
    let mut data = (&mut raw).take(data_len);
    let mut buf = vec![0; 64 << 10];
    loop {
        let read = data.read(&mut buf)?;
        if read == 0 {
            break;
        }
        mac.update(&buf[..read]);
    }
    let mut code = [0; AES_CODE_LEN as usize];
    raw.read_exact(&mut code)?;
    Ok(mac.finalize().into_bytes()[..code.len()] == code)
}

impl Engine<'_> {
    pub(super) fn extract(&mut self, archives: &[PathBuf], into: &Path) {
        self.extract_into(archives, into, None);
    }

    /// F5 and F3 inside an archive: `names` from folder `inner`.
    pub(super) fn extract_entries(
        &mut self,
        archive: &Path,
        inner: &[OsString],
        names: &[OsString],
        to: &Destination,
    ) {
        let (into, rename) = match to {
            Destination::Into(dir) => (dir.as_path(), None),
            Destination::As(path) => match (path.parent(), path.file_name()) {
                (Some(dir), Some(name)) if names.len() == 1 => (dir, Some(name.to_os_string())),
                _ => {
                    self.fail(path, "not a valid target");
                    return;
                }
            },
        };
        let pick = Pick {
            inner: inner.to_vec(),
            names: names.to_vec(),
            rename,
        };
        let archives = [archive.to_path_buf()];
        self.extract_into(&archives, into, Some(&pick));
    }

    fn extract_into(&mut self, archives: &[PathBuf], into: &Path, pick: Option<&Pick>) {
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
                Some(Format::Zip) => self.extract_zip(&dir, archive, pick),
                Some(format) => self.extract_tar(&dir, archive, format, pick),
            };
            if step == Step::Cancelled {
                self.report.cancelled = true;
                return;
            }
            self.progress.items_done += 1;
            self.report_progress();
        }
    }

    fn extract_zip(&mut self, dir: &SafeDir, archive: &Path, pick: Option<&Pick>) -> Step {
        let file = match File::open(archive) {
            Ok(file) => file,
            Err(e) => return self.fail(archive, e),
        };
        let mut zip = match zip::ZipArchive::new(BufReader::new(file)) {
            Ok(zip) => zip,
            Err(e) => return self.fail(archive, e),
        };
        let listed = match list_zip(&mut zip, || true) {
            Ok(listed) => listed,
            Err(e) => return self.fail(archive, e),
        };
        let mut sink = Sink::new(dir, archive, &listed, pick);
        self.count(&listed, &sink);
        let mut step = Step::Done;
        let mut skip_locked = false;
        for (i, item) in listed.iter().enumerate() {
            if self.observer.is_cancelled() {
                return self.end_archive(&mut sink, Step::Cancelled);
            }
            if !sink.takes(i, item) {
                continue;
            }
            let needs_data = item.parts.is_ok() && matches!(item.what, What::File | What::Symlink);
            let entry_step = if needs_data && item.encrypted {
                self.put_encrypted(&mut zip, i, &mut sink, item, &mut skip_locked)
            } else if needs_data {
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

    /// An encrypted zip entry: opened with the job's password, asking for
    /// one until it fits, the user skips the archive, or cancels.
    fn put_encrypted<R: Read + io::Seek>(
        &mut self,
        zip: &mut zip::ZipArchive<R>,
        index: usize,
        sink: &mut Sink,
        item: &Listed,
        skip_locked: &mut bool,
    ) -> Step {
        let shown = inside(sink.archive, &item.name);
        if *skip_locked {
            return self.skip_locked(&shown, item);
        }
        // The remembered password is tried once per entry before asking;
        // a question is a retry only after a typed password didn't fit.
        let mut remembered_tried = false;
        let mut typed = false;
        loop {
            let password = match &self.password {
                Some(password) if !remembered_tried => {
                    remembered_tried = true;
                    password.clone()
                }
                _ => {
                    let question = PasswordQuestion {
                        archive: sink.archive.to_path_buf(),
                        retry: typed,
                    };
                    match self.observer.password(&question) {
                        PasswordAnswer::Password(password) => {
                            typed = true;
                            remembered_tried = true;
                            self.password = Some(password.clone());
                            password
                        }
                        PasswordAnswer::Skip => {
                            *skip_locked = true;
                            return self.skip_locked(&shown, item);
                        }
                        PasswordAnswer::Cancel => return Step::Cancelled,
                    }
                }
            };
            match zip.by_index_decrypt(index, password.as_bytes()) {
                Ok(_) => {}
                Err(zip::result::ZipError::InvalidPassword) => continue,
                Err(e) => return self.fail(&shown, e),
            }
            match aes_authentic(zip, index, password.as_bytes()) {
                Ok(true) => {}
                Ok(false) => return self.fail(&shown, WRONG_PASSWORD),
                Err(e) => return self.fail(&shown, e),
            }
            return match zip.by_index_decrypt(index, password.as_bytes()) {
                Ok(entry) => self.put(sink, item, None, &mut Decrypted(entry)),
                Err(e) => self.fail(&shown, e),
            };
        }
    }

    /// An encrypted entry of a skipped archive: counted as skipped, and as
    /// done for the progress bar.
    fn skip_locked(&mut self, shown: &Path, item: &Listed) -> Step {
        self.note(format_args!("skipped {}: no password", shown.display()));
        self.report.skipped += 1;
        if matches!(item.what, What::File) {
            self.progress.files_done += 1;
            self.progress.bytes_done += item.size;
        }
        Step::Incomplete
    }

    fn extract_tar(
        &mut self,
        dir: &SafeDir,
        archive: &Path,
        format: Format,
        pick: Option<&Pick>,
    ) -> Step {
        // First pass: names, kinds, sizes (smart extraction, progress). It
        // stops quietly at a read error; the second pass reports it.
        let listed = list_tar(archive, format);
        let mut sink = Sink::new(dir, archive, &listed, pick);
        self.count(&listed, &sink);
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
        // The place of each entry in `listed` (metadata entries skipped).
        let mut i = 0;
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
            i += 1;
            if !sink.takes(i - 1, &item) {
                continue;
            }
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
    pub(super) fn count(&mut self, listed: &[Listed], sink: &Sink) {
        for name in sink.missing(listed) {
            self.fail(&inside(sink.archive, &name), "no longer in the archive");
        }
        let files = listed
            .iter()
            .enumerate()
            .filter(|(i, item)| matches!(item.what, What::File) && sink.takes(*i, item))
            .map(|(_, item)| item);
        for item in files {
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
            Ok(parts) => sink
                .parts(parts)
                .expect("only entries the sink takes are put"),
            Err(why) => return self.fail(&shown, why),
        };
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
        let Some(existing) = existing.filter(|e| sink.written.contains(e)) else {
            let why = if sink.pick.is_some() {
                "a hard link to an entry not copied"
            } else {
                "a hard link to something outside this archive"
            };
            return self.fail(shown, why);
        };
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

/// A zip's entries from its central directory; `each` runs once per entry
/// and stops the listing (`Interrupted`) by returning false.
pub(crate) fn list_zip<R: Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    mut each: impl FnMut() -> bool,
) -> zip::result::ZipResult<Vec<Listed>> {
    let mut listed = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        if !each() {
            return Err(io::Error::from(io::ErrorKind::Interrupted).into());
        }
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
            owner: None,
        });
    }
    Ok(listed)
}

/// A tar's entries for browsing: the first read error is the result.
/// `each` runs once per entry and stops the listing by returning false.
pub(crate) fn read_tar_list(
    path: &Path,
    format: Format,
    mut each: impl FnMut() -> bool,
) -> io::Result<Vec<Listed>> {
    let mut tar = tar::Archive::new(tar_reader(path, format)?);
    let mut listed = Vec::new();
    for entry in tar.entries()? {
        let entry = entry?;
        if is_tar_metadata(&entry) {
            continue;
        }
        if !each() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        listed.push(listed_tar(&entry));
    }
    Ok(listed)
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
        owner: match (header.username(), header.groupname()) {
            (Ok(Some(user)), Ok(Some(group))) if !user.is_empty() => {
                Some(format!("{user}:{group}"))
            }
            _ => None,
        },
    }
}
