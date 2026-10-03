//! Alt-Enter: an entry's details (`read`, `from_entry`), a folder's
//! contents (`count`), and the text the Properties box shows (`lines`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use crate::entry::{Entry, EntryKind};
use crate::format::{format_count, format_size, format_time, permissions_text};

/// What an entry is. A symlink is described by its target's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
    /// "pipe", "socket" or "device".
    Other(&'static str),
}

/// A symlink's target as stored, and whether it points nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Empty inside an archive (not read).
    pub target: PathBuf,
    pub missing: bool,
}

/// An entry's details. Size and times are a symlink's target's, mode and
/// owner the link's own, as in the panel.
#[derive(Debug, Clone)]
pub struct Info {
    pub kind: Kind,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub link: Option<Link>,
}

/// A folder's contents, below the folder itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub folders: u64,
    pub bytes: u64,
    /// Folders that could not be read (or not to their end).
    pub unreadable: u64,
    /// Mount points inside, not counted (like `du -x`): another
    /// filesystem, or `/proc` and `/sys` under `/`.
    pub other_fs: u64,
}

/// A running count's files and folders so far, shared with the UI.
#[derive(Debug, Default)]
pub struct Progress {
    files: AtomicU64,
    folders: AtomicU64,
}

impl Progress {
    pub fn so_far(&self) -> Totals {
        Totals {
            files: self.files.load(Ordering::Relaxed),
            folders: self.folders.load(Ordering::Relaxed),
            ..Totals::default()
        }
    }
}

/// Where a folder's count stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    NotAFolder,
    /// Files and folders counted so far.
    Counting(Totals),
    Done(Totals),
}

/// Reads `path`'s details without following it, then its target's.
pub fn read(path: &Path) -> io::Result<Info> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let own = fs::symlink_metadata(path)?;
    let target = if own.file_type().is_symlink() {
        fs::metadata(path).ok()
    } else {
        Some(own.clone())
    };
    let link = own.file_type().is_symlink().then(|| Link {
        target: fs::read_link(path).unwrap_or_default(),
        missing: target.is_none(),
    });
    let kind = match &target {
        Some(m) if m.is_dir() => Kind::Folder,
        Some(m) if m.file_type().is_fifo() => Kind::Other("pipe"),
        Some(m) if m.file_type().is_socket() => Kind::Other("socket"),
        Some(m) if m.file_type().is_block_device() || m.file_type().is_char_device() => {
            Kind::Other("device")
        }
        // A file, or a dangling link (its Type line says so).
        _ => Kind::File,
    };
    Ok(Info {
        kind,
        size: target.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
        modified: target.as_ref().and_then(|m| m.modified().ok()),
        accessed: target.as_ref().and_then(|m| m.accessed().ok()),
        created: target.as_ref().and_then(|m| m.created().ok()),
        mode: Some(own.mode()),
        owner: crate::entry::owner_text(&own),
        link,
    })
}

/// Counts what folder `path` holds, never following symlinks (a link
/// counts as a file of its own size) and staying on `path`'s filesystem.
/// A hard-linked file's bytes count once. `progress` shows the files and
/// folders so far; setting `cancel` stops the walk (`None`).
pub fn count(path: &Path, progress: &Progress, cancel: &AtomicBool) -> Option<Totals> {
    let device = |_: &Path, meta: &fs::Metadata| std::os::unix::fs::MetadataExt::dev(meta);
    walk(path, progress, |_| !cancel.load(Ordering::Relaxed), &device)
}

/// [`count`]'s walk: `keep_going` is asked before each entry; `device`
/// gives a folder's filesystem (tests fake a mount point). Iterative, so
/// depth is no limit.
fn walk(
    path: &Path,
    progress: &Progress,
    mut keep_going: impl FnMut(&Totals) -> bool,
    device: &dyn Fn(&Path, &fs::Metadata) -> u64,
) -> Option<Totals> {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;
    let mut totals = Totals::default();
    let Ok(start) = fs::metadata(path) else {
        totals.unreadable += 1;
        return Some(totals);
    };
    let home = device(path, &start);
    let mut linked = HashSet::new();
    let mut folders = vec![path.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            totals.unreadable += 1;
            continue;
        };
        let mut broken = false;
        for entry in entries {
            if !keep_going(&totals) {
                return None;
            }
            let Ok(entry) = entry else {
                // The folder could not be read to its end.
                if !broken {
                    totals.unreadable += 1;
                    broken = true;
                }
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue; // vanished meanwhile
            };
            let path = entry.path();
            if meta.is_dir() {
                if device(&path, &meta) != home {
                    totals.other_fs += 1;
                    continue;
                }
                totals.folders += 1;
                progress.folders.fetch_add(1, Ordering::Relaxed);
                folders.push(path);
                continue;
            }
            totals.files += 1;
            progress.files.fetch_add(1, Ordering::Relaxed);
            let once = meta.nlink() < 2 || linked.insert((meta.dev(), meta.ino()));
            if once && (meta.is_file() || meta.file_type().is_symlink()) {
                totals.bytes += meta.len();
            }
        }
    }
    Some(totals)
}

/// An entry inside an archive, from its index: no access or creation time.
pub fn from_entry(entry: &Entry) -> Info {
    Info {
        kind: match entry.kind {
            EntryKind::File => Kind::File,
            _ => Kind::Folder,
        },
        size: entry.size,
        modified: entry.modified,
        accessed: None,
        created: None,
        mode: entry.mode,
        owner: entry.owner.as_deref().map(str::to_owned),
        link: entry.is_symlink.then(|| Link {
            target: PathBuf::new(),
            missing: false,
        }),
    }
}

/// `1 file`, `2 files`, `1,000 files`.
fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", format_count(n), if n == 1 { one } else { many })
}

/// The Properties box's lines, as (label, value).
pub fn lines(
    name: &str,
    folder: &str,
    info: &io::Result<Info>,
    count: &Count,
) -> Vec<(&'static str, String)> {
    let mut lines = vec![("Name", name.to_owned())];
    // Nothing contains a filesystem root.
    if !folder.is_empty() {
        lines.push(("Folder", folder.to_owned()));
    }
    let info = match info {
        Ok(info) => info,
        Err(e) => {
            lines.push(("Type", format!("Cannot read: {e}")));
            return lines;
        }
    };
    let kind = match info.kind {
        Kind::File => "File".to_owned(),
        Kind::Folder => "Folder".to_owned(),
        Kind::Other(what) => format!("Other ({what})"),
    };
    let kind = match &info.link {
        None => kind,
        Some(link) if link.target.as_os_str().is_empty() => "Symbolic link".to_owned(),
        Some(link) => {
            let missing = if link.missing { " (missing)" } else { "" };
            format!("Symbolic link to {}{missing}", link.target.display())
        }
    };
    lines.push(("Type", kind));
    let bytes = |n: u64| format!("{} ({})", counted(n, "byte", "bytes"), format_size(n));
    match count {
        Count::NotAFolder => {
            if let Some(size) = info.size {
                lines.push(("Size", bytes(size)));
            }
        }
        Count::Counting(so_far) => {
            lines.push(("Size", "Counting...".to_owned()));
            let files = counted(so_far.files, "file", "files");
            let folders = counted(so_far.folders, "folder", "folders");
            lines.push(("Contains", format!("Counting... {files}, {folders}")));
        }
        Count::Done(totals) => {
            lines.push(("Size", bytes(totals.bytes)));
            let mut contains = format!(
                "{}, {}",
                counted(totals.files, "file", "files"),
                counted(totals.folders, "folder", "folders")
            );
            if totals.unreadable > 0 {
                contains += &format!(", {} unreadable", format_count(totals.unreadable));
            }
            if totals.other_fs > 0 {
                let other = counted(totals.other_fs, "other filesystem", "other filesystems");
                contains += &format!(", {other} not counted");
            }
            lines.push(("Contains", contains));
        }
    }
    for (label, time) in [
        ("Modified", info.modified),
        ("Accessed", info.accessed),
        ("Created", info.created),
    ] {
        if let Some(time) = time {
            lines.push((label, format_time(time)));
        }
    }
    if let Some(mode) = info.mode {
        let text = permissions_text(mode, info.kind == Kind::Folder);
        let octal = if mode & 0o7000 != 0 {
            format!("{:04o}", mode & 0o7777)
        } else {
            format!("{:03o}", mode & 0o777)
        };
        lines.push(("Permissions", format!("{} ({octal})", &text[1..])));
    }
    if let Some(owner) = &info.owner {
        lines.push(("Owner", owner.clone()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn get<'a>(lines: &'a [(&'static str, String)], label: &str) -> Option<&'a str> {
        lines
            .iter()
            .find(|(l, _)| *l == label)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("f");
        fs::write(&f, vec![0; 1_234_567]).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o4755)).unwrap();
        let info = read(&f).unwrap();
        assert!(matches!(info.kind, Kind::File) && info.link.is_none());
        let lines = lines("f", "/x", &Ok(info), &Count::NotAFolder);
        assert_eq!(get(&lines, "Name"), Some("f"));
        assert_eq!(get(&lines, "Folder"), Some("/x"));
        assert_eq!(get(&lines, "Type"), Some("File"));
        assert_eq!(get(&lines, "Size"), Some("1,234,567 bytes (1.2 MiB)"));
        assert_eq!(get(&lines, "Permissions"), Some("rwsr-xr-x (4755)"));
        assert!(get(&lines, "Modified").is_some() && get(&lines, "Accessed").is_some());
        assert!(get(&lines, "Owner").is_some_and(|o| o.contains(':')));
        assert!(get(&lines, "Contains").is_none());
        let labels: Vec<_> = lines.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            &labels[..6],
            ["Name", "Folder", "Type", "Size", "Modified", "Accessed"]
        );
        let tail = &labels[labels.len() - 2..];
        assert_eq!(tail, ["Permissions", "Owner"]);
    }

    #[test]
    fn a_folder_while_counting_and_done() {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o750)).unwrap();
        let so_far = Totals {
            files: 1234,
            folders: 56,
            ..Totals::default()
        };
        let counting = lines("d", "/", &read(tmp.path()), &Count::Counting(so_far));
        assert_eq!(get(&counting, "Type"), Some("Folder"));
        assert_eq!(get(&counting, "Size"), Some("Counting..."));
        assert_eq!(
            get(&counting, "Contains"),
            Some("Counting... 1,234 files, 56 folders")
        );
        let totals = Totals {
            files: 12,
            folders: 3,
            bytes: 2048,
            unreadable: 2,
            other_fs: 0,
        };
        let done = lines("d", "/", &read(tmp.path()), &Count::Done(totals));
        assert_eq!(get(&done, "Size"), Some("2,048 bytes (2.0 KiB)"));
        assert_eq!(
            get(&done, "Contains"),
            Some("12 files, 3 folders, 2 unreadable")
        );
        assert_eq!(get(&done, "Permissions"), Some("rwxr-x--- (750)"));
        let clean = Count::Done(Totals {
            unreadable: 0,
            ..totals
        });
        let clean = lines("d", "/", &read(tmp.path()), &clean);
        assert_eq!(get(&clean, "Contains"), Some("12 files, 3 folders"));
    }

    #[test]
    fn symlinks_describe_their_target_and_say_when_it_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("t"), b"abc").unwrap();
        symlink("t", tmp.path().join("l")).unwrap();
        symlink("nowhere", tmp.path().join("dead")).unwrap();
        symlink(tmp.path(), tmp.path().join("dirlink")).unwrap();
        let l = lines("l", "/", &read(&tmp.path().join("l")), &Count::NotAFolder);
        assert_eq!(get(&l, "Type"), Some("Symbolic link to t"));
        assert_eq!(get(&l, "Size"), Some("3 bytes (3 B)"));
        let dead = read(&tmp.path().join("dead"));
        let dead = lines("dead", "/", &dead, &Count::NotAFolder);
        assert_eq!(
            get(&dead, "Type"),
            Some("Symbolic link to nowhere (missing)")
        );
        assert!(get(&dead, "Size").is_none() && get(&dead, "Modified").is_none());
        assert!(get(&dead, "Permissions").is_some(), "the link's own");
        let info = read(&tmp.path().join("dirlink")).unwrap();
        assert!(matches!(info.kind, Kind::Folder), "the target's kind");
        assert!(info.link.is_some());
    }

    #[test]
    fn others_and_missing_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("p");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let p = lines("p", "/", &read(&fifo), &Count::NotAFolder);
        assert_eq!(get(&p, "Type"), Some("Other (pipe)"));
        assert!(get(&p, "Size").is_none());
        let gone = lines("g", "/", &read(&tmp.path().join("g")), &Count::NotAFolder);
        assert_eq!(gone.len(), 3, "{gone:?}");
        assert!(gone[2].1.starts_with("Cannot read: "), "{gone:?}");
    }

    #[test]
    fn non_utf8_names_and_archive_entries() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let entry = Entry::archived(
            OsString::from_vec(b"bad\xff".to_vec()),
            EntryKind::File,
            false,
            Some(5),
            Some(SystemTime::UNIX_EPOCH),
            Some(0o100644),
            None,
        );
        let shown = lines(
            &entry.label,
            "/x/a.zip",
            &Ok(from_entry(&entry)),
            &Count::NotAFolder,
        );
        assert_eq!(get(&shown, "Name"), Some("bad\u{fffd}"));
        assert_eq!(get(&shown, "Size"), Some("5 bytes (5 B)"));
        assert!(get(&shown, "Accessed").is_none() && get(&shown, "Created").is_none());
        assert!(get(&shown, "Owner").is_none(), "zips store none");
        assert_eq!(get(&shown, "Permissions"), Some("rw-r--r-- (644)"));

        let link = Entry::archived(
            "l".into(),
            EntryKind::File,
            true,
            Some(1),
            None,
            Some(0o120777),
            Some("me:staff".into()),
        );
        let shown = lines("l", "/", &Ok(from_entry(&link)), &Count::NotAFolder);
        assert_eq!(get(&shown, "Type"), Some("Symbolic link"));
        assert_eq!(get(&shown, "Owner"), Some("me:staff"));
        let dir = Entry::archived("d".into(), EntryKind::Dir, false, None, None, None, None);
        assert!(matches!(from_entry(&dir).kind, Kind::Folder));
    }

    fn counted(path: &Path) -> Totals {
        count(path, &Progress::default(), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn count_adds_up_files_folders_and_bytes_without_following_links() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/x"), b"12345").unwrap();
        fs::write(d.join("a/b/.hidden"), b"123").unwrap();
        symlink("/", d.join("a/root")).unwrap();
        symlink("..", d.join("a/b/loop")).unwrap();
        let t = counted(d);
        assert_eq!(
            (t.files, t.folders, t.unreadable),
            (4, 2, 0),
            "x, .hidden, two links"
        );
        let links = fs::symlink_metadata(d.join("a/root")).unwrap().len()
            + fs::symlink_metadata(d.join("a/b/loop")).unwrap().len();
        assert_eq!(t.bytes, 8 + links);
    }

    #[test]
    fn others_count_as_empty_files() {
        let tmp = tempfile::tempdir().unwrap();
        nix::unistd::mkfifo(&tmp.path().join("p"), nix::sys::stat::Mode::S_IRWXU).unwrap();
        let t = counted(tmp.path());
        assert_eq!((t.files, t.bytes), (1, 0));
    }

    #[test]
    fn unreadable_folders_are_counted_and_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("f"), b"1").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&locked).is_ok(); // root reads anything
        let t = counted(tmp.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        if !readable {
            assert_eq!((t.folders, t.unreadable, t.files), (1, 1, 0));
        }
    }

    #[test]
    fn count_reports_progress_and_stops_when_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..10 {
            fs::write(tmp.path().join(i.to_string()), b"").unwrap();
        }
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let progress = Progress::default();
        let t = count(tmp.path(), &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(t.files, 10);
        assert_eq!(
            progress.so_far(),
            Totals {
                files: 10,
                folders: 1,
                ..Totals::default()
            }
        );
        assert!(count(tmp.path(), &progress, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn deep_trees_need_no_recursion() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = tmp.path().to_path_buf();
        for _ in 0..200 {
            p.push("d");
        }
        fs::create_dir_all(&p).unwrap();
        assert_eq!(counted(tmp.path()).folders, 200);
    }

    #[test]
    fn a_missing_folder_counts_as_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(counted(&tmp.path().join("gone")).unreadable, 1);
    }

    #[test]
    fn one_of_a_kind_is_singular() {
        let tmp = tempfile::tempdir().unwrap();
        let one = Count::Done(Totals {
            files: 1,
            folders: 1,
            bytes: 1,
            unreadable: 1,
            other_fs: 1,
        });
        let shown = lines("d", "/", &read(tmp.path()), &one);
        assert_eq!(get(&shown, "Size"), Some("1 byte (1 B)"));
        assert_eq!(
            get(&shown, "Contains"),
            Some("1 file, 1 folder, 1 unreadable, 1 other filesystem not counted")
        );
        let so_far = Totals {
            files: 1,
            folders: 1,
            ..Totals::default()
        };
        let counting = lines("d", "/", &read(tmp.path()), &Count::Counting(so_far));
        assert_eq!(
            get(&counting, "Contains"),
            Some("Counting... 1 file, 1 folder")
        );
    }

    #[test]
    fn a_count_stops_partway_when_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::write(tmp.path().join(i.to_string()), b"").unwrap();
        }
        let progress = Progress::default();
        let same = |_: &Path, m: &fs::Metadata| std::os::unix::fs::MetadataExt::dev(m);
        let stopped = walk(tmp.path(), &progress, |t| t.files < 5, &same);
        assert!(stopped.is_none());
        assert_eq!(progress.so_far().files, 5, "stopped right there");
    }

    #[test]
    fn other_filesystems_are_skipped_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("mnt/inner")).unwrap();
        fs::write(tmp.path().join("mnt/inner/f"), b"123").unwrap();
        fs::write(tmp.path().join("g"), b"1").unwrap();
        // Pretend `mnt` is a mount point: another device.
        let device = |p: &Path, m: &fs::Metadata| {
            let dev = std::os::unix::fs::MetadataExt::dev(m);
            if p.ends_with("mnt") { dev + 1 } else { dev }
        };
        let t = walk(tmp.path(), &Progress::default(), |_| true, &device).unwrap();
        assert_eq!((t.files, t.folders, t.bytes, t.other_fs), (1, 0, 1, 1));
        let shown = lines("d", "/", &read(tmp.path()), &Count::Done(t));
        assert_eq!(
            get(&shown, "Contains"),
            Some("1 file, 0 folders, 1 other filesystem not counted")
        );
        let two = Totals { other_fs: 2, ..t };
        let shown = lines("d", "/", &read(tmp.path()), &Count::Done(two));
        assert!(
            get(&shown, "Contains")
                .unwrap()
                .ends_with("2 other filesystems not counted")
        );
    }

    #[test]
    fn hard_links_add_their_bytes_once() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"12345").unwrap();
        fs::hard_link(tmp.path().join("a"), tmp.path().join("b")).unwrap();
        let t = counted(tmp.path());
        assert_eq!((t.files, t.bytes), (2, 5));
    }

    #[test]
    fn the_folder_line_is_left_out_at_the_root_and_created_sits_after_accessed() {
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let mut info = Info {
            kind: Kind::Folder,
            size: None,
            modified: Some(time),
            accessed: Some(time),
            created: Some(time),
            mode: Some(0o40755),
            owner: Some("root:root".into()),
            link: None,
        };
        let shown = lines("/", "", &Ok(info.clone()), &Count::Done(Totals::default()));
        let labels: Vec<_> = shown.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            [
                "Name",
                "Type",
                "Size",
                "Contains",
                "Modified",
                "Accessed",
                "Created",
                "Permissions",
                "Owner"
            ]
        );
        info.created = None;
        let shown = lines("x", "/", &Ok(info), &Count::Done(Totals::default()));
        assert!(get(&shown, "Created").is_none());
        assert_eq!(get(&shown, "Folder"), Some("/"));
    }
}
