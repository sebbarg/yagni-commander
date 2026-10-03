//! Reading a directory for a panel, off the UI thread. The core decides what
//! to read ([`LoadRequest`], from [`crate::Commander::take_requests`]); the
//! UI runs [`read_listing`] on a thread of its own and hands the result to
//! [`crate::Commander::finish_load`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::archive::{ArchiveIndex, Stamp};
use crate::entry::{Entry, read_entries};
use crate::find::Results;

/// A directory read the UI must run.
#[derive(Debug, Clone)]
pub struct LoadRequest {
    /// Matches the panel's pending load; a result for any other id is stale.
    pub id: u64,
    pub path: PathBuf,
    /// Startup only: if `path` can't be read, try its parents, then this
    /// folder (home).
    pub fallback: Option<PathBuf>,
    /// Entries read so far, for the "Loading..." indicator.
    pub progress: Arc<AtomicUsize>,
    /// Set when the panel no longer wants this read (Escape, a newer
    /// read): an archive listing stops early.
    pub cancel: Arc<AtomicBool>,
    /// `path` is inside this archive (Enter on an archive, or a re-read
    /// of a panel inside one).
    pub archive: Option<ArchiveRead>,
    /// A search's results to re-check (a re-read of a results panel).
    pub results: Option<Arc<Results>>,
    /// A missing `path` (not an unreadable one) is replaced by its nearest
    /// existing parent: go to file, leaving search results.
    pub up_if_missing: bool,
}

/// An archive to list for a [`LoadRequest`].
#[derive(Debug, Clone)]
pub struct ArchiveRead {
    pub file: PathBuf,
    /// The index the panel already has: reused when the file's size and
    /// time are unchanged.
    pub known: Option<Arc<ArchiveIndex>>,
}

/// Same file, and the same index (not an equal one).
impl PartialEq for ArchiveRead {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file
            && match (&self.known, &other.known) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for ArchiveRead {}

/// A directory's entries, unsorted.
#[derive(Debug)]
pub struct Listing {
    /// The folder actually read; differs from the request after a fallback.
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    /// The archive's index, when `path` is inside one.
    pub archive: Option<Arc<ArchiveIndex>>,
    /// The search results shown instead of the folder's entries.
    pub results: Option<Arc<Results>>,
}

/// Reads the requested directory, or with a fallback, the first readable
/// one of it, its parents and the fallback folder.
pub fn read_listing(request: &LoadRequest) -> io::Result<Listing> {
    if let Some(results) = &request.results {
        return Ok(Arc::new(results.recheck()).listing());
    }
    if let Some(archive) = &request.archive {
        return read_archive(request, archive);
    }
    let read = |path: &Path| {
        request.progress.store(0, Ordering::Relaxed);
        read_entries(path, &request.progress).map(|entries| Listing {
            path: path.to_path_buf(),
            entries,
            archive: None,
            results: None,
        })
    };
    if request.up_if_missing {
        return read_nearest(&request.path, read);
    }
    let Some(home) = &request.fallback else {
        return read(&request.path);
    };
    let mut first_error = None;
    let candidates = request
        .path
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .chain([home.as_path()]);
    for path in candidates {
        match read(path) {
            Ok(listing) => return Ok(listing),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.expect("the fallback folder is always tried"))
}

/// `path`, or its nearest parent where `path` is missing. Any other error
/// (no permission) is reported, never skipped.
fn read_nearest(path: &Path, read: impl Fn(&Path) -> io::Result<Listing>) -> io::Result<Listing> {
    let mut first_error = None;
    for candidate in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match read(candidate) {
            Ok(listing) => return Ok(listing),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                first_error.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(first_error.unwrap_or_else(|| io::ErrorKind::NotFound.into()))
}

/// The archive's index (read again only if the file changed) and the
/// folder inside it that the request names, or its nearest existing parent.
fn read_archive(request: &LoadRequest, archive: &ArchiveRead) -> io::Result<Listing> {
    request.progress.store(0, Ordering::Relaxed);
    let stamp = Stamp::read(&archive.file)?;
    let index = match &archive.known {
        Some(known) if known.stamp() == stamp => known.clone(),
        _ => Arc::new(ArchiveIndex::read(
            &archive.file,
            &request.progress,
            &request.cancel,
        )?),
    };
    Ok(crate::archive::listing(index, &request.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn request(path: &Path, fallback: Option<&Path>) -> LoadRequest {
        LoadRequest {
            id: 1,
            path: path.to_path_buf(),
            fallback: fallback.map(Path::to_path_buf),
            progress: Arc::default(),
            cancel: Arc::default(),
            archive: None,
            results: None,
            up_if_missing: false,
        }
    }

    fn archive_request(
        target: &Path,
        file: &Path,
        known: Option<Arc<ArchiveIndex>>,
    ) -> LoadRequest {
        LoadRequest {
            archive: Some(ArchiveRead {
                file: file.to_path_buf(),
                known,
            }),
            ..request(target, None)
        }
    }

    #[test]
    fn an_archive_read_lists_the_folder_inside_or_its_nearest_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        crate::test_archives::make_zip(&zip, &[("s/t/x", "1")]);
        let req = archive_request(&zip.join("s/gone"), &zip, None);
        let listing = read_listing(&req).unwrap();
        assert_eq!(listing.path, zip.join("s"));
        let labels: Vec<_> = listing.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["..", "t"]);
        assert!(listing.archive.is_some());
        assert_eq!(req.progress.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn an_unchanged_archive_keeps_its_index() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        crate::test_archives::make_zip(&zip, &[("x", "1")]);
        let first = read_listing(&archive_request(&zip, &zip, None))
            .unwrap()
            .archive
            .unwrap();
        let again = read_listing(&archive_request(&zip, &zip, Some(first.clone()))).unwrap();
        assert!(
            Arc::ptr_eq(&first, again.archive.as_ref().unwrap()),
            "not re-read"
        );
        crate::test_archives::make_zip(&zip, &[("x", "1"), ("y", "22")]);
        let changed = read_listing(&archive_request(&zip, &zip, Some(first.clone()))).unwrap();
        assert!(!Arc::ptr_eq(&first, changed.archive.as_ref().unwrap()));
        assert_eq!(changed.entries.len(), 3);
    }

    #[test]
    fn a_cancelled_archive_read_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        crate::test_archives::make_zip(&zip, &[("x", "1"), ("y", "2")]);
        let req = archive_request(&zip, &zip, None);
        req.cancel.store(true, Ordering::Relaxed);
        let err = read_listing(&req).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        let tar = tmp.path().join("a.tar.gz");
        crate::test_archives::make_tar(
            &tar,
            crate::file_ops::Format::TarGz,
            &[crate::test_archives::T::File("x", "1", 0o644)],
        );
        let req = archive_request(&tar, &tar, None);
        req.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            read_listing(&req).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn a_vanished_archive_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        let err = read_listing(&archive_request(&zip, &zip, None)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn archive_reads_compare_by_file_and_index() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        crate::test_archives::make_zip(&zip, &[("x", "1")]);
        let index = read_listing(&archive_request(&zip, &zip, None))
            .unwrap()
            .archive
            .unwrap();
        let other = read_listing(&archive_request(&zip, &zip, None))
            .unwrap()
            .archive
            .unwrap();
        let read = |known: Option<&Arc<ArchiveIndex>>| ArchiveRead {
            file: zip.clone(),
            known: known.cloned(),
        };
        assert_eq!(read(Some(&index)), read(Some(&index)));
        assert_ne!(read(Some(&index)), read(Some(&other)), "another read");
        assert_ne!(read(Some(&index)), read(None));
        assert_eq!(read(None), read(None));
        let elsewhere = ArchiveRead {
            file: tmp.path().join("b.zip"),
            known: None,
        };
        assert_ne!(read(None), elsewhere);
    }

    #[test]
    fn reads_a_directory_and_counts_its_entries() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c"] {
            fs::write(tmp.path().join(name), b"").unwrap();
        }
        let req = request(tmp.path(), None);
        let listing = read_listing(&req).unwrap();
        assert_eq!(listing.path, tmp.path());
        assert_eq!(listing.entries.len(), 4, "a, b, c and ..");
        assert!(listing.archive.is_none());
        assert_eq!(req.progress.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn without_fallback_a_missing_directory_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_listing(&request(&tmp.path().join("gone"), None)).is_err());
    }

    #[test]
    fn fallback_walks_up_from_a_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("a")).unwrap();
        let req = request(&tmp.path().join("a/b/c"), Some(Path::new("/")));
        assert_eq!(read_listing(&req).unwrap().path, tmp.path().join("a"));
    }

    #[test]
    fn fallback_skips_files_and_unreadable_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        fs::write(&file, b"").unwrap();
        let req = request(&file, Some(Path::new("/")));
        assert_eq!(read_listing(&req).unwrap().path, tmp.path());

        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok(); // root reads anything
        let found = read_listing(&request(&locked, Some(Path::new("/"))))
            .unwrap()
            .path;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        if !readable {
            assert_eq!(found, tmp.path());
        }
    }

    #[test]
    fn fallback_ends_at_home() {
        let home = tempfile::tempdir().unwrap();
        let req = request(Path::new("relative/missing"), Some(home.path()));
        assert_eq!(read_listing(&req).unwrap().path, home.path());
    }

    #[test]
    fn fallback_reports_the_first_error_when_nothing_is_readable() {
        let req = request(Path::new("missing"), Some(Path::new("also-missing")));
        let err = read_listing(&req).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
