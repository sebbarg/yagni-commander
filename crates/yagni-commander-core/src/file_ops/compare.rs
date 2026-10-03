//! Compare by content: two files byte for byte, or two folder trees by
//! names, types and contents. Reads only; nothing is logged.
//!
//! A folder comparison walks both trees first (without following symlinks),
//! recording what differs by name or type and queueing file pairs of the
//! same size; then it reads the queued pairs, with progress in files and
//! bytes.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, FileType};
use std::io::{self, Read};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use super::{Observer, Progress};

/// Bytes read from each file between progress reports and cancel checks.
const CHUNK: usize = 1 << 20;

/// What a comparison found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Regular-file pairs found under the same name (1 for two files).
    pub files: usize,
    /// Sorted by path.
    pub differences: Vec<Difference>,
}

impl Comparison {
    pub fn identical(&self) -> bool {
        self.differences.is_empty()
    }
}

/// One difference. Paths are relative to the two compared folders (empty
/// for the two picked entries themselves), except for [`Difference::Unreadable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Only in the first folder; `is_dir` for a folder (listed without its contents).
    OnlyFirst { path: PathBuf, is_dir: bool },
    /// Only in the second folder.
    OnlySecond { path: PathBuf, is_dir: bool },
    /// Same name, different contents (or symlinks with different targets).
    Content(PathBuf),
    /// Same name, different types (a file and a folder, ...).
    Type(PathBuf),
    /// Could not be read; `path` is the full path.
    Unreadable { path: PathBuf, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Link,
    Fifo,
    Socket,
    Block,
    Char,
}

fn kind(file_type: FileType) -> Kind {
    if file_type.is_dir() {
        Kind::Dir
    } else if file_type.is_symlink() {
        Kind::Link
    } else if file_type.is_fifo() {
        Kind::Fifo
    } else if file_type.is_socket() {
        Kind::Socket
    } else if file_type.is_block_device() {
        Kind::Block
    } else if file_type.is_char_device() {
        Kind::Char
    } else {
        Kind::File
    }
}

/// A folder entry: its type and size, or `None` when it could not be read
/// (already recorded as unreadable).
type Listing = BTreeMap<OsString, Option<(Kind, u64)>>;

/// Compares `first` with `second`. A picked symlink is followed. Returns
/// the result and whether the user cancelled.
pub(super) fn run(first: &Path, second: &Path, observer: &mut dyn Observer) -> (Comparison, bool) {
    let mut walk = Walk {
        observer,
        first,
        second,
        progress: Progress::default(),
        comparison: Comparison::default(),
        pairs: Vec::new(),
    };
    let cancelled = !walk.top() || !walk.contents();
    let mut comparison = walk.comparison;
    comparison
        .differences
        .sort_by(|a, b| sort_key(a, first, second).cmp(sort_key(b, first, second)));
    (comparison, cancelled)
}

fn sort_key<'a>(difference: &'a Difference, first: &Path, second: &Path) -> &'a Path {
    match difference {
        Difference::OnlyFirst { path, .. }
        | Difference::OnlySecond { path, .. }
        | Difference::Content(path)
        | Difference::Type(path) => path,
        Difference::Unreadable { path, .. } => path
            .strip_prefix(first)
            .or_else(|_| path.strip_prefix(second))
            .unwrap_or(path),
    }
}

struct Walk<'a> {
    observer: &'a mut dyn Observer,
    first: &'a Path,
    second: &'a Path,
    progress: Progress,
    comparison: Comparison,
    /// Same-size file pairs to read, by relative path, with their size.
    pairs: Vec<(PathBuf, u64)>,
}

impl Walk<'_> {
    /// The two picked entries. Returns false if cancelled.
    fn top(&mut self) -> bool {
        let stat = |path: &Path| fs::metadata(path).map(|m| (kind(m.file_type()), m.len()));
        let (a, b) = match (stat(self.first), stat(self.second)) {
            (Ok(a), Ok(b)) => (a, b),
            (a, b) => {
                for (path, result) in [(self.first, a), (self.second, b)] {
                    if let Err(e) = result {
                        self.unreadable(path, &e);
                    }
                }
                return true;
            }
        };
        match (a, b) {
            ((Kind::Dir, _), (Kind::Dir, _)) => self.dir(Path::new("")),
            (a, b) => {
                self.pair(PathBuf::new(), a, b);
                true
            }
        }
    }

    /// Compares the folder `rel` on both sides. Returns false if cancelled.
    fn dir(&mut self, rel: &Path) -> bool {
        if self.observer.is_cancelled() {
            return false;
        }
        let (first, second) = (at(self.first, rel), at(self.second, rel));
        self.progress.current = first.clone();
        self.observer.progress(&self.progress);
        let (a, b) = match (self.list(&first), self.list(&second)) {
            (Some(a), Some(b)) => (a, b),
            _ => return true,
        };
        let mut names: Vec<&OsString> = a.keys().chain(b.keys()).collect();
        names.sort();
        names.dedup();
        for name in names {
            let path = rel.join(name);
            match (a.get(name), b.get(name)) {
                (Some(Some((kind, _))), None) => {
                    let is_dir = *kind == Kind::Dir;
                    self.differ(Difference::OnlyFirst { path, is_dir });
                }
                (None, Some(Some((kind, _)))) => {
                    let is_dir = *kind == Kind::Dir;
                    self.differ(Difference::OnlySecond { path, is_dir });
                }
                (Some(Some(x)), Some(Some(y))) if x.0 == Kind::Dir && y.0 == Kind::Dir => {
                    if !self.dir(&path) {
                        return false;
                    }
                }
                (Some(Some(x)), Some(Some(y))) => self.pair(path, *x, *y),
                // Unreadable on a side: already recorded.
                _ => {}
            }
        }
        true
    }

    /// Two entries of the same name that are not both folders.
    fn pair(&mut self, rel: PathBuf, (a, a_len): (Kind, u64), (b, b_len): (Kind, u64)) {
        if a != b {
            self.differ(Difference::Type(rel));
            return;
        }
        match a {
            Kind::File => {
                self.comparison.files += 1;
                if a_len != b_len {
                    self.differ(Difference::Content(rel));
                } else {
                    self.pairs.push((rel, a_len));
                }
            }
            Kind::Link => {
                let first = fs::read_link(at(self.first, &rel));
                let second = fs::read_link(at(self.second, &rel));
                match (first, second) {
                    (Ok(x), Ok(y)) if x == y => {}
                    (Ok(_), Ok(_)) => self.differ(Difference::Content(rel)),
                    (Err(e), _) => self.unreadable(&at(self.first, &rel), &e),
                    (_, Err(e)) => self.unreadable(&at(self.second, &rel), &e),
                }
            }
            // FIFOs, sockets and devices: by type only, never opened.
            _ => {}
        }
    }

    /// A folder's entries by name, or `None` (recorded) if it can't be read.
    fn list(&mut self, dir: &Path) -> Option<Listing> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                self.unreadable(dir, &e);
                return None;
            }
        };
        let mut listing = Listing::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.unreadable(dir, &e);
                    return None;
                }
            };
            // DirEntry::metadata doesn't follow symlinks.
            let info = match entry.metadata() {
                Ok(meta) => Some((kind(meta.file_type()), meta.len())),
                Err(e) => {
                    self.unreadable(&entry.path(), &e);
                    None
                }
            };
            listing.insert(entry.file_name(), info);
        }
        Some(listing)
    }

    /// Reads the queued pairs. Returns false if cancelled.
    fn contents(&mut self) -> bool {
        let pairs = std::mem::take(&mut self.pairs);
        self.progress.files_total = self.comparison.files;
        self.progress.files_done = self.comparison.files - pairs.len();
        self.progress.bytes_total = pairs.iter().map(|(_, len)| len).sum();
        let mut buffers = (vec![0; CHUNK], vec![0; CHUNK]);
        for (rel, _) in pairs {
            if self.observer.is_cancelled() {
                return false;
            }
            let (first, second) = (at(self.first, &rel), at(self.second, &rel));
            self.progress.current = first.clone();
            self.observer.progress(&self.progress);
            match self.same(&first, &second, &mut buffers) {
                Ok(Some(true)) => {}
                Ok(Some(false)) => self.differ(Difference::Content(rel)),
                Ok(None) => return false,
                Err((path, e)) => self.unreadable(&path, &e),
            }
            self.progress.files_done += 1;
        }
        self.observer.progress(&self.progress);
        true
    }

    /// Whether two files hold the same bytes; `None` if cancelled. An error
    /// names the file it came from.
    fn same(
        &mut self,
        first: &Path,
        second: &Path,
        (a, b): &mut (Vec<u8>, Vec<u8>),
    ) -> Result<Option<bool>, (PathBuf, io::Error)> {
        let open = |path: &Path| File::open(path).map_err(|e| (path.to_path_buf(), e));
        let (mut x, mut y) = (open(first)?, open(second)?);
        loop {
            let n = fill(&mut x, a).map_err(|e| (first.to_path_buf(), e))?;
            let m = fill(&mut y, b).map_err(|e| (second.to_path_buf(), e))?;
            if a[..n] != b[..m] {
                return Ok(Some(false));
            }
            if n == 0 {
                return Ok(Some(true));
            }
            self.progress.bytes_done += n as u64;
            self.observer.progress(&self.progress);
            if self.observer.is_cancelled() {
                return Ok(None);
            }
        }
    }

    fn differ(&mut self, difference: Difference) {
        self.comparison.differences.push(difference);
    }

    fn unreadable(&mut self, path: &Path, error: &io::Error) {
        self.differ(Difference::Unreadable {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
}

/// `rel` under `root`; `root` itself for an empty `rel` (`join` would add a
/// trailing slash, which fails on a file).
fn at(root: &Path, rel: &Path) -> PathBuf {
    if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

/// Reads until `buf` is full or the file ends; returns the bytes read.
fn fill(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests;
