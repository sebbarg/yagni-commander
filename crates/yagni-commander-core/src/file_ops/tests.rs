use super::*;
use std::collections::VecDeque;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::SystemTime;

/// Answers conflicts from a script and cancels on demand.
#[derive(Default)]
struct Script {
    answers: VecDeque<Answer>,
    conflicts: Vec<Conflict>,
    /// Cancel once this many bytes have been copied.
    cancel_at_bytes: Option<u64>,
    /// Cancel on the progress report naming this path.
    cancel_at_path: Option<PathBuf>,
    cancelled: bool,
    last: Progress,
}

impl Script {
    fn answering(answers: &[Answer]) -> Self {
        Self {
            answers: answers.iter().copied().collect(),
            ..Self::default()
        }
    }
}

impl Observer for Script {
    fn progress(&mut self, progress: &Progress) {
        if self
            .cancel_at_bytes
            .is_some_and(|at| progress.bytes_done >= at)
            || self.cancel_at_path.as_ref() == Some(&progress.current)
        {
            self.cancelled = true;
        }
        self.last = progress.clone();
    }

    fn conflict(&mut self, conflict: &Conflict) -> Answer {
        self.conflicts.push(conflict.clone());
        self.answers.pop_front().expect("unexpected conflict")
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

/// "Trash" that deletes, and fails for names containing "stuck".
fn fake_trash(path: &Path) -> Result<(), String> {
    if path.to_string_lossy().contains("stuck") {
        return Err("cannot trash".into());
    }
    fs::remove_dir_all(path)
        .or_else(|_| fs::remove_file(path))
        .map_err(|e| e.to_string())
}

fn run_script(operation: &Operation, script: &mut Script) -> Report {
    run_with(operation, script, fake_trash)
}

/// `src/` holds `a.txt` (5 bytes), `dir/b.txt` (3 bytes), `dir/deep/c.txt`
/// (1 byte); `dst/` is empty.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(src.join("dir/deep")).unwrap();
    fs::create_dir(&dst).unwrap();
    fs::write(src.join("a.txt"), b"hello").unwrap();
    fs::write(src.join("dir/b.txt"), b"bee").unwrap();
    fs::write(src.join("dir/deep/c.txt"), b"c").unwrap();
    (tmp, src, dst)
}

fn copy(sources: &[PathBuf], to: &Path) -> Operation {
    Operation::Copy {
        sources: sources.to_vec(),
        to: to.to_path_buf(),
    }
}

fn mv(sources: &[PathBuf], to: &Path) -> Operation {
    Operation::Move {
        sources: sources.to_vec(),
        to: to.to_path_buf(),
    }
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

/// Names in `dir`, sorted, to check that no temporary files are left.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn mtime(path: &Path) -> SystemTime {
    path.symlink_metadata().unwrap().modified().unwrap()
}

fn mode(path: &Path) -> u32 {
    path.symlink_metadata().unwrap().permissions().mode() & 0o7777
}

fn set_mtime(path: &Path, secs: u64) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    File::open(path).unwrap().set_modified(time).unwrap();
}

fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

#[test]
fn copies_files_and_trees_with_times_permissions_and_progress() {
    let (_tmp, src, dst) = fixture();
    set_mtime(&src.join("a.txt"), 1_000_000);
    set_mtime(&src.join("dir"), 2_000_000);
    fs::set_permissions(src.join("a.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(src.join("dir"), fs::Permissions::from_mode(0o750)).unwrap();

    let mut script = Script::default();
    let report = run_script(
        &copy(&[src.join("a.txt"), src.join("dir")], &dst),
        &mut script,
    );

    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("a.txt")), "hello");
    assert_eq!(read(dst.join("dir/deep/c.txt")), "c");
    assert_eq!(read(src.join("a.txt")), "hello", "source untouched");
    assert_eq!(mtime(&dst.join("a.txt")), mtime(&src.join("a.txt")));
    assert_eq!(mtime(&dst.join("dir")), mtime(&src.join("dir")));
    assert_eq!(mode(&dst.join("a.txt")), 0o640);
    assert_eq!(mode(&dst.join("dir")), 0o750);
    let p = &script.last;
    assert_eq!((p.items_done, p.items_total), (2, 2));
    assert_eq!((p.bytes_done, p.bytes_total), (9, 9));
}

#[test]
fn symlinks_are_copied_as_links() {
    let (_tmp, src, dst) = fixture();
    symlink("a.txt", src.join("link")).unwrap();
    symlink("missing", src.join("broken")).unwrap();
    let report = run_script(
        &copy(&[src.join("link"), src.join("broken")], &dst),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(fs::read_link(dst.join("link")).unwrap(), Path::new("a.txt"));
    assert_eq!(
        fs::read_link(dst.join("broken")).unwrap(),
        Path::new("missing")
    );
}

#[test]
fn skip_keeps_the_target_and_overwrite_replaces_it() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let op = copy(&[src.join("a.txt")], &dst);

    let mut script = Script::answering(&[Answer::Skip]);
    let report = run_script(&op, &mut script);
    assert_eq!(report.skipped, 1);
    assert_eq!(read(dst.join("a.txt")), "old");
    assert_eq!(
        script.conflicts,
        [Conflict {
            source: src.join("a.txt"),
            target: dst.join("a.txt")
        }]
    );
    assert_eq!(script.last.bytes_done, 5, "skipped bytes count as done");

    let report = run_script(&op, &mut Script::answering(&[Answer::Overwrite]));
    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("a.txt")), "hello");
    assert_eq!(names(&dst), ["a.txt"], "no temporary file left");
}

#[test]
fn all_answers_are_asked_once() {
    let (_tmp, src, dst) = fixture();
    let op = copy(&[src.join("dir")], &dst);
    run_script(&op, &mut Script::default());
    fs::write(src.join("dir/b.txt"), b"new").unwrap();
    fs::write(src.join("dir/deep/c.txt"), b"new").unwrap();

    let mut script = Script::answering(&[Answer::SkipAll]);
    let report = run_script(&op, &mut script);
    assert_eq!((script.conflicts.len(), report.skipped), (1, 2));
    assert_eq!(read(dst.join("dir/b.txt")), "bee");

    let mut script = Script::answering(&[Answer::OverwriteAll]);
    run_script(&op, &mut script);
    assert_eq!(script.conflicts.len(), 1);
    assert_eq!(read(dst.join("dir/b.txt")), "new");
    assert_eq!(read(dst.join("dir/deep/c.txt")), "new");
}

#[test]
fn cancel_answer_stops_the_operation() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let op = copy(&[src.join("a.txt"), src.join("dir")], &dst);
    let report = run_script(&op, &mut Script::answering(&[Answer::Cancel]));
    assert!(report.cancelled);
    assert_eq!(read(dst.join("a.txt")), "old");
    assert!(!dst.join("dir").exists());
}

#[test]
fn cancelling_mid_file_removes_the_partial_copy() {
    let (_tmp, src, dst) = fixture();
    let big = src.join("big");
    fs::write(&big, vec![7u8; (CHUNK * 2) as usize]).unwrap();
    let mut script = Script {
        cancel_at_bytes: Some(1),
        ..Script::default()
    };
    let report = run_script(&copy(std::slice::from_ref(&big), &dst), &mut script);
    assert!(report.cancelled);
    assert_eq!(script.last.bytes_done, CHUNK);
    assert!(names(&dst).is_empty());

    // The same when replacing: the old target survives untouched.
    fs::write(dst.join("big"), b"old").unwrap();
    let mut script = Script {
        cancel_at_bytes: Some(1),
        ..Script::answering(&[Answer::Overwrite])
    };
    let report = run_script(&copy(&[big], &dst), &mut script);
    assert!(report.cancelled);
    assert_eq!(read(dst.join("big")), "old");
    assert_eq!(names(&dst), ["big"]);
}

#[test]
fn cancel_before_start_or_during_scan_does_nothing() {
    let (_tmp, src, dst) = fixture();
    let mut script = Script {
        cancelled: true,
        ..Script::default()
    };
    for op in [
        copy(&[src.join("dir")], &dst),
        mv(&[src.join("dir")], &dst),
        Operation::Trash {
            sources: vec![src.join("dir")],
        },
    ] {
        assert!(run_script(&op, &mut script).cancelled);
    }
    assert!(names(&dst).is_empty());
    assert!(src.join("dir").exists());
}

#[test]
fn copying_into_an_existing_directory_merges() {
    let (_tmp, src, dst) = fixture();
    fs::create_dir(dst.join("dir")).unwrap();
    fs::write(dst.join("dir/mine"), b"").unwrap();
    set_mtime(&src.join("dir"), 2_000_000);
    let report = run_script(&copy(&[src.join("dir")], &dst), &mut Script::default());
    assert_eq!(report, Report::default());
    assert_eq!(names(&dst.join("dir")), ["b.txt", "deep", "mine"]);
    assert_ne!(
        mtime(&dst.join("dir")),
        mtime(&src.join("dir")),
        "an existing directory keeps its own attributes"
    );
}

#[test]
fn file_and_directory_never_replace_each_other() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("dir"), b"file").unwrap();
    fs::create_dir(dst.join("a.txt")).unwrap();
    let report = run_script(
        &copy(&[src.join("a.txt"), src.join("dir")], &dst),
        &mut Script::default(),
    );
    let messages: Vec<_> = report.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "a directory with this name exists",
            "a file with this name exists"
        ]
    );
    assert_eq!(read(dst.join("dir")), "file");
}

#[test]
fn refuses_copies_onto_or_into_themselves_and_roots() {
    let (_tmp, src, dst) = fixture();
    let op = copy(
        &[src.join("a.txt"), src.join("dir"), PathBuf::from("/")],
        &src.join("dir/deep"),
    );
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report.failures.len(), 2);
    assert_eq!(
        report.failures[0].message,
        "a directory cannot be put inside itself"
    );
    assert!(report.failures[1].message.contains("root"));
    assert!(
        src.join("dir/deep/a.txt").exists(),
        "other sources still run"
    );

    let report = run_script(&copy(&[src.join("a.txt")], &src), &mut Script::default());
    assert_eq!(report.failures[0].message, "source and target are the same");
    assert!(names(&dst).is_empty());
}

#[test]
fn destination_must_be_a_directory() {
    let (_tmp, src, dst) = fixture();
    let report = run_script(
        &copy(&[src.join("a.txt")], &dst.join("missing")),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, dst.join("missing"));
}

#[test]
fn errors_are_collected_and_the_rest_continues() {
    let (_tmp, src, dst) = fixture();
    nix::unistd::mkfifo(&src.join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    let op = copy(
        &[src.join("missing"), src.join("pipe"), src.join("a.txt")],
        &dst,
    );
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report.failures.len(), 2);
    assert_eq!(report.failures[0].path, src.join("missing"));
    assert_eq!(report.failures[1].path, src.join("pipe"));
    assert_eq!(read(dst.join("a.txt")), "hello");
}

#[test]
fn unreadable_entries_fail_without_leaving_partial_files() {
    if is_root() {
        return; // root reads everything
    }
    let (_tmp, src, dst) = fixture();
    fs::set_permissions(src.join("a.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(src.join("dir/deep"), fs::Permissions::from_mode(0o000)).unwrap();
    let report = run_script(
        &copy(&[src.join("a.txt"), src.join("dir")], &dst),
        &mut Script::default(),
    );
    fs::set_permissions(src.join("dir/deep"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(report.failures.len(), 2);
    assert_eq!(names(&dst), ["dir"]);
    assert_eq!(read(dst.join("dir/b.txt")), "bee");
}

#[test]
fn unwritable_target_directory_is_reported() {
    if is_root() {
        return;
    }
    let (_tmp, src, dst) = fixture();
    fs::create_dir(dst.join("dir")).unwrap();
    fs::write(dst.join("dir/b.txt"), b"old").unwrap();
    fs::set_permissions(dst.join("dir"), fs::Permissions::from_mode(0o555)).unwrap();
    let report = run_script(
        &copy(&[src.join("dir")], &dst),
        &mut Script::answering(&[Answer::OverwriteAll]),
    );
    fs::set_permissions(dst.join("dir"), fs::Permissions::from_mode(0o755)).unwrap();
    // b.txt: no room for the replacement; deep: cannot create.
    assert_eq!(report.failures.len(), 2);
    assert_eq!(read(dst.join("dir/b.txt")), "old");
}

#[test]
fn move_renames_within_a_filesystem() {
    let (_tmp, src, dst) = fixture();
    symlink("a.txt", src.join("link")).unwrap();
    let op = mv(
        &[src.join("a.txt"), src.join("dir"), src.join("link")],
        &dst,
    );
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report, Report::default());
    assert!(names(&src).is_empty());
    assert_eq!(read(dst.join("dir/deep/c.txt")), "c");
    assert_eq!(fs::read_link(dst.join("link")).unwrap(), Path::new("a.txt"));
}

#[test]
fn move_onto_existing_entries_merges_and_asks() {
    let (_tmp, src, dst) = fixture();
    fs::create_dir_all(dst.join("dir/deep")).unwrap();
    fs::write(dst.join("dir/b.txt"), b"old").unwrap();
    fs::write(dst.join("dir/deep/c.txt"), b"old").unwrap();

    let mut script = Script::answering(&[Answer::Overwrite, Answer::Skip]);
    let op = mv(&[src.join("dir")], &dst);
    let report = run_script(&op, &mut script);
    assert_eq!(report.skipped, 1);
    let replaced = script.conflicts[0].target.clone();
    let kept = script.conflicts[1].target.clone();
    assert_eq!(
        read(&replaced),
        if replaced.ends_with("b.txt") {
            "bee"
        } else {
            "c"
        }
    );
    assert_eq!(read(&kept), "old");
    // The skipped file and the directories holding it stay behind.
    let left: Vec<_> = walk(&src.join("dir"));
    assert_eq!(left.len(), 1);
    assert!(kept.ends_with(left[0].strip_prefix(&src).unwrap()));

    let report = run_script(&op, &mut Script::answering(&[Answer::Overwrite]));
    assert_eq!(report, Report::default());
    assert!(!src.join("dir").exists());
}

/// Files (not directories) under `dir`.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn move_type_clashes_and_self_moves_fail() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("dir"), b"").unwrap();
    fs::create_dir(dst.join("a.txt")).unwrap();
    let report = run_script(
        &mv(&[src.join("a.txt"), src.join("dir")], &dst),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 2);
    assert!(src.join("a.txt").exists() && src.join("dir").exists());

    let report = run_script(
        &mv(&[src.join("dir")], &src.join("dir/deep")),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1);
    let report = run_script(&mv(&[src.join("missing")], &dst), &mut Script::default());
    assert_eq!(report.failures[0].path, src.join("missing"));
}

#[test]
fn move_by_copy_deletes_only_what_was_copied() {
    // What a move to another filesystem does, run directly since the tests
    // may not have a second filesystem.
    let (_tmp, src, dst) = fixture();
    fs::create_dir_all(dst.join("dir")).unwrap();
    fs::write(dst.join("dir/b.txt"), b"old").unwrap();
    symlink("b.txt", src.join("dir/link")).unwrap();
    let mut script = Script::answering(&[Answer::Skip]);
    let mut engine = Engine {
        observer: &mut script,
        progress: Progress::default(),
        always: None,
        report: Report::default(),
    };
    assert_eq!(
        engine.copy_entry(&src.join("dir"), &dst.join("dir"), true),
        Step::Incomplete
    );
    assert_eq!(
        engine.copy_entry(&src.join("a.txt"), &dst.join("a.txt"), true),
        Step::Done
    );
    assert_eq!(engine.report.skipped, 1);
    assert_eq!(walk(&src), [src.join("dir/b.txt")]);
    assert_eq!(read(dst.join("dir/deep/c.txt")), "c");
    assert_eq!(
        fs::read_link(dst.join("dir/link")).unwrap(),
        Path::new("b.txt")
    );

    engine.always = Some(true);
    assert_eq!(
        engine.copy_entry(&src.join("dir"), &dst.join("dir"), true),
        Step::Done
    );
    assert!(names(&src).is_empty());
    assert_eq!(read(dst.join("dir/b.txt")), "bee");
}

#[test]
fn move_across_filesystems_when_available() {
    use std::os::unix::fs::MetadataExt;
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    let (_tmp, src, _) = fixture();
    let dev = |p: &Path| p.metadata().unwrap().dev();
    if dev(other.path()) == dev(&src) {
        return;
    }
    fs::write(other.path().join("a.txt"), b"old").unwrap();
    let mut script = Script::answering(&[Answer::Overwrite]);
    let report = run_script(
        &mv(&[src.join("a.txt"), src.join("dir")], other.path()),
        &mut script,
    );
    assert_eq!(report, Report::default());
    assert!(names(&src).is_empty());
    assert_eq!(read(other.path().join("a.txt")), "hello");
    assert_eq!(read(other.path().join("dir/deep/c.txt")), "c");
    assert_eq!(script.last.bytes_total, 9, "bytes that had to be copied");
}

#[test]
fn trash_reports_failures_and_continues() {
    let (_tmp, src, _) = fixture();
    fs::write(src.join("stuck"), b"").unwrap();
    let op = Operation::Trash {
        sources: vec![src.join("stuck"), src.join("a.txt"), src.join("dir")],
    };
    let mut script = Script::default();
    let report = run_script(&op, &mut script);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, src.join("stuck"));
    assert_eq!(names(&src), ["stuck"]);
    assert_eq!((script.last.items_done, script.last.items_total), (3, 3));
}

#[test]
fn temp_names_avoid_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("t");
    fs::write(tmp.path().join(".t.yagni-0.tmp"), b"").unwrap();
    let (temp, _) = create_temp(&target, create_new).unwrap();
    assert_eq!(temp, tmp.path().join(".t.yagni-1.tmp"));
    let err = create_temp(&target, |_| -> io::Result<()> {
        Err(io::ErrorKind::AlreadyExists.into())
    });
    assert_eq!(err.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    let err = create_temp(&target, |_| -> io::Result<()> {
        Err(io::ErrorKind::PermissionDenied.into())
    });
    assert_eq!(err.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
}

fn next(job: &Job) -> Event {
    job.events.recv_timeout(Duration::from_secs(10)).unwrap()
}

fn finish(job: &Job) -> Report {
    loop {
        if let Event::Finished(report) = next(job) {
            return report;
        }
    }
}

#[test]
fn job_runs_in_the_background_and_waits_for_answers() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let job = Job::spawn_with(
        copy(&[src.join("a.txt"), src.join("dir")], &dst),
        fake_trash,
    )
    .unwrap();
    let conflict = loop {
        match next(&job) {
            Event::Conflict(conflict) => break conflict,
            Event::Progress(_) => {}
            Event::Finished(report) => panic!("finished early: {report:?}"),
        }
    };
    assert_eq!(conflict.target, dst.join("a.txt"));
    assert!(job.try_event().is_none(), "waiting for the answer");
    job.answer(Answer::Overwrite);
    assert_eq!(finish(&job), Report::default());
    assert_eq!(read(dst.join("a.txt")), "hello");
}

#[test]
fn job_cancel_and_drop_stop_the_worker() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let op = copy(&[src.join("a.txt"), src.join("dir")], &dst);

    let job = Job::spawn_with(op, fake_trash).unwrap();
    job.cancel();
    job.answer(Answer::Cancel); // in case it got to the conflict first
    assert!(finish(&job).cancelled);
}

#[test]
fn job_dropped_while_asking_cancels() {
    // Dropping a Job closes the answer channel; the worker must not hang.
    let (sender, _events) = mpsc::channel();
    let (answers, answer_rx) = mpsc::channel::<Answer>();
    let mut observer = ChannelObserver {
        events: sender,
        answers: answer_rx,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(answers);
    let conflict = Conflict {
        source: PathBuf::new(),
        target: PathBuf::new(),
    };
    assert_eq!(observer.conflict(&conflict), Answer::Cancel);
}

#[test]
fn job_progress_is_throttled() {
    let (sender, events) = mpsc::channel();
    let (_answers, answer_rx) = mpsc::channel();
    let mut observer = ChannelObserver {
        events: sender,
        answers: answer_rx,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    for _ in 0..100 {
        observer.progress(&Progress::default());
    }
    assert_eq!(events.try_iter().count(), 1);
}

#[test]
fn job_conflict_without_a_listener_cancels() {
    let (sender, events) = mpsc::channel();
    let (_answers, answer_rx) = mpsc::channel();
    let mut observer = ChannelObserver {
        events: sender,
        answers: answer_rx,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(events);
    let conflict = Conflict {
        source: PathBuf::new(),
        target: PathBuf::new(),
    };
    assert_eq!(observer.conflict(&conflict), Answer::Cancel);
}

fn engine(script: &mut Script) -> Engine<'_> {
    Engine {
        observer: script,
        progress: Progress::default(),
        always: None,
        report: Report::default(),
    }
}

#[test]
fn symlink_can_overwrite_a_file() {
    let (_tmp, src, dst) = fixture();
    symlink("a.txt", src.join("link")).unwrap();
    fs::write(dst.join("link"), b"file").unwrap();
    let op = copy(&[src.join("link")], &dst);
    let report = run_script(&op, &mut Script::answering(&[Answer::Skip]));
    assert_eq!(report.skipped, 1);
    let report = run_script(&op, &mut Script::answering(&[Answer::Overwrite]));
    assert_eq!(report, Report::default());
    assert_eq!(fs::read_link(dst.join("link")).unwrap(), Path::new("a.txt"));
    assert_eq!(names(&dst), ["link"]);
}

#[test]
fn a_hard_link_to_the_source_is_never_overwritten() {
    let (_tmp, src, dst) = fixture();
    fs::create_dir(dst.join("dir")).unwrap();
    fs::hard_link(src.join("dir/b.txt"), dst.join("dir/b.txt")).unwrap();
    let report = run_script(&copy(&[src.join("dir")], &dst), &mut Script::default());
    assert_eq!(report.failures[0].message, "source and target are the same");
    assert_eq!(read(src.join("dir/b.txt")), "bee");
}

#[test]
fn cancel_between_entries_stops_copy_and_move() {
    let (_tmp, src, dst) = fixture();
    let deep = src.join("dir/deep");
    let mut script = Script {
        cancel_at_path: Some(deep.clone()),
        ..Script::default()
    };
    assert!(run_script(&copy(&[src.join("dir")], &dst), &mut script).cancelled);
    assert!(!dst.join("dir/deep/c.txt").exists());

    // A move merging into an existing directory, and a move by copy.
    fs::create_dir_all(dst.join("dir/deep")).unwrap();
    let mut script = Script {
        cancel_at_path: Some(deep.clone()),
        ..Script::default()
    };
    assert!(run_script(&mv(&[src.join("dir")], &dst), &mut script).cancelled);
    assert!(deep.join("c.txt").exists());
    let mut script = Script {
        cancel_at_path: Some(deep.clone()),
        ..Script::default()
    };
    let other = dst.join("other");
    assert_eq!(
        engine(&mut script).copy_entry(&src.join("dir"), &other, true),
        Step::Cancelled
    );
    assert!(deep.join("c.txt").exists());
}

#[test]
fn pre_agreed_replace_writes_over_the_target() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    let mut script = Script::default();
    let meta = src.join("a.txt").symlink_metadata().unwrap();
    let step = engine(&mut script).copy_leaf(&src.join("a.txt"), &dst.join("a.txt"), &meta, true);
    assert_eq!(step, Step::Done);
    assert_eq!(read(dst.join("a.txt")), "hello");
    assert!(script.conflicts.is_empty());
}

#[test]
fn move_by_copy_reports_an_original_that_cannot_be_deleted() {
    if is_root() {
        return;
    }
    let (_tmp, src, dst) = fixture();
    let locked = src.join("dir/deep");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let mut script = Script::default();
    let mut engine = engine(&mut script);
    let step = engine.copy_entry(&src.join("dir"), &dst.join("dir"), true);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(step, Step::Incomplete);
    let failure = &engine.report.failures[0];
    assert_eq!(failure.path, locked.join("c.txt"));
    assert!(
        failure
            .message
            .starts_with("copied, but cannot delete the original")
    );
    assert_eq!(read(dst.join("dir/deep/c.txt")), "c");
    assert!(!src.join("dir/b.txt").exists());
}

#[test]
fn move_merge_reports_unreadable_sources() {
    if is_root() {
        return;
    }
    let (_tmp, src, dst) = fixture();
    fs::create_dir_all(dst.join("dir/deep")).unwrap();
    fs::set_permissions(src.join("dir/deep"), fs::Permissions::from_mode(0o000)).unwrap();
    let report = run_script(&mv(&[src.join("dir")], &dst), &mut Script::default());
    fs::set_permissions(src.join("dir/deep"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, src.join("dir/deep"));
    assert_eq!(read(dst.join("dir/b.txt")), "bee", "the rest still moved");
}
