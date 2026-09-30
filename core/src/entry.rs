use std::cmp::Ordering;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The synthetic ".." entry.
    Parent,
    Dir,
    File,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Raw name, used for navigation. Not necessarily valid UTF-8.
    pub name: OsString,
    /// Lossy UTF-8 name for display.
    pub label: String,
    pub kind: EntryKind,
    pub is_symlink: bool,
    /// File size in bytes. `None` for directories and unreadable entries.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn parent() -> Self {
        Self {
            name: OsString::from(".."),
            label: "..".into(),
            kind: EntryKind::Parent,
            is_symlink: false,
            size: None,
            modified: None,
        }
    }

    /// True for entries that Enter navigates into (including "..").
    pub fn is_navigable(&self) -> bool {
        matches!(self.kind, EntryKind::Parent | EntryKind::Dir)
    }
}

/// Reads `dir` and returns its entries sorted TC-style: ".." first, then
/// directories, then files, each group by case-insensitive name.
///
/// A ".." entry is included unless `dir` is a filesystem root.
/// Entries whose metadata cannot be read are still listed, as files without size.
pub fn read_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    if dir.parent().is_some() {
        entries.push(Entry::parent());
    }

    for dirent in fs::read_dir(dir)? {
        let Ok(dirent) = dirent else { continue };
        let name = dirent.file_name();
        let label = name.to_string_lossy().into_owned();
        let link_meta = dirent.metadata().ok();
        let is_symlink = link_meta
            .as_ref()
            .is_some_and(|m| m.file_type().is_symlink());
        // Follow symlinks so a link to a directory behaves like a directory.
        let meta = if is_symlink {
            fs::metadata(dirent.path()).ok()
        } else {
            link_meta
        };
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(Entry {
            name,
            label,
            kind: if is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            is_symlink,
            size: meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()),
            modified: meta.and_then(|m| m.modified().ok()),
        });
    }

    entries.sort_by(compare);
    Ok(entries)
}

fn rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Parent => 0,
        EntryKind::Dir => 1,
        EntryKind::File => 2,
    }
}

fn compare(a: &Entry, b: &Entry) -> Ordering {
    rank(a.kind)
        .cmp(&rank(b.kind))
        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

/// Human-readable size with binary units, e.g. `1.5 KiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn sorts_parent_then_dirs_then_files_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        File::create(tmp.path().join("b.txt")).unwrap();
        File::create(tmp.path().join("A.txt")).unwrap();
        fs::create_dir(tmp.path().join("zdir")).unwrap();
        fs::create_dir(tmp.path().join("Adir")).unwrap();

        let labels: Vec<_> = read_entries(tmp.path())
            .unwrap()
            .into_iter()
            .map(|e| e.label)
            .collect();
        assert_eq!(labels, ["..", "Adir", "zdir", "A.txt", "b.txt"]);
    }

    #[test]
    fn root_has_no_parent_entry() {
        let entries = read_entries(Path::new("/")).unwrap();
        assert!(entries.iter().all(|e| e.kind != EntryKind::Parent));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_dir_is_a_dir_and_broken_link_is_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("nope"), tmp.path().join("broken")).unwrap();

        let entries = read_entries(tmp.path()).unwrap();
        let find = |n: &str| entries.iter().find(|e| e.label == n).unwrap();
        assert_eq!(find("link").kind, EntryKind::Dir);
        assert!(find("link").is_symlink);
        assert_eq!(find("broken").kind, EntryKind::File);
        assert!(find("broken").is_symlink);
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
    }
}
