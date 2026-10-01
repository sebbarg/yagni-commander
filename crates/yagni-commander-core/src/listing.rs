//! Reading a directory for a panel, off the UI thread. The core decides what
//! to read ([`LoadRequest`], from [`crate::Commander::take_requests`]); the
//! UI runs [`read_listing`] on a thread of its own and hands the result to
//! [`crate::Commander::finish_load`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::entry::{Entry, read_entries};

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
}

/// A directory's entries, unsorted.
#[derive(Debug)]
pub struct Listing {
    /// The folder actually read; differs from the request after a fallback.
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

/// Reads the requested directory, or with a fallback, the first readable
/// one of it, its parents and the fallback folder.
pub fn read_listing(request: &LoadRequest) -> io::Result<Listing> {
    let read = |path: &Path| {
        request.progress.store(0, Ordering::Relaxed);
        read_entries(path, &request.progress).map(|entries| Listing {
            path: path.to_path_buf(),
            entries,
        })
    };
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
        }
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
