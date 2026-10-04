use super::*;
use crate::file_ops::{
    Answer, Conflict, LinkAnswer, LinkQuestion, Operation, PasswordAnswer, PasswordQuestion,
    Settings,
};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Arc;

/// Records progress; cancels once `cancel_after` progress reports came in.
#[derive(Default)]
struct Watch {
    reports: usize,
    cancel_after: Option<usize>,
    last: Progress,
}

impl Observer for Watch {
    fn progress(&mut self, progress: &Progress) {
        self.reports += 1;
        self.last = progress.clone();
    }

    fn conflict(&mut self, _: &Conflict) -> Answer {
        unreachable!("compare never asks")
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|n| self.reports >= n)
    }

    fn link(&mut self, _: &LinkQuestion) -> LinkAnswer {
        unreachable!("compare never asks")
    }

    fn password(&mut self, _: &PasswordQuestion) -> PasswordAnswer {
        unreachable!("compare never asks")
    }
}

fn compare(first: &Path, second: &Path) -> Comparison {
    let (comparison, cancelled) = run(first, second, &mut Watch::default());
    assert!(!cancelled);
    comparison
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

fn rel(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[test]
fn files_with_the_same_bytes_are_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    // More than one chunk, differing only in the last byte of the second.
    let mut data = vec![7u8; CHUNK + 10];
    write(&a, &data);
    write(&b, &data);
    let result = compare(&a, &b);
    assert!(result.identical());
    assert_eq!(result.files, 1);
    *data.last_mut().unwrap() = 8;
    write(&b, &data);
    assert_eq!(compare(&a, &b).differences, [Difference::Content(rel(""))]);
}

#[test]
fn empty_files_are_identical_and_sizes_decide_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write(&a, "");
    write(&b, "");
    assert!(compare(&a, &b).identical());
    write(&b, "x");
    let mut watch = Watch::default();
    let (result, _) = run(&a, &b, &mut watch);
    assert_eq!(result.differences, [Difference::Content(rel(""))]);
    assert_eq!(watch.last.bytes_total, 0, "nothing read");
}

#[test]
fn a_picked_symlink_is_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write(&a, "same");
    symlink(&a, &b).unwrap();
    assert!(compare(&a, &b).identical());
    fs::create_dir(tmp.path().join("d")).unwrap();
    symlink(tmp.path().join("d"), tmp.path().join("to-d")).unwrap();
    assert!(compare(&tmp.path().join("d"), &tmp.path().join("to-d")).identical());
}

#[test]
fn a_file_and_a_folder_differ_in_type() {
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("f"), "");
    fs::create_dir(tmp.path().join("d")).unwrap();
    let result = compare(&tmp.path().join("f"), &tmp.path().join("d"));
    assert_eq!(result.differences, [Difference::Type(rel(""))]);
}

#[test]
fn a_missing_pick_is_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("gone"));
    write(&a, "");
    let result = compare(&a, &b);
    assert!(matches!(
        &result.differences[..],
        [Difference::Unreadable { path, .. }] if *path == b
    ));
    let result = compare(&b, &b);
    assert_eq!(result.differences.len(), 2, "both sides reported");
}

#[test]
fn two_picked_fifos_are_never_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for path in [&a, &b] {
        nix::unistd::mkfifo(path, nix::sys::stat::Mode::S_IRWXU).unwrap();
    }
    // Opening one would block forever.
    assert!(compare(&a, &b).identical());
}

#[test]
fn identical_trees_count_their_files_hidden_ones_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        write(&root.join("one.txt"), "1");
        write(&root.join(".hidden"), "h");
        write(&root.join("sub/deep/two.txt"), "22");
        fs::create_dir_all(root.join("empty")).unwrap();
        symlink("one.txt", root.join("link")).unwrap();
    }
    let result = compare(&a, &b);
    assert!(result.identical(), "{:?}", result.differences);
    assert_eq!(result.files, 3);
}

#[test]
fn tree_differences_are_listed_by_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write(&a.join("same.txt"), "s");
    write(&b.join("same.txt"), "s");
    write(&a.join("only-a.txt"), "");
    write(&a.join("gone/inner.txt"), "");
    write(&b.join("only-b/inner.txt"), "");
    write(&a.join("x/changed.txt"), "abc");
    write(&b.join("x/changed.txt"), "abd");
    write(&a.join("x/longer.txt"), "abc");
    write(&b.join("x/longer.txt"), "abcd");
    write(&a.join("kind"), "");
    fs::create_dir_all(b.join("kind")).unwrap();
    symlink("one", a.join("link")).unwrap();
    symlink("two", b.join("link")).unwrap();
    symlink("same.txt", a.join("file-or-link")).unwrap();
    write(&b.join("file-or-link"), "s");
    // Contents and times don't matter for special files.
    nix::unistd::mkfifo(&a.join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    nix::unistd::mkfifo(&b.join("pipe"), nix::sys::stat::Mode::S_IRUSR).unwrap();
    let result = compare(&a, &b);
    assert_eq!(
        result.differences,
        [
            Difference::Type(rel("file-or-link")),
            Difference::OnlyFirst {
                path: rel("gone"),
                is_dir: true
            },
            Difference::Type(rel("kind")),
            Difference::Content(rel("link")),
            Difference::OnlyFirst {
                path: rel("only-a.txt"),
                is_dir: false
            },
            Difference::OnlySecond {
                path: rel("only-b"),
                is_dir: true
            },
            Difference::Content(rel("x/changed.txt")),
            Difference::Content(rel("x/longer.txt")),
        ]
    );
    // same.txt, x/changed.txt, x/longer.txt.
    assert_eq!(result.files, 3);
}

#[test]
fn a_tree_compared_with_itself_is_identical() {
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("a/b/c"), "c");
    assert!(compare(tmp.path(), tmp.path()).identical());
}

#[test]
fn unreadable_entries_are_listed_and_never_identical() {
    if is_root() {
        return; // root reads everything
    }
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        write(&root.join("secret.txt"), "s");
        write(&root.join("locked/x"), "x");
        write(&root.join("z.txt"), "z");
    }
    fs::set_permissions(b.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(a.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = compare(&a, &b);
    fs::set_permissions(a.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    let paths: Vec<_> = result
        .differences
        .iter()
        .map(|d| match d {
            Difference::Unreadable { path, message } => {
                assert!(!message.is_empty());
                path.clone()
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(paths, [a.join("locked"), b.join("secret.txt")]);
}

#[test]
fn a_dangling_link_still_compares_by_its_target_text() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        fs::create_dir(root).unwrap();
        symlink("nowhere", root.join("l")).unwrap();
    }
    assert!(compare(&a, &b).identical());
}

#[test]
fn progress_counts_files_and_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        write(&root.join("big"), vec![1u8; 3000]);
        write(&root.join("small"), "12");
    }
    write(&b.join("other-size"), "1");
    write(&a.join("other-size"), "12");
    let mut watch = Watch::default();
    let (_, cancelled) = run(&a, &b, &mut watch);
    assert!(!cancelled);
    let last = &watch.last;
    assert_eq!((last.files_done, last.files_total), (3, 3));
    assert_eq!((last.bytes_done, last.bytes_total), (3002, 3002));
}

#[test]
fn cancel_stops_the_walk_and_the_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        write(&root.join("sub/f"), vec![0u8; 3 * CHUNK]);
    }
    for after in 1..=4 {
        let mut watch = Watch {
            cancel_after: Some(after),
            ..Watch::default()
        };
        let (_, cancelled) = run(&a, &b, &mut watch);
        assert!(cancelled, "cancelled after {after} reports");
    }
}

#[test]
fn the_engine_runs_compare_without_logging() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write(&a, "1");
    write(&b, "2");
    let log_dir = tmp.path().join("logs");
    let log = crate::oplog::OperationLog::open(&log_dir).unwrap();
    let settings = Settings {
        trash: |_| Err("never".into()),
        log: Some(Arc::new(log)),
        mounts: None,
    };
    let operation = Operation::Compare {
        first: a,
        second: b,
    };
    let report = crate::file_ops::run(&operation, &mut Watch::default(), &settings);
    let comparison = report.comparison.unwrap();
    assert_eq!(comparison.differences, [Difference::Content(rel(""))]);
    assert!(!report.cancelled);
    let logged: usize = fs::read_dir(&log_dir)
        .map(|dir| {
            dir.map(|e| fs::read(e.unwrap().path()).unwrap().len())
                .sum()
        })
        .unwrap_or(0);
    assert_eq!(logged, 0);
}

#[test]
fn a_job_reports_the_comparison() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    write(&a, "same");
    write(&b, "same");
    let operation = Operation::Compare {
        first: a,
        second: b,
    };
    let job = crate::file_ops::Job::spawn(operation, Settings::default()).unwrap();
    let report = loop {
        match job.try_event() {
            Some(crate::file_ops::Event::Finished(report)) => break report,
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    };
    assert!(report.comparison.unwrap().identical());
}

#[test]
fn sockets_and_devices_compare_by_type() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(a.join("s")).unwrap();
    nix::unistd::mkfifo(&b.join("s"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    assert_eq!(compare(&a, &b).differences, [Difference::Type(rel("s"))]);
    // Character devices: by type only, never read.
    let (null, zero) = (Path::new("/dev/null"), Path::new("/dev/zero"));
    assert!(compare(null, zero).identical());
}

#[test]
fn entries_that_cannot_be_examined_are_unreadable() {
    if is_root() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for root in [&a, &b] {
        write(&root.join("d/x"), "x");
    }
    // Readable but not searchable: names list, metadata fails.
    fs::set_permissions(a.join("d"), fs::Permissions::from_mode(0o444)).unwrap();
    let result = compare(&a, &b);
    fs::set_permissions(a.join("d"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(
            &result.differences[..],
            [Difference::Unreadable { path, .. }] if *path == a.join("d/x")
        ),
        "{:?}",
        result.differences
    );
}
