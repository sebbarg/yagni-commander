//! Browsing into an archive: its entries as a tree of folders, read once
//! when the archive is opened (Enter). Panels inside an archive show
//! [`ArchiveIndex::entries`]; nothing in here writes.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::SystemTime;

use crate::entry::{Entry, EntryKind};
use crate::file_ops::Format;
use crate::file_ops::extract::{Listed, What, list_zip, read_tar_list};
use crate::listing::Listing;

/// Why a writing command is refused inside an archive.
pub const IN_ARCHIVE: &str = "Not available inside an archive.";

/// File type bits, as `format_permissions` reads them.
const DIR: u32 = 0o040000;
const FILE: u32 = 0o100000;
const LINK: u32 = 0o120000;

/// An archive file's size and time: unchanged means its index still fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    pub(crate) fn read(file: &Path) -> io::Result<Self> {
        let meta = fs::metadata(file)?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not an archive file",
            ));
        }
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// An archive's entries by folder.
#[derive(Debug)]
pub struct ArchiveIndex {
    file: PathBuf,
    stamp: Stamp,
    /// Every folder (name parts from the root; the root is empty) and its
    /// entries, without "..".
    folders: HashMap<Vec<OsString>, Vec<Entry>>,
}

/// Folders while the index is built: entries by name, so a later entry
/// replaces an earlier one of the same name (as extracting would).
type Building = HashMap<Vec<OsString>, HashMap<OsString, Entry>>;

impl ArchiveIndex {
    pub fn file(&self) -> &Path {
        &self.file
    }

    pub(crate) fn stamp(&self) -> Stamp {
        self.stamp
    }

    /// Lists `file` (any [`Format`]); `progress` counts the entries read.
    /// Setting `cancel` stops it (`Interrupted`): Escape, or a newer read.
    pub(crate) fn read(
        file: &Path,
        progress: &AtomicUsize,
        cancel: &AtomicBool,
    ) -> io::Result<Self> {
        let stamp = Stamp::read(file)?;
        let format = file
            .file_name()
            .and_then(Format::from_name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an archive"))?;
        let count = || {
            progress.fetch_add(1, Ordering::Relaxed);
            !cancel.load(Ordering::Relaxed)
        };
        let listed = match format {
            Format::Zip => {
                let mut zip = zip::ZipArchive::new(BufReader::new(File::open(file)?))
                    .map_err(io::Error::other)?;
                list_zip(&mut zip, count).map_err(io::Error::from)?
            }
            format => read_tar_list(file, format, count)?,
        };
        Ok(Self::build(file.to_path_buf(), stamp, listed))
    }

    fn build(file: PathBuf, stamp: Stamp, listed: Vec<Listed>) -> Self {
        let mut folders = Building::new();
        folders.insert(Vec::new(), HashMap::new());
        let mut owners: HashMap<String, Arc<str>> = HashMap::new();
        for item in listed {
            // Unsafe names (absolute, `..`) can't be copied out anyway.
            let Ok(parts) = item.parts else { continue };
            let (name, parent) = parts
                .split_last()
                .expect("`components` never gives an empty name");
            add_folders(&mut folders, parent);
            let owner = item
                .owner
                .map(|o| owners.entry(o.clone()).or_insert_with(|| o.into()).clone());
            let (kind, is_symlink, type_bits) = match item.what {
                What::Dir => (EntryKind::Dir, false, DIR),
                What::Symlink => (EntryKind::File, true, LINK),
                What::File | What::HardLink(_) | What::Special => (EntryKind::File, false, FILE),
            };
            if kind == EntryKind::Dir {
                folders.entry(parts.clone()).or_default();
            }
            let size = (kind == EntryKind::File).then_some(item.size);
            let default_mode = if kind == EntryKind::Dir { 0o755 } else { 0o644 };
            let mode = Some(type_bits | item.mode.unwrap_or(default_mode));
            let entry = Entry::archived(
                name.clone(),
                kind,
                is_symlink,
                size,
                item.modified,
                mode,
                owner,
            );
            folders
                .get_mut(parent)
                .expect("added above")
                .insert(name.clone(), entry);
        }
        let folders = folders
            .into_iter()
            .map(|(path, entries)| (path, entries.into_values().collect()))
            .collect();
        Self {
            file,
            stamp,
            folders,
        }
    }

    /// `inner` if it is a folder of this archive, else its nearest parent
    /// that is.
    pub(crate) fn nearest(&self, inner: &[OsString]) -> Vec<OsString> {
        (0..=inner.len())
            .rev()
            .map(|n| &inner[..n])
            .find(|folder| self.folders.contains_key(*folder))
            .unwrap_or(&[])
            .to_vec()
    }

    /// The entry at `path` inside the archive (Properties), if any.
    pub fn entry(&self, path: &[OsString]) -> Option<Entry> {
        let (name, folder) = path.split_last()?;
        self.folders
            .get(folder)?
            .iter()
            .find(|e| &e.name == name)
            .cloned()
    }

    /// What folder `inner` holds (Properties): instant, from the index.
    pub fn totals(&self, inner: &[OsString]) -> crate::info::Totals {
        let mut totals = crate::info::Totals::default();
        let mut folders = vec![inner.to_vec()];
        while let Some(folder) = folders.pop() {
            for entry in self.folders.get(&folder).into_iter().flatten() {
                if entry.kind == EntryKind::Dir {
                    totals.folders += 1;
                    let mut sub = folder.clone();
                    sub.push(entry.name.clone());
                    folders.push(sub);
                } else {
                    totals.files += 1;
                    totals.bytes += entry.size.unwrap_or(0);
                }
            }
        }
        totals
    }

    /// The entries of folder `inner`, with "..", or `None` if there is no
    /// such folder.
    pub(crate) fn entries(&self, inner: &[OsString]) -> Option<Vec<Entry>> {
        let entries = self.folders.get(inner)?;
        Some(
            std::iter::once(Entry::parent())
                .chain(entries.iter().cloned())
                .collect(),
        )
    }
}

/// Adds every folder on the way to `parent` that is not one yet (a zip may
/// hold `a/b/c` without `a/` or `a/b/`).
fn add_folders(folders: &mut Building, parent: &[OsString]) {
    for depth in 1..=parent.len() {
        if folders.contains_key(&parent[..depth]) {
            continue;
        }
        folders.insert(parent[..depth].to_vec(), HashMap::new());
        let name = parent[depth - 1].clone();
        let implied = Entry::archived(
            name.clone(),
            EntryKind::Dir,
            false,
            None,
            None,
            Some(DIR | 0o755),
            None,
        );
        // A file of this name listed earlier gives way: later entries win.
        folders
            .get_mut(&parent[..depth - 1])
            .expect("the root, or added on the previous turn")
            .insert(name, implied);
    }
}

/// The folder inside `file` that `target` names (`/x/a.zip/s/t` gives
/// `s`, `t`); empty for the archive itself or a path outside it.
pub fn inner_parts(file: &Path, target: &Path) -> Vec<OsString> {
    target
        .strip_prefix(file)
        .map(|rest| {
            rest.components()
                .map(|c| c.as_os_str().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// What a panel shows for `target` inside `index`'s archive: that folder,
/// or its nearest parent the archive has.
pub(crate) fn listing(index: Arc<ArchiveIndex>, target: &Path) -> Listing {
    let inner = index.nearest(&inner_parts(index.file(), target));
    let entries = index.entries(&inner).expect("nearest is a folder");
    let path = inner
        .iter()
        .fold(index.file().to_path_buf(), |path, part| path.join(part));
    Listing {
        path,
        entries,
        archive: Some(index),
        results: None,
        modified: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ops::Format;
    use crate::test_archives::{T, make_tar, make_zip};

    fn parts(names: &[&str]) -> Vec<OsString> {
        names.iter().map(OsString::from).collect()
    }

    fn names(index: &ArchiveIndex, inner: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = index
            .entries(&parts(inner))
            .unwrap()
            .into_iter()
            .map(|e| e.label)
            .collect();
        names.sort();
        names
    }

    fn read(path: &Path) -> ArchiveIndex {
        ArchiveIndex::read(path, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap()
    }

    fn find(index: &ArchiveIndex, inner: &[&str], label: &str) -> Entry {
        index
            .entries(&parts(inner))
            .unwrap()
            .into_iter()
            .find(|e| e.label == label)
            .unwrap()
    }

    #[test]
    fn a_zip_without_folder_entries_gets_implied_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        make_zip(
            &zip,
            &[("src/lib/a.rs", "a"), ("src/main.rs", "m"), ("README", "r")],
        );
        let index = read(&zip);
        assert_eq!(index.file(), zip);
        assert_eq!(names(&index, &[]), ["..", "README", "src"]);
        assert_eq!(names(&index, &["src"]), ["..", "lib", "main.rs"]);
        assert_eq!(names(&index, &["src", "lib"]), ["..", "a.rs"]);
        let src = find(&index, &[], "src");
        assert_eq!((src.kind, src.size), (EntryKind::Dir, None));
        assert_eq!(src.mode, Some(0o040755));
        let readme = find(&index, &[], "README");
        assert_eq!((readme.kind, readme.size), (EntryKind::File, Some(1)));
        assert!(readme.owner.is_none());
    }

    #[test]
    fn unsafe_names_are_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        let tar = tmp.path().join("a.tar");
        make_tar(
            &tar,
            Format::Tar,
            &[
                T::Raw(b"../evil"),
                T::Raw(b"/abs"),
                T::Raw(b"./"),
                T::File("ok", "", 0o644),
            ],
        );
        assert_eq!(names(&read(&tar), &[]), ["..", "ok"]);
    }

    #[test]
    fn every_tar_compression_lists_with_modes_links_and_owners() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, format) in [
            ("a.tar", Format::Tar),
            ("a.tar.gz", Format::TarGz),
            ("a.tar.bz2", Format::TarBz2),
            ("a.tar.xz", Format::TarXz),
            ("a.tar.zst", Format::TarZst),
        ] {
            let path = tmp.path().join(name);
            make_tar(
                &path,
                format,
                &[
                    T::Dir("d/"),
                    T::File("d/x", "xy", 0o750),
                    T::Link("d/l", "x"),
                    T::Hard("d/h", b"d/x"),
                    T::Fifo("d/p"),
                ],
            );
            let index = read(&path);
            let x = find(&index, &["d"], "x");
            assert_eq!(x.mode, Some(0o100750), "{name}");
            assert_eq!(x.owner.as_deref(), Some("me:staff"));
            let l = find(&index, &["d"], "l");
            assert!(l.is_symlink && l.kind == EntryKind::File, "{name}");
            assert_eq!(l.mode.map(|m| m & 0o170000), Some(0o120000));
            for other in ["h", "p"] {
                let entry = find(&index, &["d"], other);
                assert!(entry.kind == EntryKind::File && !entry.is_symlink);
            }
            assert_eq!(find(&index, &[], "d").mode, Some(0o040755));
        }
    }

    #[test]
    fn a_later_duplicate_wins_and_a_real_folder_entry_replaces_an_implied_one() {
        let tmp = tempfile::tempdir().unwrap();
        let tar = tmp.path().join("a.tar");
        make_tar(
            &tar,
            Format::Tar,
            &[
                T::File("d/x", "1", 0o644),
                T::Dir("d/"),
                T::File("d/x", "22", 0o644),
            ],
        );
        let index = read(&tar);
        assert_eq!(names(&index, &["d"]), ["..", "x"]);
        assert_eq!(find(&index, &["d"], "x").size, Some(2));
        assert!(find(&index, &[], "d").modified.is_some(), "the real entry");
    }

    #[test]
    fn a_later_entry_inside_a_file_makes_it_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let tar = tmp.path().join("a.tar");
        make_tar(
            &tar,
            Format::Tar,
            &[T::File("a", "1", 0o644), T::File("a/b", "2", 0o644)],
        );
        let index = read(&tar);
        assert_eq!(find(&index, &[], "a").kind, EntryKind::Dir);
        assert_eq!(names(&index, &["a"]), ["..", "b"]);
    }

    #[test]
    fn totals_walk_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        make_zip(&zip, &[("s/a", "12"), ("s/t/b", "345"), ("c", "6")]);
        let index = read(&zip);
        let all = index.totals(&[]);
        assert_eq!((all.files, all.folders, all.bytes), (3, 2, 6));
        let s = index.totals(&parts(&["s"]));
        assert_eq!((s.files, s.folders, s.bytes), (2, 1, 5));
        assert_eq!(
            index.totals(&parts(&["nope"])),
            crate::info::Totals::default()
        );
    }

    #[test]
    fn nearest_walks_up_to_an_existing_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        make_zip(&zip, &[("a/b/c", "")]);
        let index = read(&zip);
        assert_eq!(index.nearest(&parts(&["a", "gone", "x"])), parts(&["a"]));
        assert_eq!(index.nearest(&parts(&["a", "b"])), parts(&["a", "b"]));
        assert_eq!(index.nearest(&parts(&["a", "b", "c"])), parts(&["a", "b"]));
        assert!(index.nearest(&parts(&["nope"])).is_empty());
        assert!(index.entries(&parts(&["nope"])).is_none());
    }

    #[test]
    fn damaged_and_unknown_files_fail() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, data) in [("bad.zip", &b"PK nonsense"[..]), ("bad.tar.xz", b"nope")] {
            let path = tmp.path().join(name);
            std::fs::write(&path, data).unwrap();
            assert!(
                ArchiveIndex::read(&path, &AtomicUsize::new(0), &AtomicBool::new(false)).is_err(),
                "{name}"
            );
        }
        let plain = tmp.path().join("plain.txt");
        std::fs::write(&plain, b"").unwrap();
        let err =
            ArchiveIndex::read(&plain, &AtomicUsize::new(0), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let err = ArchiveIndex::read(
            &tmp.path().join("gone.zip"),
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        );
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::NotFound);
        let folder = tmp.path().join("folder.zip");
        std::fs::create_dir(&folder).unwrap();
        assert!(Stamp::read(&folder).is_err(), "a folder is not an archive");
    }

    #[test]
    fn progress_counts_entries_and_the_stamp_follows_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        make_zip(&zip, &[("x", "1"), ("y", "2")]);
        let progress = AtomicUsize::new(0);
        let index = ArchiveIndex::read(&zip, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 2);
        assert_eq!(index.stamp(), Stamp::read(&zip).unwrap());
        make_zip(&zip, &[("x", "1"), ("y", "2"), ("z", "3")]);
        assert_ne!(index.stamp(), Stamp::read(&zip).unwrap(), "size changed");
    }

    #[test]
    fn inner_parts_are_the_path_below_the_archive() {
        let file = Path::new("/x/a.zip");
        assert_eq!(
            inner_parts(file, Path::new("/x/a.zip/s/t")),
            parts(&["s", "t"])
        );
        assert!(inner_parts(file, file).is_empty());
        assert!(inner_parts(file, Path::new("/elsewhere")).is_empty());
    }
}
