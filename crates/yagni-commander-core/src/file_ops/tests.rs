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

fn settings() -> Settings {
    Settings {
        trash: fake_trash,
        log: None,
    }
}

fn run_script(operation: &Operation, script: &mut Script) -> Report {
    run(operation, script, &settings())
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
        to: Destination::Into(to.to_path_buf()),
    }
}

fn mv(sources: &[PathBuf], to: &Path) -> Operation {
    Operation::Move {
        sources: sources.to_vec(),
        to: Destination::Into(to.to_path_buf()),
    }
}

fn copy_as(source: &Path, target: &Path) -> Operation {
    Operation::Copy {
        sources: vec![source.to_path_buf()],
        to: Destination::As(target.to_path_buf()),
    }
}

fn mv_as(source: &Path, target: &Path) -> Operation {
    Operation::Move {
        sources: vec![source.to_path_buf()],
        to: Destination::As(target.to_path_buf()),
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
    assert_eq!((p.files_done, p.files_total), (3, 3));
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
fn a_missing_destination_folder_is_created() {
    let (_tmp, src, dst) = fixture();
    let report = run_script(
        &copy(&[src.join("a.txt")], &dst.join("missing")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("missing/a.txt")), "hello");
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
    let mut engine = engine(&mut script);
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
    assert_eq!(script.last.files_total, 3);
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
    let job = Job::spawn(
        copy(&[src.join("a.txt"), src.join("dir")], &dst),
        settings(),
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

    let job = Job::spawn(op, settings()).unwrap();
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
        log: None,
        name: "test",
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

fn delete(sources: &[PathBuf]) -> Operation {
    Operation::Delete {
        sources: sources.to_vec(),
    }
}

#[test]
fn delete_removes_files_and_trees_with_progress() {
    let (_tmp, src, _) = fixture();
    let mut script = Script::default();
    let report = run_script(&delete(&[src.join("a.txt"), src.join("dir")]), &mut script);
    assert_eq!(report, Report::default());
    assert!(names(&src).is_empty());
    let p = &script.last;
    assert_eq!((p.items_done, p.items_total), (2, 2));
    assert_eq!((p.files_done, p.files_total), (3, 3));
    assert_eq!(p.bytes_total, 0, "deleting is counted by files");
}

#[test]
fn delete_removes_symlinks_but_never_their_targets() {
    let (tmp, src, dst) = fixture();
    fs::write(dst.join("precious"), b"keep").unwrap();
    symlink(&dst, src.join("dir/link-to-dir")).unwrap();
    symlink(dst.join("precious"), src.join("link-to-file")).unwrap();
    let report = run_script(
        &delete(&[src.join("dir"), src.join("link-to-file")]),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert!(!src.join("dir").exists());
    assert!(src.join("link-to-file").symlink_metadata().is_err());
    assert_eq!(read(dst.join("precious")), "keep");
    drop(tmp);
}

#[test]
fn opening_a_directory_for_deletion_never_follows_a_symlink() {
    // What stops a directory swapped for a symlink mid-delete.
    let (_tmp, src, dst) = fixture();
    symlink(&dst, src.join("swapped")).unwrap();
    let parent = open_dir(&src).unwrap();
    assert!(open_dir_at(&parent, std::ffi::OsStr::new("swapped")).is_err());
    assert!(open_dir_at(&parent, std::ffi::OsStr::new("dir")).is_ok());
}

#[test]
fn delete_reports_failures_and_continues() {
    let (_tmp, src, _) = fixture();
    let report = run_script(
        &delete(&[src.join("missing"), PathBuf::from("/"), src.join("a.txt")]),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 2);
    assert!(
        report.failures[0].message.contains("root"),
        "rejected first"
    );
    assert_eq!(report.failures[1].path, src.join("missing"));
    assert!(!src.join("a.txt").exists());
}

#[test]
fn delete_keeps_what_it_cannot_remove() {
    if is_root() {
        return;
    }
    let (_tmp, src, _) = fixture();
    let deep = src.join("dir/deep");
    fs::set_permissions(&deep, fs::Permissions::from_mode(0o555)).unwrap();
    let report = run_script(&delete(&[src.join("dir")]), &mut Script::default());
    fs::set_permissions(&deep, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, deep.join("c.txt"));
    assert!(deep.join("c.txt").exists());
    assert!(!src.join("dir/b.txt").exists(), "the rest was deleted");
}

#[test]
fn delete_can_be_cancelled_before_or_between_entries() {
    let (_tmp, src, _) = fixture();
    let mut script = Script {
        cancelled: true,
        ..Script::default()
    };
    assert!(run_script(&delete(&[src.join("dir")]), &mut script).cancelled);
    assert!(src.join("dir").exists());

    let mut script = Script {
        cancel_at_path: Some(src.join("dir/deep")),
        ..Script::default()
    };
    let report = run_script(&delete(&[src.join("dir"), src.join("a.txt")]), &mut script);
    assert!(report.cancelled);
    assert!(src.join("dir/deep/c.txt").exists());
    assert!(src.join("a.txt").exists(), "later sources untouched");
}

/// Log lines written by `operations`, without their timestamps.
fn logged(operations: &[(Operation, &[Answer])]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(crate::oplog::OperationLog::open(dir.path()).unwrap());
    for (operation, answers) in operations {
        let settings = Settings {
            trash: fake_trash,
            log: Some(log.clone()),
        };
        run(operation, &mut Script::answering(answers), &settings);
    }
    let file = fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    fs::read_to_string(file.path())
        .unwrap()
        .lines()
        .map(|line| line[20..].to_owned())
        .collect()
}

#[test]
fn every_file_touched_is_logged() {
    let (_tmp, src, dst) = fixture();
    fs::create_dir(dst.join("dir")).unwrap();
    fs::write(dst.join("dir/b.txt"), b"old").unwrap();
    let (s, d) = (src.display(), dst.display());
    let mut lines = logged(&[
        (
            copy(&[src.join("dir"), src.join("missing")], &dst),
            &[Answer::Skip],
        ),
        (mv(&[src.join("a.txt")], &dst), &[]),
        (delete(&[src.join("dir")]), &[]),
    ]);
    // Entries inside a directory come in the filesystem's order.
    let mut expected = [
        format!("copy start: 2 entries to {d}"),
        format!("copy source {s}/dir"),
        format!("copy source {s}/missing"),
        format!("copy skipped {s}/dir/b.txt -> {d}/dir/b.txt: exists"),
        format!("copy created directory {d}/dir/deep"),
        format!("copy copied {s}/dir/deep/c.txt -> {d}/dir/deep/c.txt"),
        format!("copy failed {s}/missing: No such file or directory (os error 2)"),
        "copy finished: 1 failed, 1 skipped".to_owned(),
        format!("move start: 1 entries to {d}"),
        format!("move source {s}/a.txt"),
        format!("move moved {s}/a.txt -> {d}/a.txt"),
        "move finished: 0 failed, 0 skipped".to_owned(),
        "delete start: 1 entries".to_owned(),
        format!("delete source {s}/dir"),
        format!("delete deleted {s}/dir/b.txt"),
        format!("delete deleted {s}/dir/deep/c.txt"),
        format!("delete deleted {s}/dir/deep"),
        format!("delete deleted {s}/dir"),
        "delete finished: 0 failed, 0 skipped".to_owned(),
    ];
    assert_eq!(lines.first(), expected.first());
    assert_eq!(lines.last(), expected.last());
    lines.sort();
    expected.sort();
    assert_eq!(lines, expected);
}

#[test]
fn overwrites_trash_and_moves_by_copy_are_logged() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("a.txt"), b"old").unwrap();
    fs::write(src.join("stuck"), b"").unwrap();
    let (s, d) = (src.display(), dst.display());
    let lines = logged(&[
        (copy(&[src.join("a.txt")], &dst), &[Answer::Overwrite]),
        (
            Operation::Trash {
                sources: vec![src.join("stuck"), src.join("a.txt")],
            },
            &[],
        ),
    ]);
    assert!(lines.contains(&format!("copy replacing {d}/a.txt")));
    assert!(lines.contains(&format!("copy copied {s}/a.txt -> {d}/a.txt")));
    assert!(lines.contains(&format!("trash failed {s}/stuck: cannot trash")));
    assert!(lines.contains(&format!("trash trashed {s}/a.txt")));
    assert!(lines.contains(&"trash finished: 1 failed, 0 skipped".to_owned()));

    // A move to another filesystem: copied, then the original removed.
    let dir = tempfile::tempdir().unwrap();
    let log = crate::oplog::OperationLog::open(dir.path()).unwrap();
    let mut script = Script::default();
    let mut engine = engine(&mut script);
    engine.log = Some(&log);
    engine.copy_entry(&src.join("dir"), &dst.join("moved"), true);
    let file = fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    let text = fs::read_to_string(file.path()).unwrap();
    assert!(text.contains(&format!("test removed {s}/dir/b.txt")));
    assert!(text.contains(&format!("test removed {s}/dir\n")));
}

// One item to an exact path (F5/F6 on a single entry).

#[test]
fn copy_as_gives_the_copy_a_new_name() {
    let (_tmp, src, dst) = fixture();
    let report = run_script(
        &copy_as(&src.join("a.txt"), &dst.join("b.txt")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("b.txt")), "hello");
    assert_eq!(names(&dst), ["b.txt"]);
}

#[test]
fn copy_as_works_for_a_folder() {
    let (_tmp, src, dst) = fixture();
    let report = run_script(
        &copy_as(&src.join("dir"), &dst.join("renamed")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("renamed/deep/c.txt")), "c");
}

#[test]
fn move_as_in_the_same_folder_renames() {
    let (_tmp, src, _dst) = fixture();
    let report = run_script(
        &mv_as(&src.join("a.txt"), &src.join("b.txt")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(src.join("b.txt")), "hello");
    assert!(!src.join("a.txt").exists());
}

#[test]
fn copy_as_in_the_same_folder_duplicates() {
    let (_tmp, src, _dst) = fixture();
    let report = run_script(
        &copy_as(&src.join("a.txt"), &src.join("a copy.txt")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(src.join("a copy.txt")), "hello");
    assert_eq!(read(src.join("a.txt")), "hello");
}

#[test]
fn missing_folders_are_created_and_logged() {
    let (_tmp, src, dst) = fixture();
    let d = dst.display();
    let lines = logged(&[
        (copy_as(&src.join("a.txt"), &dst.join("new/sub/x.txt")), &[]),
        (mv(&[src.join("dir")], &dst.join("other")), &[]),
    ]);
    assert_eq!(read(dst.join("new/sub/x.txt")), "hello");
    assert_eq!(read(dst.join("other/dir/b.txt")), "bee");
    for line in [
        format!("copy created directory {d}/new"),
        format!("copy created directory {d}/new/sub"),
        format!("move created directory {d}/other"),
    ] {
        assert!(lines.contains(&line), "{line} in {lines:#?}");
    }
}

#[test]
fn a_file_in_the_way_of_a_new_folder_fails() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("file"), b"x").unwrap();
    let report = run_script(
        &copy_as(&src.join("a.txt"), &dst.join("file/x.txt")),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1);
    assert!(src.join("a.txt").exists());
    let report = run_script(
        &copy(&[src.join("a.txt")], &dst.join("file")),
        &mut Script::default(),
    );
    assert_eq!(
        report.failures[0].message,
        "the destination is not a directory"
    );
}

#[test]
fn copy_as_onto_itself_is_refused() {
    let (_tmp, src, _dst) = fixture();
    let report = run_script(
        &copy_as(&src.join("a.txt"), &src.join("a.txt")),
        &mut Script::default(),
    );
    assert_eq!(report.failures[0].message, "source and target are the same");
    assert_eq!(read(src.join("a.txt")), "hello");
}

#[test]
fn a_folder_cannot_be_copied_as_something_inside_itself() {
    let (_tmp, src, _dst) = fixture();
    let report = run_script(
        &copy_as(&src.join("dir"), &src.join("dir/new/x")),
        &mut Script::default(),
    );
    assert_eq!(
        report.failures[0].message,
        "a directory cannot be put inside itself"
    );
    assert!(!src.join("dir/new").exists(), "no folder created");
}

#[test]
fn a_conflict_at_the_new_name_is_asked() {
    let (_tmp, src, dst) = fixture();
    fs::write(dst.join("b.txt"), b"old").unwrap();
    let mut script = Script::answering(&[Answer::Overwrite]);
    let report = run_script(
        &copy_as(&src.join("a.txt"), &dst.join("b.txt")),
        &mut script,
    );
    assert_eq!(report, Report::default());
    assert_eq!(script.conflicts[0].target, dst.join("b.txt"));
    assert_eq!(read(dst.join("b.txt")), "hello");
}
