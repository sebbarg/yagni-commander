//! Alt-F7's walk: below the root, depth first in name order, hidden
//! entries included, symlinks never followed, never into another
//! filesystem.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::masks::Masks;
use super::text::TextQuery;
use crate::entry::{Entry, OwnerCache, stat_entry};
use crate::mounts::Mounts;

/// What to look for, and where.
#[derive(Debug, Clone)]
pub struct Query {
    pub root: PathBuf,
    pub masks: Masks,
    /// Folder names (any case) neither listed nor entered below the root:
    /// the dialog's "Skip folders".
    pub skip: Vec<String>,
    pub text: Option<TextQuery>,
}

/// How a search went.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    /// Entries looked at.
    pub seen: u64,
    pub found: u64,
    /// Folders that couldn't be listed, entries that couldn't be stat'ed,
    /// and files that couldn't be read for the text.
    pub unreadable: u64,
    /// Mount points below the root, not entered.
    pub other_filesystems: u64,
    /// Folders in `Query::skip`, not entered.
    pub skipped: u64,
    pub stopped: bool,
}

/// A running search's counts, readable from another thread (the status
/// line, and what a stopped search shows).
#[derive(Debug, Default)]
pub struct Progress {
    seen: AtomicU64,
    unreadable: AtomicU64,
    other_filesystems: AtomicU64,
    skipped: AtomicU64,
}

impl Progress {
    /// The counts so far; `found` and `stopped` are the caller's.
    pub fn so_far(&self) -> SearchSummary {
        SearchSummary {
            seen: self.seen.load(Ordering::Relaxed),
            unreadable: self.unreadable.load(Ordering::Relaxed),
            other_filesystems: self.other_filesystems.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            ..SearchSummary::default()
        }
    }

    fn add(counter: &AtomicU64, total: &mut u64) {
        counter.fetch_add(1, Ordering::Relaxed);
        *total += 1;
    }
}

/// Runs `query`, handing each result to `found` as it turns up: a panel
/// entry named by its path relative to the root. `progress`
/// counts as it goes. Stops when `cancel` is set.
pub fn search(
    query: &Query,
    cancel: &AtomicBool,
    progress: &Progress,
    found: &mut dyn FnMut(Entry),
) -> SearchSummary {
    let mut summary = SearchSummary::default();
    let root = std::path::absolute(&query.root).unwrap_or_else(|_| query.root.clone());
    let Ok(root_meta) = fs::metadata(&root) else {
        Progress::add(&progress.unreadable, &mut summary.unreadable);
        return summary;
    };
    let mounts = Mounts::read();
    // Mount points are listed by their real path.
    let real_root = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let mut owners = OwnerCache::default();
    let mut stack = vec![(root, PathBuf::new())];
    while let Some((dir, relative)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            Progress::add(&progress.unreadable, &mut summary.unreadable);
            continue;
        };
        let mut names: Vec<OsString> = read.filter_map(|d| Some(d.ok()?.file_name())).collect();
        names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
        let mut folders = Vec::new();
        for name in names {
            if cancel.load(Ordering::Relaxed) {
                summary.stopped = true;
                return summary;
            }
            Progress::add(&progress.seen, &mut summary.seen);
            let path = dir.join(&name);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                Progress::add(&progress.unreadable, &mut summary.unreadable);
                continue;
            };
            let label = name.to_string_lossy();
            let is_dir = meta.is_dir();
            if is_dir
                && query
                    .skip
                    .iter()
                    .any(|s| s.to_lowercase() == label.to_lowercase())
            {
                Progress::add(&progress.skipped, &mut summary.skipped);
                continue;
            }
            let rel = relative.join(&name);
            if query.masks.matches(&label, is_dir) {
                let hit = match &query.text {
                    None => true,
                    Some(text) if meta.is_file() => match text.is_match_in(&path, cancel) {
                        Ok(contains) => contains != text.not_containing,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                            summary.stopped = true;
                            return summary;
                        }
                        Err(_) => {
                            Progress::add(&progress.unreadable, &mut summary.unreadable);
                            false
                        }
                    },
                    Some(_) => false,
                };
                if hit {
                    summary.found += 1;
                    found(stat_entry(
                        rel.clone().into_os_string(),
                        &path,
                        Some(meta.clone()),
                        &mut owners,
                    ));
                }
            }
            if is_dir && query.masks.enters(&label) {
                // Another device and a mount point: a btrfs subvolume (a
                // device of its own, no mount) is searched.
                let (dev, root_dev) = (meta.dev(), root_meta.dev());
                if dev == root_dev || !mounts.is_mount_point(&real_root.join(&rel), dev, root_dev) {
                    folders.push((path, rel));
                } else {
                    Progress::add(&progress.other_filesystems, &mut summary.other_filesystems);
                }
            }
        }
        // Popped in name order, after this folder's own entries.
        stack.extend(folders.into_iter().rev());
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::text::Text;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        fs::create_dir_all(p.join("src/deep")).unwrap();
        fs::create_dir(p.join("target")).unwrap();
        for (f, body) in [
            ("a.rs", "fn main"),
            ("b.txt", "hello"),
            (".hidden.rs", "x"),
            ("src/main.rs", "fn main() {}"),
            ("src/deep/x.rs", "needle"),
            ("target/out.rs", "needle"),
        ] {
            fs::write(p.join(f), body).unwrap();
        }
        symlink(p.join("src"), p.join("link")).unwrap();
        symlink(p, p.join("loop")).unwrap();
        tmp
    }

    fn query(root: &Path, masks: &str, text: Option<Text>) -> Query {
        Query {
            root: root.to_path_buf(),
            masks: Masks::parse(masks),
            skip: Vec::new(),
            text: text.map(|t| t.compile().unwrap()),
        }
    }

    fn run(root: &Path, masks: &str, text: Option<Text>) -> (Vec<String>, SearchSummary) {
        let mut names = Vec::new();
        let progress = Progress::default();
        let summary = search(
            &query(root, masks, text),
            &AtomicBool::new(false),
            &progress,
            &mut |f| {
                assert!(root.join(&f.name).exists() || f.is_symlink);
                names.push(f.label.clone());
            },
        );
        // What a stopped search shows: the counts so far.
        assert_eq!(
            SearchSummary {
                found: summary.found,
                ..progress.so_far()
            },
            summary
        );
        assert_eq!(summary.found, names.len() as u64);
        (names, summary)
    }

    fn needle() -> Text {
        Text {
            pattern: "needle".into(),
            ..Text::default()
        }
    }

    #[test]
    fn finds_by_name_recursively_in_order_with_hidden_files() {
        let tmp = fixture();
        let (names, s) = run(tmp.path(), "*.rs", None);
        assert_eq!(
            names,
            [
                ".hidden.rs",
                "a.rs",
                "src/main.rs",
                "src/deep/x.rs",
                "target/out.rs"
            ]
        );
        assert!(!s.stopped);
        assert_eq!(s.unreadable, 0);
    }

    #[test]
    fn results_are_panel_entries() {
        let tmp = fixture();
        let mut found = Vec::new();
        search(
            &query(tmp.path(), "*", None),
            &AtomicBool::new(false),
            &Progress::default(),
            &mut |f| found.push(f),
        );
        let by = |label: &str| found.iter().find(|e| e.label == label).unwrap().clone();
        assert_eq!(by("src").kind, crate::EntryKind::Dir);
        assert_eq!(by("src/main.rs").size, Some(12));
        let link = by("link");
        assert!(link.is_symlink);
        assert_eq!(link.kind, crate::EntryKind::Dir, "like the panel shows it");
    }

    #[test]
    fn folders_and_links_match_by_name_and_links_are_not_followed() {
        let tmp = fixture();
        let (names, _) = run(tmp.path(), "*", None);
        assert!(names.contains(&"src".to_owned()));
        assert!(names.contains(&"link".to_owned()));
        assert!(
            !names
                .iter()
                .any(|n| n.starts_with("link/") || n.starts_with("loop/"))
        );
    }

    #[test]
    fn skipped_folders_are_neither_listed_nor_entered_and_are_counted() {
        let tmp = fixture();
        let p = tmp.path();
        for dir in [".git/objects", "node_modules/x", "src/NODE_MODULES"] {
            fs::create_dir_all(p.join(dir)).unwrap();
            fs::write(p.join(dir).join("in.rs"), b"").unwrap();
        }
        let mut q = query(p, "*", None);
        q.skip = vec![".git".into(), "node_modules".into()];
        let mut names = Vec::new();
        let s = search(
            &q,
            &AtomicBool::new(false),
            &Progress::default(),
            &mut |f| names.push(f.label),
        );
        assert!(
            !names
                .iter()
                .any(|n| n.contains(".git") || n.to_lowercase().contains("node_modules")),
            "{names:?}"
        );
        assert_eq!(s.skipped, 3, "case-insensitive, at any level");
        // The root itself is searched even when it has a skipped name.
        let mut q = query(&p.join(".git"), "*", None);
        q.skip = vec![".git".into()];
        let mut inside = Vec::new();
        search(
            &q,
            &AtomicBool::new(false),
            &Progress::default(),
            &mut |f| inside.push(f.label),
        );
        assert!(inside.contains(&"objects/in.rs".to_owned()), "{inside:?}");
    }

    #[test]
    fn folder_excludes_are_not_entered() {
        let tmp = fixture();
        let (names, _) = run(tmp.path(), "*.rs | target/", None);
        assert!(!names.iter().any(|n| n.starts_with("target")));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn text_searches_only_matching_regular_files() {
        let tmp = fixture();
        assert_eq!(
            run(tmp.path(), "", Some(needle())).0,
            ["src/deep/x.rs", "target/out.rs"]
        );
        let not = Text {
            not_containing: true,
            ..needle()
        };
        let (names, _) = run(tmp.path(), "*.rs", Some(not));
        assert_eq!(names, [".hidden.rs", "a.rs", "src/main.rs"]);
    }

    #[test]
    fn unreadable_folders_and_files_are_counted() {
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let tmp = fixture();
        let p = tmp.path();
        fs::set_permissions(p.join("src/deep"), fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(p.join("b.txt"), fs::Permissions::from_mode(0o000)).unwrap();
        let hello = Text {
            pattern: "hello".into(),
            ..Text::default()
        };
        let not_hello = Text {
            not_containing: true,
            ..hello.clone()
        };
        let (names, s) = run(p, "", Some(hello));
        let (not_names, _) = run(p, "*.txt", Some(not_hello));
        fs::set_permissions(p.join("src/deep"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(names.is_empty());
        assert_eq!(s.unreadable, 2);
        assert!(not_names.is_empty(), "an unreadable file is never a result");
    }

    #[test]
    fn cancel_stops_the_walk() {
        let tmp = fixture();
        let s = search(
            &query(tmp.path(), "", None),
            &AtomicBool::new(true),
            &Progress::default(),
            &mut |_| panic!("found after cancel"),
        );
        assert!(s.stopped);
    }

    #[test]
    fn cancel_during_a_file_read_stops_the_walk() {
        // The flag is set by the first result; the next file read sees it.
        let tmp = fixture();
        let cancel = AtomicBool::new(false);
        let mut found = 0;
        let s = search(
            &query(tmp.path(), "", Some(needle())),
            &cancel,
            &Progress::default(),
            &mut |_| {
                found += 1;
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            },
        );
        assert!(s.stopped);
        assert_eq!(found, 1);
    }

    #[test]
    fn other_filesystems_are_skipped() {
        // /proc is its own filesystem on Linux: searching / skips it. The
        // walk lists / first, so stopping once "proc" is found keeps this
        // cheap; it was counted as skipped just before.
        if !Path::new("/proc/self").exists() {
            return;
        }
        let cancel = AtomicBool::new(false);
        let mut names = Vec::new();
        let s = search(
            &query(Path::new("/"), "proc", None),
            &cancel,
            &Progress::default(),
            &mut |f| {
                if f.label == "proc" {
                    cancel.store(true, Ordering::Relaxed);
                }
                names.push(f.label);
            },
        );
        assert!(s.other_filesystems >= 1);
        assert!(
            names.contains(&"proc".to_owned()),
            "a mount point can match"
        );
        assert!(!names.iter().any(|n| n.starts_with("proc/")));
    }

    #[test]
    fn a_missing_root_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let (names, s) = run(&tmp.path().join("gone"), "", None);
        assert!(names.is_empty());
        assert_eq!(s.unreadable, 1);
    }
}
