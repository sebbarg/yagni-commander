use super::*;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::SystemTime;

use crate::test_archives::{T, make_tar, make_zip};

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
    /// Answers to link questions, in order.
    links: VecDeque<LinkAnswer>,
    link_questions: Vec<LinkQuestion>,
    /// Answers to password questions, in order.
    passwords: VecDeque<PasswordAnswer>,
    password_questions: Vec<PasswordQuestion>,
}

impl Script {
    fn answering(answers: &[Answer]) -> Self {
        Self {
            answers: answers.iter().copied().collect(),
            ..Self::default()
        }
    }
}

impl Script {
    fn linking(answers: &[LinkAnswer]) -> Self {
        Self {
            links: answers.iter().copied().collect(),
            ..Self::default()
        }
    }
}

impl Script {
    fn with_passwords(answers: &[PasswordAnswer]) -> Self {
        Self {
            passwords: answers.iter().cloned().collect(),
            ..Self::default()
        }
    }
}

fn pw(password: &str) -> PasswordAnswer {
    PasswordAnswer::Password(password.to_owned())
}

fn link(choice: LinkChoice) -> LinkAnswer {
    LinkAnswer {
        choice,
        for_all: false,
    }
}

fn pack_op(sources: &[PathBuf], base: &Path, to: &Path) -> Operation {
    Operation::Pack {
        sources: sources.to_vec(),
        base: base.to_path_buf(),
        to: to.to_path_buf(),
    }
}

fn extract_op(archives: &[PathBuf], into: &Path) -> Operation {
    Operation::Extract {
        archives: archives.to_vec(),
        into: into.to_path_buf(),
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

    fn password(&mut self, question: &PasswordQuestion) -> PasswordAnswer {
        self.password_questions.push(question.clone());
        self.passwords
            .pop_front()
            .expect("unexpected password question")
    }

    fn link(&mut self, question: &LinkQuestion) -> LinkAnswer {
        self.link_questions.push(question.clone());
        self.links.pop_front().expect("unexpected link question")
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
        mounts: None,
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
            target: dst.join("a.txt"),
            incoming: None,
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
            Event::Progress(_) | Event::Link(_) | Event::Password(_) => {}
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
        links: mpsc::channel().1,
        passwords: mpsc::channel().1,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(answers);
    let conflict = Conflict {
        source: PathBuf::new(),
        target: PathBuf::new(),
        incoming: None,
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
        links: mpsc::channel().1,
        passwords: mpsc::channel().1,
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
        links: mpsc::channel().1,
        passwords: mpsc::channel().1,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(events);
    let conflict = Conflict {
        source: PathBuf::new(),
        target: PathBuf::new(),
        incoming: None,
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
        link_choice: None,
        password: None,
        mounts: None,
        sync_copies: false,
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

    // A move merging into an existing directory, and a move by copy. The
    // copy above may have finished b.txt before cancelling (read_dir order
    // depends on the file system), so start the merge from an empty target.
    fs::remove_dir_all(dst.join("dir")).unwrap();
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

/// Runs `operation` with `mounts` as the mount table: the paths' real
/// paths, since the table lists those.
fn run_with_mounts(operation: &Operation, mounts: &[PathBuf], script: &mut Script) -> Report {
    let settings = Settings {
        mounts: Some(Mounts::table(
            mounts.iter().map(|p| p.canonicalize().unwrap()),
        )),
        ..settings()
    };
    run(operation, script, &settings)
}

#[test]
fn delete_leaves_a_mount_point_and_the_folders_above_it() {
    let (_tmp, src, _) = fixture();
    let deep = src.join("dir/deep");
    let mut script = Script::default();
    let report = run_with_mounts(
        &delete(&[src.join("dir"), src.join("a.txt")]),
        std::slice::from_ref(&deep),
        &mut script,
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].path, deep);
    assert_eq!(report.failures[0].message, MOUNT_POINT);
    assert!(deep.join("c.txt").exists(), "the mount's contents stay");
    assert_eq!(walk(&src), [deep.join("c.txt")], "everything else went");
    assert_eq!(script.last.files_total, 2, "the mount is not counted");
}

#[test]
fn delete_refuses_a_chosen_mount_point_even_through_a_link() {
    let (tmp, src, _) = fixture();
    // The panel's folder is reached through a link; the table has real paths.
    let via = tmp.path().join("via");
    symlink(&src, &via).unwrap();
    let report = run_with_mounts(
        &delete(&[via.join("dir")]),
        &[src.join("dir")],
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, via.join("dir"));
    assert!(src.join("dir/b.txt").exists());
    assert!(src.join("dir/deep/c.txt").exists());
}

#[test]
fn delete_without_a_mount_table_stops_at_another_device() {
    // macOS: no table, so a device of its own marks a mount point.
    let (_tmp, src, _) = fixture();
    let mut script = Script::default();
    let mut engine = engine(&mut script);
    engine.mounts = Some(Mounts::default());
    let dir = open_dir(&src).unwrap();
    let real = src.canonicalize().unwrap().join("dir");
    let other_device = dir_device(&dir).unwrap() + 1;
    let step = engine.delete_at(
        &dir,
        std::ffi::OsStr::new("dir"),
        &src.join("dir"),
        &real,
        other_device,
    );
    assert_eq!(step, Step::Incomplete);
    assert_eq!(engine.report.failures[0].message, MOUNT_POINT);
    assert!(src.join("dir/b.txt").exists());
}

#[test]
fn move_by_copy_leaves_a_mount_point() {
    let (_tmp, src, dst) = fixture();
    let deep = src.join("dir/deep");
    let mut script = Script::default();
    let mut engine = engine(&mut script);
    engine.mounts = Some(Mounts::table([deep.canonicalize().unwrap()]));
    engine.sync_copies = true;
    assert_eq!(
        engine.copy_entry(&src.join("dir"), &dst.join("dir"), true),
        Step::Incomplete
    );
    assert_eq!(engine.report.failures.len(), 1);
    assert_eq!(engine.report.failures[0].path, deep);
    assert_eq!(engine.report.failures[0].message, MOUNT_POINT);
    assert!(deep.join("c.txt").exists(), "the mount stays");
    assert!(!src.join("dir/b.txt").exists());
    assert_eq!(read(dst.join("dir/b.txt")), "bee", "the rest moved");
    assert!(!dst.join("dir/deep").exists());
}

#[test]
fn a_copy_goes_into_a_mount_point() {
    let (_tmp, src, dst) = fixture();
    let mut script = Script::default();
    let report = run_with_mounts(
        &copy(&[src.join("dir")], &dst),
        &[src.join("dir/deep")],
        &mut script,
    );
    assert_eq!(report, Report::default());
    assert_eq!(read(dst.join("dir/deep/c.txt")), "c");
    assert_eq!(script.last.files_total, 2);
}

/// Log lines written by `operations`, without their timestamps.
fn logged(operations: &[(Operation, &[Answer])]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(crate::oplog::OperationLog::open(dir.path()).unwrap());
    for (operation, answers) in operations {
        let settings = Settings {
            trash: fake_trash,
            log: Some(log.clone()),
            mounts: None,
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

#[test]
fn a_job_relays_link_questions() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    symlink("/bin/ls", src.join("ls")).unwrap();
    let job = Job::spawn(
        pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        settings(),
    )
    .unwrap();
    let question = loop {
        match next(&job) {
            Event::Link(question) => break question,
            Event::Finished(report) => panic!("finished without asking: {report:?}"),
            _ => {}
        }
    };
    assert_eq!(question.link, src.join("ls"));
    assert_eq!(question.shown, PathBuf::from("src/ls"));
    assert_eq!(question.target, PathBuf::from("/bin/ls"));
    assert_eq!(question.place, LinkPlace::Outside);
    job.answer_link(link(LinkChoice::Store));
    let report = finish(&job);
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(tmp.path().join("out.zip").is_file());
}

#[test]
fn a_dropped_job_answers_link_questions_with_cancel() {
    let (sender, _events) = mpsc::channel();
    let (_answers, answer_rx) = mpsc::channel();
    let (link_answers, link_rx) = mpsc::channel::<LinkAnswer>();
    let mut observer = ChannelObserver {
        events: sender,
        answers: answer_rx,
        links: link_rx,
        passwords: mpsc::channel().1,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(link_answers);
    let question = LinkQuestion {
        link: PathBuf::new(),
        shown: PathBuf::new(),
        target: PathBuf::new(),
        place: LinkPlace::Missing,
    };
    assert_eq!(observer.link(&question).choice, LinkChoice::Cancel);
}

/// A zip's entries: (name, kind, mode & 0o777, contents or link target), by name.
fn zip_listing(zip: &Path) -> Vec<(String, &'static str, u32, String)> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let kind = if entry.is_dir() {
            "dir"
        } else if entry.is_symlink() {
            "link"
        } else {
            "file"
        };
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        let mode = entry.unix_mode().unwrap_or(0) & 0o777;
        out.push((entry.name().to_owned(), kind, mode, text));
    }
    out.sort();
    out
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn temps_left(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| n.contains(".yagni-"))
        .collect()
}

#[test]
fn packs_files_and_folders_with_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    fs::create_dir_all(d.join("dir/empty")).unwrap();
    fs::write(d.join("dir/run.sh"), "#!/bin/sh\n").unwrap();
    fs::write(d.join("dir/.hidden"), "h").unwrap();
    fs::write(d.join("top.txt"), "t").unwrap();
    set_mode(&d.join("dir"), 0o755);
    set_mode(&d.join("dir/empty"), 0o700);
    set_mode(&d.join("dir/run.sh"), 0o755);
    set_mode(&d.join("dir/.hidden"), 0o600);
    set_mode(&d.join("top.txt"), 0o644);
    let mut script = Script::default();
    let report = run_script(
        &pack_op(&[d.join("dir"), d.join("top.txt")], d, &d.join("out.zip")),
        &mut script,
    );
    assert_eq!(report, Report::default());
    assert_eq!(
        zip_listing(&d.join("out.zip")),
        [
            ("dir/".into(), "dir", 0o755, String::new()),
            ("dir/.hidden".into(), "file", 0o600, "h".into()),
            ("dir/empty/".into(), "dir", 0o700, String::new()),
            ("dir/run.sh".into(), "file", 0o755, "#!/bin/sh\n".into()),
            ("top.txt".into(), "file", 0o644, "t".into()),
        ]
    );
    assert_eq!(script.last.files_done, 3);
    assert_eq!(script.last.files_total, 3);
    assert_eq!(script.last.bytes_done, script.last.bytes_total);
    assert_eq!(script.last.items_done, 2);
    assert!(temps_left(d).is_empty());
}

#[test]
fn links_follow_store_or_leave_out_as_answered() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let src = d.join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("real.txt"), "r").unwrap();
    fs::write(d.join("elsewhere.txt"), "e").unwrap();
    symlink("missing", src.join("gone")).unwrap();
    symlink("real.txt", src.join("inside")).unwrap();
    symlink("../elsewhere.txt", src.join("outside")).unwrap();
    // Walk order is by name: gone, inside, outside, real.txt.
    let mut script = Script::linking(&[
        link(LinkChoice::Follow),
        link(LinkChoice::Store),
        link(LinkChoice::LeaveOut),
    ]);
    let report = run_script(
        &pack_op(std::slice::from_ref(&src), d, &d.join("out.zip")),
        &mut script,
    );
    let places: Vec<_> = script.link_questions.iter().map(|q| q.place).collect();
    assert_eq!(
        places,
        [LinkPlace::Missing, LinkPlace::Inside, LinkPlace::Outside]
    );
    assert_eq!(script.link_questions[0].shown, PathBuf::from("src/gone"));
    assert_eq!(script.link_questions[1].target, PathBuf::from("real.txt"));
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].path, src.join("gone"));
    assert_eq!(
        report.failures[0].message,
        "the link's target does not exist"
    );
    assert_eq!(report.left_out, [src.join("outside")]);
    let listing: Vec<_> = zip_listing(&d.join("out.zip"))
        .into_iter()
        .map(|(name, kind, _, text)| (name, kind, text))
        .collect();
    assert_eq!(
        listing,
        [
            ("src/".into(), "dir", String::new()),
            ("src/inside".into(), "link", "real.txt".into()),
            ("src/real.txt".into(), "file", "r".into()),
        ]
    );
}

#[test]
fn same_for_the_remaining_links_asks_once() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("real.txt"), "r").unwrap();
    for name in ["a", "b", "c"] {
        symlink("real.txt", src.join(name)).unwrap();
    }
    let mut script = Script::linking(&[LinkAnswer {
        choice: LinkChoice::Store,
        for_all: true,
    }]);
    let report = run_script(
        &pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut script,
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(script.link_questions.len(), 1);
    let links = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .filter(|e| e.1 == "link")
        .count();
    assert_eq!(links, 3);
}

#[test]
fn a_followed_folder_link_is_packed_as_a_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    fs::create_dir_all(d.join("real")).unwrap();
    fs::write(d.join("real/x.txt"), "x").unwrap();
    fs::create_dir(d.join("src")).unwrap();
    symlink("../real", d.join("src/lib")).unwrap();
    let mut script = Script::linking(&[link(LinkChoice::Follow)]);
    let report = run_script(
        &pack_op(&[d.join("src")], d, &d.join("out.zip")),
        &mut script,
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&d.join("out.zip"))
        .into_iter()
        .map(|e| (e.0, e.1))
        .collect();
    assert_eq!(
        names,
        [
            ("src/".into(), "dir"),
            ("src/lib/".into(), "dir"),
            ("src/lib/x.txt".into(), "file"),
        ]
    );
}

#[test]
fn a_link_loop_is_reported_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    symlink(".", src.join("loop")).unwrap();
    let mut script = Script::linking(&[LinkAnswer {
        choice: LinkChoice::Follow,
        for_all: true,
    }]);
    let report = run_script(
        &pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut script,
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].path, src.join("loop"));
    assert_eq!(report.failures[0].message, "a symbolic link loop");
    assert!(tmp.path().join("out.zip").is_file());
}

#[test]
fn cancel_at_a_link_leaves_no_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    symlink("/bin/ls", src.join("ls")).unwrap();
    let mut script = Script::linking(&[link(LinkChoice::Cancel)]);
    let report = run_script(
        &pack_op(&[src], tmp.path(), &tmp.path().join("out.zip")),
        &mut script,
    );
    assert!(report.cancelled);
    assert!(!tmp.path().join("out.zip").exists());
    assert!(temps_left(tmp.path()).is_empty());
}

#[test]
fn cancelling_mid_file_leaves_no_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("big");
    fs::write(&big, vec![7u8; 10 << 20]).unwrap();
    let mut script = Script {
        cancel_at_bytes: Some(4 << 20),
        ..Script::default()
    };
    let report = run_script(
        &pack_op(&[big], tmp.path(), &tmp.path().join("out.zip")),
        &mut script,
    );
    assert!(report.cancelled);
    assert!(!tmp.path().join("out.zip").exists());
    assert!(temps_left(tmp.path()).is_empty());
}

#[test]
fn an_existing_zip_is_replaced_whole() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("out.zip"), "garbage").unwrap();
    fs::write(tmp.path().join("a.txt"), "a").unwrap();
    let report = run_script(
        &pack_op(
            &[tmp.path().join("a.txt")],
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["a.txt"]);
}

#[test]
fn special_files_are_not_packed() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    nix::unistd::mkfifo(
        &src.join("pipe"),
        nix::sys::stat::Mode::from_bits_truncate(0o644),
    )
    .unwrap();
    let report = run_script(
        &pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].path, src.join("pipe"));
    assert_eq!(
        report.failures[0].message,
        "only files, folders and symbolic links can be packed"
    );
    let names: Vec<_> = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["src/", "src/a.txt"]);
}

#[test]
fn the_zip_never_contains_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    let report = run_script(
        &pack_op(std::slice::from_ref(&src), tmp.path(), &src.join("out.zip")),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&src.join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["src/", "src/a.txt"]);
}

#[test]
fn non_utf8_names_are_packed_lossily() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join(std::ffi::OsStr::from_bytes(b"caf\xe9")), "c").unwrap();
    let report = run_script(
        &pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["src/", "src/caf\u{fffd}"]);
}

#[test]
fn missing_folders_for_the_zip_are_created() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "a").unwrap();
    let to = tmp.path().join("new/sub/out.zip");
    let report = run_script(
        &pack_op(&[tmp.path().join("a.txt")], tmp.path(), &to),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(to.is_file());
}

#[test]
fn packing_is_logged() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.txt"), "a").unwrap();
    let to = tmp.path().join("out.zip");
    let lines = logged(&[(pack_op(&[tmp.path().join("a.txt")], tmp.path(), &to), &[])]);
    assert!(
        lines.contains(&format!(
            "pack added {}",
            tmp.path().join("a.txt").display()
        )),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!("pack packed {} (1 entry)", to.display())),
        "{lines:?}"
    );
}

#[test]
fn one_top_folder_goes_straight_in() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("pkg-1.0.zip");
    make_zip(
        &zip,
        &[
            ("pkg-1.0/", ""),
            ("pkg-1.0/a.txt", "a"),
            ("pkg-1.0/sub/b.txt", "b"),
        ],
    );
    let out = tmp.path().join("out");
    let mut script = Script::default();
    let report = run_script(&extract_op(&[zip], &out), &mut script);
    assert_eq!(report, Report::default());
    assert_eq!(read(out.join("pkg-1.0/a.txt")), "a");
    assert_eq!(read(out.join("pkg-1.0/sub/b.txt")), "b");
    assert_eq!(names(&out), ["pkg-1.0"]);
    assert_eq!(script.last.files_done, 2);
    assert_eq!(script.last.files_total, 2);
    assert_eq!(script.last.bytes_done, 2);
}

#[test]
fn loose_entries_go_into_a_folder_named_after_the_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("photos.zip");
    make_zip(&zip, &[("a.jpg", "1"), ("b.jpg", "2")]);
    let out = tmp.path().join("out");
    run_script(&extract_op(&[zip], &out), &mut Script::default());
    assert_eq!(read(out.join("photos/a.jpg")), "1");
    assert_eq!(read(out.join("photos/b.jpg")), "2");
}

#[test]
fn escaping_names_fail_and_nothing_lands_outside() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("evil.zip");
    make_zip(
        &zip,
        &[
            ("../escaped", "x"),
            ("/abs", "x"),
            ("C:/x", "x"),
            ("a\\b", "x"),
            ("ok.txt", "fine"),
        ],
    );
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(std::slice::from_ref(&zip), &out),
        &mut Script::default(),
    );
    let messages: Vec<_> = report.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "“..” in an archive path is not allowed",
            "an absolute path in an archive is not allowed",
            "a drive letter in a zip is not allowed",
            "a backslash path in a zip is not allowed",
        ]
    );
    assert_eq!(
        report.failures[0].path,
        PathBuf::from(format!("{}: ../escaped", zip.display()))
    );
    assert!(!tmp.path().join("escaped").exists());
    assert_eq!(read(out.join("evil/ok.txt")), "fine");
}

#[test]
fn a_link_then_a_write_through_it_fails() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = tmp.path().join("trap.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.add_directory("d/", options).unwrap();
    zip.add_symlink("d/a", outside.path().display().to_string(), options)
        .unwrap();
    zip.start_file("d/a/passwd", options).unwrap();
    zip.write_all(b"pwned").unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(std::slice::from_ref(&path), &out),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].path,
        PathBuf::from(format!("{}: d/a/passwd", path.display()))
    );
    assert_eq!(
        report.failures[0].message,
        "a symbolic link or a file is in the way"
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(out.join("d/a").symlink_metadata().unwrap().is_symlink());
}

#[test]
fn an_existing_link_is_replaced_not_written_through() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("pkg")).unwrap();
    symlink(&outside, out.join("pkg/f")).unwrap();
    let zip = tmp.path().join("pkg.zip");
    make_zip(&zip, &[("pkg/f", "new")]);
    let report = run_script(
        &extract_op(&[zip], &out),
        &mut Script::answering(&[Answer::Overwrite]),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(&outside), "keep");
    assert!(out.join("pkg/f").symlink_metadata().unwrap().is_file());
    assert_eq!(read(out.join("pkg/f")), "new");
}

#[test]
fn modes_are_restored_without_special_bits() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("m.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("p/suid", options.unix_permissions(0o4755))
        .unwrap();
    zip.write_all(b"x").unwrap();
    zip.start_file("p/private", options.unix_permissions(0o600))
        .unwrap();
    zip.write_all(b"y").unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(mode(&out.join("p/suid")), 0o755);
    assert_eq!(mode(&out.join("p/private")), 0o600);
}

#[test]
fn times_are_restored() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let when = zip::DateTime::from_date_and_time(2020, 1, 2, 3, 4, 6).unwrap();
    zip.start_file(
        "p/a",
        zip::write::SimpleFileOptions::default().last_modified_time(when),
    )
    .unwrap();
    zip.write_all(b"a").unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    run_script(&extract_op(&[path], &out), &mut Script::default());
    let expected = jiff::civil::date(2020, 1, 2)
        .at(3, 4, 6, 0)
        .to_zoned(jiff::tz::TimeZone::system())
        .unwrap()
        .timestamp();
    assert_eq!(mtime(&out.join("p/a")), SystemTime::from(expected));
}

#[test]
fn existing_files_ask_and_skip_or_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("photos.zip");
    make_zip(&zip, &[("a.jpg", "1"), ("b.jpg", "22")]);
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("photos")).unwrap();
    fs::write(out.join("photos/b.jpg"), "old").unwrap();

    let mut script = Script::answering(&[Answer::Skip]);
    let report = run_script(&extract_op(std::slice::from_ref(&zip), &out), &mut script);
    assert_eq!(report.skipped, 1);
    assert_eq!(read(out.join("photos/b.jpg")), "old");
    assert_eq!(read(out.join("photos/a.jpg")), "1");
    let conflict = &script.conflicts[0];
    assert_eq!(conflict.target, out.join("photos/b.jpg"));
    assert_eq!(conflict.incoming.unwrap().size, 2);

    // a.jpg exists now too: Overwrite all.
    let report = run_script(
        &extract_op(&[zip], &out),
        &mut Script::answering(&[Answer::OverwriteAll]),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("photos/b.jpg")), "22");
    assert_eq!(
        names(&out.join("photos")),
        ["a.jpg", "b.jpg"],
        "no temp files left"
    );
}

#[test]
fn a_file_where_a_folder_goes_fails_and_the_reverse() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("pkg/f")).unwrap();
    fs::write(out.join("pkg/a"), "x").unwrap();
    let zip = tmp.path().join("pkg.zip");
    make_zip(
        &zip,
        &[
            ("pkg/", ""),
            ("pkg/a/", ""),
            ("pkg/a/b.txt", "b"),
            ("pkg/f", "f"),
            ("pkg/ok", "ok"),
        ],
    );
    let report = run_script(&extract_op(&[zip], &out), &mut Script::default());
    let messages: Vec<_> = report.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "a file with this name exists",
            "a symbolic link or a file is in the way",
            "a folder with this name exists",
        ]
    );
    assert_eq!(read(out.join("pkg/ok")), "ok");
    assert_eq!(read(out.join("pkg/a")), "x");
}

#[test]
fn a_corrupt_archive_fails_and_the_next_one_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.zip");
    fs::write(&bad, "not a zip").unwrap();
    let good = tmp.path().join("good.zip");
    make_zip(&good, &[("g/x", "x")]);
    let notes = tmp.path().join("notes.txt");
    fs::write(&notes, "n").unwrap();
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(&[bad.clone(), notes.clone(), good], &out),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 2, "{report:?}");
    assert_eq!(report.failures[0].path, bad);
    assert_eq!(report.failures[1].path, notes);
    assert_eq!(report.failures[1].message, "not an archive");
    assert_eq!(read(out.join("g/x")), "x");
}

#[test]
fn cancelling_mid_file_removes_only_that_file() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("p/a.txt", options).unwrap();
    zip.write_all(b"a").unwrap();
    zip.start_file("p/big", options).unwrap();
    zip.write_all(&vec![0u8; 10 << 20]).unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    let mut script = Script {
        cancel_at_bytes: Some(4 << 20),
        ..Script::default()
    };
    let report = run_script(&extract_op(&[path], &out), &mut script);
    assert!(report.cancelled);
    assert_eq!(read(out.join("p/a.txt")), "a");
    assert_eq!(names(&out.join("p")), ["a.txt"]);
}

#[test]
fn extraction_is_logged() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("photos.zip");
    make_zip(&zip, &[("a.jpg", "1")]);
    let out = tmp.path().join("out");
    let lines = logged(&[(extract_op(std::slice::from_ref(&zip), &out), &[])]);
    assert!(
        lines.contains(&format!(
            "extract created {}",
            out.join("photos/a.jpg").display()
        )),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!(
            "extract extracted {} -> {}",
            zip.display(),
            out.join("photos").display()
        )),
        "{lines:?}"
    );
}

#[test]
fn every_compression_round_trips() {
    for (format, ext) in [
        (Format::Tar, "tar"),
        (Format::TarGz, "tar.gz"),
        (Format::TarBz2, "tar.bz2"),
        (Format::TarXz, "tar.xz"),
        (Format::TarZst, "tar.zst"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("pkg.{ext}"));
        make_tar(
            &path,
            format,
            &[
                T::Dir("pkg/"),
                T::File("pkg/a.txt", "a", 0o644),
                T::File("pkg/bin/run", "#!", 0o755),
            ],
        );
        let out = tmp.path().join("out");
        let report = run_script(&extract_op(&[path], &out), &mut Script::default());
        assert_eq!(report, Report::default(), "{ext}");
        assert_eq!(read(out.join("pkg/a.txt")), "a", "{ext}");
        assert_eq!(mode(&out.join("pkg/bin/run")), 0o755, "{ext}");
        assert_eq!(
            mtime(&out.join("pkg/a.txt")),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000)
        );
    }
}

#[test]
fn multi_member_gzip_is_read_whole() {
    let tmp = tempfile::tempdir().unwrap();
    // Two gzip members back to back, like `cat a.gz b.gz`: one tar inside.
    let tar_path = tmp.path().join("plain.tar");
    make_tar(
        &tar_path,
        Format::Tar,
        &[T::File("p/a", "a", 0o644), T::File("p/b", "b", 0o644)],
    );
    let bytes = fs::read(&tar_path).unwrap();
    let (first, second) = bytes.split_at(512 * 2);
    let mut joined = Vec::new();
    for part in [first, second] {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(part).unwrap();
        joined.extend(encoder.finish().unwrap());
    }
    let path = tmp.path().join("p.tar.gz");
    fs::write(&path, joined).unwrap();
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("p/b")), "b");
}

#[test]
fn escaping_tar_names_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("evil.tar");
    make_tar(
        &path,
        Format::Tar,
        &[T::Raw(b"../x"), T::Raw(b"/abs"), T::File("ok", "ok", 0o644)],
    );
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(report.failures.len(), 2, "{report:?}");
    assert!(!tmp.path().join("x").exists());
    assert_eq!(read(out.join("evil/ok")), "ok");
}

#[test]
fn hard_links_only_to_files_already_extracted() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("h.tar");
    make_tar(
        &path,
        Format::Tar,
        &[
            T::File("p/a", "a", 0o644),
            T::Hard("p/b", b"p/a"),
            T::Hard("p/c", b"/etc/passwd"),
            T::Hard("p/d", b"p/later"),
            T::File("p/later", "l", 0o644),
        ],
    );
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    let messages: Vec<_> = report.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "an absolute path in an archive is not allowed",
            "a hard link to something outside this archive",
        ]
    );
    assert_eq!(fs::metadata(out.join("p/b")).unwrap().nlink(), 2);
    assert!(!out.join("p/c").exists());
    assert!(!out.join("p/d").exists());
}

#[test]
fn special_entries_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("s.tar");
    make_tar(
        &path,
        Format::Tar,
        &[T::Fifo("p/pipe"), T::File("p/a", "a", 0o644)],
    );
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].message,
        "special files (devices, pipes) are not extracted"
    );
    assert_eq!(read(out.join("p/a")), "a");
}

#[test]
fn setuid_is_dropped_and_links_are_made() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("m.tar.gz");
    make_tar(
        &path,
        Format::TarGz,
        &[T::File("p/suid", "x", 0o4755), T::Link("p/l", "suid")],
    );
    let out = tmp.path().join("out");
    run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(mode(&out.join("p/suid")), 0o755);
    assert_eq!(
        fs::read_link(out.join("p/l")).unwrap(),
        PathBuf::from("suid")
    );
}

#[test]
fn non_utf8_tar_names_are_kept() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("n.tar");
    make_tar(&path, Format::Tar, &[T::Raw(b"p/caf\xe9")]);
    let out = tmp.path().join("out");
    run_script(&extract_op(&[path], &out), &mut Script::default());
    assert!(
        out.join("p")
            .join(std::ffi::OsStr::from_bytes(b"caf\xe9"))
            .is_file()
    );
}

#[test]
fn a_truncated_tar_keeps_what_came_before() {
    let tmp = tempfile::tempdir().unwrap();
    let full = tmp.path().join("full.tar");
    let big = "x".repeat(1 << 20);
    make_tar(
        &full,
        Format::Tar,
        &[T::File("p/a", "a", 0o644), T::File("p/big", &big, 0o644)],
    );
    let bytes = fs::read(&full).unwrap();
    let path = tmp.path().join("cut.tar");
    fs::write(&path, &bytes[..600 * 1024]).unwrap();
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert!(!report.failures.is_empty());
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.message == "the archive ends early"),
        "{report:?}"
    );
    assert_eq!(read(out.join("p/a")), "a");
    assert!(
        !out.join("p/big").exists(),
        "the half-written file is removed"
    );
}

#[test]
fn smart_extraction_applies_to_tars() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("loose.tar.xz");
    make_tar(
        &path,
        Format::TarXz,
        &[T::File("a", "a", 0o644), T::File("b", "b", 0o644)],
    );
    let out = tmp.path().join("out");
    run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(read(out.join("loose/a")), "a");
}

#[test]
fn a_file_that_is_not_really_a_tar_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("fake.tar.gz");
    fs::write(&path, "not gzip").unwrap();
    let report = run_script(
        &extract_op(std::slice::from_ref(&path), &tmp.path().join("out")),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].path, path);
}

#[test]
fn a_link_entry_over_an_existing_entry_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("l.tar");
    make_tar(
        &path,
        Format::Tar,
        &[
            T::Link("p/l", "new"),
            T::Link("p/m", "x"),
            T::Link("p/d", "x"),
        ],
    );
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("p/d")).unwrap();
    fs::write(out.join("p/l"), "old").unwrap();
    fs::write(out.join("p/m"), "old").unwrap();
    let mut script = Script::answering(&[Answer::Overwrite, Answer::Skip]);
    let report = run_script(&extract_op(std::slice::from_ref(&path), &out), &mut script);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].message, "a folder with this name exists");
    assert_eq!(script.conflicts[0].incoming.unwrap().size, 3);
    assert_eq!(
        fs::read_link(out.join("p/l")).unwrap(),
        PathBuf::from("new")
    );
    assert_eq!(read(out.join("p/m")), "old");

    let report = run_script(
        &extract_op(&[path], &out),
        &mut Script::answering(&[Answer::Cancel]),
    );
    assert!(report.cancelled);
}

#[test]
fn a_hard_link_onto_an_existing_name_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("h.tar");
    make_tar(
        &path,
        Format::Tar,
        &[T::File("p/a", "a", 0o644), T::Hard("p/b", b"p/a")],
    );
    let out = tmp.path().join("out");
    fs::create_dir_all(out.join("p")).unwrap();
    fs::write(out.join("p/b"), "b").unwrap();
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].message, "an entry with this name exists");
    assert_eq!(read(out.join("p/b")), "b");
}

#[test]
fn cancel_between_archives_and_entries_stops() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("z.zip");
    make_zip(&zip, &[("p/a", "a"), ("p/b", "b")]);
    let tar = tmp.path().join("t.tar");
    make_tar(
        &tar,
        Format::Tar,
        &[T::File("q/a", "a", 0o644), T::File("q/b", "b", 0o644)],
    );
    let out = tmp.path().join("out");
    for archive in [&zip, &tar] {
        // Cancelled on the report naming the archive: before its first entry.
        let mut script = Script {
            cancel_at_path: Some(archive.clone()),
            ..Script::default()
        };
        let report = run_script(
            &extract_op(std::slice::from_ref(archive), &out),
            &mut script,
        );
        assert!(report.cancelled, "{archive:?}");
    }
    assert!(!out.join("p/a").exists());
    assert!(!out.join("q/a").exists());
    // A conflict answered with Cancel stops a zip too.
    make_zip(&zip, &[("p/a", "a")]);
    fs::create_dir_all(out.join("p")).unwrap();
    fs::write(out.join("p/a"), "old").unwrap();
    let report = run_script(
        &extract_op(&[zip], &out),
        &mut Script::answering(&[Answer::Cancel]),
    );
    assert!(report.cancelled);
    assert_eq!(read(out.join("p/a")), "old");
}

#[test]
fn a_source_outside_the_base_is_packed_under_its_name() {
    let tmp = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("x.txt"), "x").unwrap();
    let report = run_script(
        &pack_op(
            &[other.path().join("x.txt")],
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["x.txt"]);
}

#[test]
fn a_cancelled_pack_stops_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    let mut script = Script {
        cancelled: true,
        ..Script::default()
    };
    let report = run_script(
        &pack_op(&[src], tmp.path(), &tmp.path().join("out.zip")),
        &mut script,
    );
    assert!(report.cancelled);
    assert!(!tmp.path().join("out.zip").exists());
    assert!(temps_left(tmp.path()).is_empty());
}

#[test]
fn an_archive_named_dots_stays_inside_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("...zip");
    make_zip(&zip, &[("a.txt", "a"), ("b.txt", "b")]);
    let out = tmp.path().join("target");
    let report = run_script(&extract_op(&[zip], &out), &mut Script::default());
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(!tmp.path().join("a.txt").exists(), "nothing in the parent");
    assert_eq!(read(out.join("archive/a.txt")), "a");
}

#[test]
fn names_that_collide_in_the_zip_keep_the_first() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    // Both become "caf\u{fffd}" in the zip.
    fs::write(src.join(std::ffi::OsStr::from_bytes(b"caf\xe8")), "1").unwrap();
    fs::write(src.join(std::ffi::OsStr::from_bytes(b"caf\xe9")), "2").unwrap();
    let report = run_script(
        &pack_op(
            std::slice::from_ref(&src),
            tmp.path(),
            &tmp.path().join("out.zip"),
        ),
        &mut Script::default(),
    );
    assert_eq!(report.failures.len(), 1, "{report:?}");
    let names: Vec<_> = zip_listing(&tmp.path().join("out.zip"))
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(names, ["src/", "src/caf\u{fffd}"]);
}

#[test]
fn an_old_zip_inside_the_sources_is_not_packed_into_its_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a.txt"), "a").unwrap();
    let to = src.join("out.zip");
    make_zip(&to, &[("old", "o")]);
    let report = run_script(
        &pack_op(std::slice::from_ref(&src), tmp.path(), &to),
        &mut Script::default(),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    let names: Vec<_> = zip_listing(&to).into_iter().map(|e| e.0).collect();
    assert_eq!(names, ["src/", "src/a.txt"]);
}

#[test]
fn pax_global_headers_are_not_entries() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("proj.tar.gz");
    let gz =
        flate2::write::GzEncoder::new(File::create(&path).unwrap(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    // What `git archive` writes first.
    let record = b"52 comment=0123456789012345678901234567890123456789\n";
    let mut header = tar::Header::new_ustar();
    header.set_path("pax_global_header").unwrap();
    header.set_entry_type(tar::EntryType::XGlobalHeader);
    header.set_size(record.len() as u64);
    header.set_mode(0o666);
    header.set_cksum();
    builder.append(&header, &record[..]).unwrap();
    let mut header = tar::Header::new_ustar();
    header.set_path("proj/a.txt").unwrap();
    header.set_size(1);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, &b"a"[..]).unwrap();
    builder
        .into_inner()
        .unwrap()
        .finish()
        .unwrap()
        .flush()
        .unwrap();
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("proj/a.txt")), "a");
    assert_eq!(names(&out), ["proj"]);
}

#[test]
fn an_overlong_zip_link_target_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("l.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.add_symlink("p/l", "x".repeat(5000), options).unwrap();
    zip.add_symlink("p/ok", "target", options).unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    let report = run_script(&extract_op(&[path], &out), &mut Script::default());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].message,
        "a link target that long is not allowed"
    );
    assert!(out.join("p/l").symlink_metadata().is_err());
    assert_eq!(
        fs::read_link(out.join("p/ok")).unwrap(),
        PathBuf::from("target")
    );
}

#[test]
fn a_dropped_job_answers_password_questions_with_cancel() {
    let (sender, _events) = mpsc::channel();
    let (_answers, answer_rx) = mpsc::channel();
    let (password_answers, password_rx) = mpsc::channel::<PasswordAnswer>();
    let mut observer = ChannelObserver {
        events: sender,
        answers: answer_rx,
        links: mpsc::channel().1,
        passwords: password_rx,
        cancel: Arc::new(AtomicBool::new(false)),
        last_progress: None,
    };
    drop(password_answers);
    let question = PasswordQuestion {
        archive: PathBuf::from("/a.zip"),
        retry: false,
    };
    assert_eq!(observer.password(&question), PasswordAnswer::Cancel);
}

#[test]
fn a_password_answer_never_prints_the_password() {
    let shown = format!("{:?}", pw("hunter2"));
    assert!(!shown.contains("hunter2"), "{shown}");
    assert_eq!(format!("{:?}", PasswordAnswer::Skip), "Skip");
}

/// A zip of `entries` (name, contents, encrypted) whose encrypted entries
/// use `password`, AES-256 with `aes`, else ZipCrypto; stored, with a fixed
/// time. Both kinds start with random bytes, so a wrong password passes
/// the quick check by chance (ZipCrypto: 1 in 256): the zip is written
/// again until every password in `rejects` is rejected by every encrypted
/// entry, so tests that need a wrong password never flake.
fn make_locked_zip(
    path: &Path,
    password: &str,
    aes: bool,
    entries: &[(&str, &str, bool)],
    rejects: &[&str],
) {
    use std::io::Write;
    use zip::unstable::write::FileOptionsExt;
    for _ in 0..1000 {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, contents, encrypted) in entries {
            let plain = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .last_modified_time(
                    zip::DateTime::from_date_and_time(2020, 1, 2, 3, 4, 6).unwrap(),
                );
            let options = match (encrypted, aes) {
                (false, _) => plain,
                (true, true) => plain.with_aes_encryption(zip::AesMode::Aes256, password),
                (true, false) => plain
                    .with_deprecated_encryption(password.as_bytes())
                    .unwrap(),
            };
            zip.start_file(*name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let all_rejected = (0..archive.len()).all(|i| {
            !entries[i].2
                || rejects.iter().all(|wrong| {
                    matches!(
                        archive.by_index_decrypt(i, wrong.as_bytes()),
                        Err(zip::result::ZipError::InvalidPassword)
                    )
                })
        });
        if all_rejected {
            return;
        }
    }
    panic!("no zip rejecting {rejects:?}");
}

fn question(archive: &Path, retry: bool) -> PasswordQuestion {
    PasswordQuestion {
        archive: archive.to_path_buf(),
        retry,
    }
}

#[test]
fn each_kind_extracts_with_its_password() {
    for aes in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("s.zip");
        make_locked_zip(&zip, "pw", aes, &[("p/a.txt", "secret", true)], &[]);
        let out = tmp.path().join("out");
        let mut script = Script::with_passwords(&[pw("pw")]);
        let report = run_script(&extract_op(std::slice::from_ref(&zip), &out), &mut script);
        assert_eq!(report, Report::default(), "aes {aes}");
        assert_eq!(read(out.join("p/a.txt")), "secret", "aes {aes}");
        assert_eq!(script.password_questions, [question(&zip, false)]);
    }
}

#[test]
fn a_wrong_password_asks_again() {
    for aes in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("s.zip");
        make_locked_zip(&zip, "pw", aes, &[("p/a.txt", "secret", true)], &["nope"]);
        let out = tmp.path().join("out");
        let mut script = Script::with_passwords(&[pw("nope"), pw("pw")]);
        let report = run_script(&extract_op(std::slice::from_ref(&zip), &out), &mut script);
        assert!(report.failures.is_empty(), "{report:?}");
        assert_eq!(
            script.password_questions,
            [question(&zip, false), question(&zip, true)]
        );
        assert_eq!(read(out.join("p/a.txt")), "secret");
    }
}

#[test]
fn an_empty_password_is_tried_and_asks_again() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(&zip, "pw", true, &[("p/a.txt", "secret", true)], &[""]);
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[pw(""), pw("pw")]);
    run_script(&extract_op(std::slice::from_ref(&zip), &out), &mut script);
    assert_eq!(
        script.password_questions,
        [question(&zip, false), question(&zip, true)]
    );
    assert_eq!(read(out.join("p/a.txt")), "secret");
}

#[test]
fn skip_leaves_out_the_encrypted_entries_only() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(
        &zip,
        "pw",
        false,
        &[
            ("p/plain.txt", "x", false),
            ("p/a", "a", true),
            ("p/b", "b", true),
        ],
        &[],
    );
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[PasswordAnswer::Skip]);
    let report = run_script(&extract_op(&[zip], &out), &mut script);
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(report.skipped, 2);
    assert_eq!(script.password_questions.len(), 1, "asked once per archive");
    assert_eq!(names(&out.join("p")), ["plain.txt"]);
    assert_eq!(script.last.files_done, 3, "skipped entries count as done");
}

#[test]
fn cancel_at_the_password_stops_the_job() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(
        &zip,
        "pw",
        false,
        &[("p/a", "a", true), ("p/b", "b", false)],
        &[],
    );
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(&[zip], &out),
        &mut Script::with_passwords(&[PasswordAnswer::Cancel]),
    );
    assert!(report.cancelled);
    assert!(!out.join("p/a").exists());
    assert!(!out.join("p/b").exists());
}

#[test]
fn the_password_is_remembered_for_the_next_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let one = tmp.path().join("one.zip");
    let two = tmp.path().join("two.zip");
    make_locked_zip(
        &one,
        "pw",
        true,
        &[("1/a", "a", true), ("1/b", "b", true)],
        &[],
    );
    make_locked_zip(&two, "pw", false, &[("2/c", "c", true)], &[]);
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[pw("pw")]);
    let report = run_script(&extract_op(&[one.clone(), two], &out), &mut script);
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(script.password_questions, [question(&one, false)]);
    assert_eq!(read(out.join("2/c")), "c");
}

#[test]
fn a_second_archive_with_another_password_asks_again() {
    let tmp = tempfile::tempdir().unwrap();
    let one = tmp.path().join("one.zip");
    let two = tmp.path().join("two.zip");
    make_locked_zip(&one, "pw", true, &[("1/a", "a", true)], &[]);
    make_locked_zip(&two, "other", true, &[("2/c", "c", true)], &["pw"]);
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[pw("pw"), pw("other")]);
    let report = run_script(&extract_op(&[one.clone(), two.clone()], &out), &mut script);
    assert!(report.failures.is_empty(), "{report:?}");
    // Nothing was typed wrong for two.zip: not a retry.
    assert_eq!(
        script.password_questions,
        [question(&one, false), question(&two, false)]
    );
    assert_eq!(read(out.join("2/c")), "c");
}

#[test]
fn a_plain_zip_never_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("plain.zip");
    make_zip(&zip, &[("p/a", "a")]);
    // Script::default() panics on any password question.
    let report = run_script(
        &extract_op(&[zip], &tmp.path().join("out")),
        &mut Script::default(),
    );
    assert_eq!(report, Report::default());
}

#[test]
fn a_wrongly_accepted_password_fails_the_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(
        &zip,
        "pw",
        false,
        &[
            ("p/a", "secret data that fails its check", true),
            ("p/b", "b", false),
        ],
        &[],
    );
    // A wrong password that passes ZipCrypto's 1-byte check (1 in 256).
    let mut archive = zip::ZipArchive::new(File::open(&zip).unwrap()).unwrap();
    let lucky = (0..100_000)
        .map(|n| format!("w{n}"))
        .find(|wrong| archive.by_index_decrypt(0, wrong.as_bytes()).is_ok())
        .expect("a password passing the check");
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[pw(&lucky)]);
    let report = run_script(&extract_op(std::slice::from_ref(&zip), &out), &mut script);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].message,
        "wrong password or a damaged entry"
    );
    assert_eq!(
        report.failures[0].path,
        PathBuf::from(format!("{}: p/a", zip.display()))
    );
    assert!(
        !out.join("p/a").exists(),
        "the half-written file is removed"
    );
    assert_eq!(read(out.join("p/b")), "b");
    assert_eq!(script.password_questions.len(), 1, "no new question");
}

#[test]
fn a_job_relays_password_questions() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(&zip, "pw", true, &[("p/a", "a", true)], &[]);
    let out = tmp.path().join("out");
    let job = Job::spawn(extract_op(std::slice::from_ref(&zip), &out), settings()).unwrap();
    let asked = loop {
        match next(&job) {
            Event::Password(asked) => break asked,
            Event::Finished(report) => panic!("finished without asking: {report:?}"),
            _ => {}
        }
    };
    assert_eq!(asked, question(&zip, false));
    job.answer_password(pw("pw"));
    let report = finish(&job);
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("p/a")), "a");
}

#[test]
fn the_password_is_never_logged() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_locked_zip(
        &zip,
        "pw-secret-123",
        true,
        &[("p/a", "a", true)],
        &["wrong-456"],
    );
    let out = tmp.path().join("out");
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(crate::oplog::OperationLog::open(dir.path()).unwrap());
    let settings = Settings {
        trash: fake_trash,
        log: Some(log),
        mounts: None,
    };
    let mut script = Script::with_passwords(&[pw("wrong-456"), pw("pw-secret-123")]);
    let report = run(&extract_op(&[zip], &out), &mut script, &settings);
    assert!(report.failures.is_empty(), "{report:?}");
    let file = fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap();
    let text = fs::read_to_string(file.path()).unwrap();
    assert!(text.contains("created"), "{text}");
    assert!(
        !text.contains("pw-secret-123") && !text.contains("wrong-456"),
        "{text}"
    );
}

/// Finds `needle` from `from` on.
fn find(bytes: &[u8], needle: &[u8], from: usize) -> usize {
    from + bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap()
}

/// An AES (AE-2, so no CRC) deflated entry `p/a` holding "hello", whose
/// encrypted data runs on past the end of the deflate stream and whose
/// authentication code doesn't match (with `damaged`). Decompression stops before the end
/// of the encrypted data, so zip never checks the code: the way a wrong
/// password that passed the 2-byte check would look.
fn make_unauthenticated_zip(path: &Path, damaged: bool) {
    use std::io::Write;
    let mut deflate =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    deflate.write_all(b"hello").unwrap();
    let mut data = deflate.finish().unwrap();
    let stream_len = data.len();
    data.extend(vec![0u8; 64 << 10]);
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption(zip::AesMode::Aes256, "pw");
    zip.start_file("p/a", options).unwrap();
    zip.write_all(&data).unwrap();
    zip.finish().unwrap();

    let mut bytes = fs::read(path).unwrap();
    let local = find(&bytes, b"PK\x03\x04", 0);
    let central = find(&bytes, b"PK\x01\x02", local + 4);
    for (header, crc, size) in [(local, 14, 22), (central, 16, 24)] {
        bytes[header + crc..header + crc + 4].copy_from_slice(&0u32.to_le_bytes());
        bytes[header + size..header + size + 4].copy_from_slice(&5u32.to_le_bytes());
        // The AES extra field: id 0x9901, size 7, vendor version, "AE",
        // strength, the real compression method.
        let extra = find(&bytes, b"\x01\x99\x07\x00", header);
        bytes[extra + 4..extra + 6].copy_from_slice(&2u16.to_le_bytes());
        bytes[extra + 9..extra + 11].copy_from_slice(&8u16.to_le_bytes());
    }
    // Damage the encrypted data well after the deflate stream.
    let name_len = u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
    let data_start = local + 30 + name_len + extra_len + 16 + 2;
    if damaged {
        bytes[data_start + stream_len + 40_000] ^= 0xff;
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn an_unauthenticated_aes_entry_fails_instead_of_keeping_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_unauthenticated_zip(&zip, true);
    let out = tmp.path().join("out");
    let mut script = Script::with_passwords(&[pw("pw")]);
    let report = run_script(&extract_op(&[zip], &out), &mut script);
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(
        report.failures[0].message,
        "wrong password or a damaged entry"
    );
    assert!(!out.join("p/a").exists());
}

#[test]
fn an_authentic_deflated_aes_entry_passes_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("s.zip");
    make_unauthenticated_zip(&zip, false);
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(&[zip], &out),
        &mut Script::with_passwords(&[pw("pw")]),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("p/a")), "hello");
}

#[test]
fn deflated_aes_entries_of_both_versions_extract() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("s.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .with_aes_encryption(zip::AesMode::Aes128, "pw");
    // zip writes AE-2 (no CRC) below 20 bytes, AE-1 from there.
    zip.start_file("p/small", options).unwrap();
    zip.write_all(b"tiny").unwrap();
    zip.start_file("p/big", options).unwrap();
    zip.write_all(&b"0123456789".repeat(1000)).unwrap();
    zip.finish().unwrap();
    let out = tmp.path().join("out");
    let report = run_script(
        &extract_op(&[path], &out),
        &mut Script::with_passwords(&[pw("pw")]),
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(out.join("p/small")), "tiny");
    assert_eq!(read(out.join("p/big")).len(), 10_000);
}

#[test]
fn tar_listing_is_strict_and_carries_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.tar");
    make_tar(
        &path,
        Format::Tar,
        &[T::Dir("d/"), T::File("d/x", "xy", 0o644)],
    );
    let mut seen = 0;
    let listed = extract::read_tar_list(&path, Format::Tar, || {
        seen += 1;
        true
    })
    .unwrap();
    assert_eq!(seen, 2);
    assert_eq!(listed[1].size, 2);
    assert_eq!(listed[1].owner.as_deref(), Some("me:staff"));

    let bad = tmp.path().join("bad.tar.gz");
    fs::write(&bad, b"not gzip").unwrap();
    assert!(extract::read_tar_list(&bad, Format::TarGz, || true).is_err());
}

#[test]
fn zip_listing_counts_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.zip");
    make_zip(&path, &[("d/", ""), ("d/x", "xy")]);
    let mut zip = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let mut seen = 0;
    let listed = extract::list_zip(&mut zip, || {
        seen += 1;
        true
    })
    .unwrap();
    assert_eq!((seen, listed.len()), (2, 2));
    assert!(listed[0].owner.is_none());
}

fn entries_op(archive: &Path, inner: &[&str], names: &[&str], to: Destination) -> Operation {
    Operation::ExtractEntries {
        archive: archive.to_path_buf(),
        inner: inner.iter().map(OsString::from).collect(),
        names: names.iter().map(OsString::from).collect(),
        to,
    }
}

/// `pkg.zip` with `src/lib/{a,b}.rs`, `src/main.rs`, `lib/other.rs` and
/// `README` (no folder entries).
fn source_zip(dir: &Path) -> PathBuf {
    let zip = dir.join("pkg.zip");
    make_zip(
        &zip,
        &[
            ("src/lib/a.rs", "a"),
            ("src/lib/b.rs", "b"),
            ("src/main.rs", "m"),
            ("lib/other.rs", "o"),
            ("README", "r"),
        ],
    );
    zip
}

#[test]
fn extract_entries_copies_the_picked_trees_only() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = source_zip(tmp.path());
    let out = tmp.path().join("out");
    let mut script = Script::default();
    let op = entries_op(
        &zip,
        &["src"],
        &["lib", "main.rs"],
        Destination::Into(out.clone()),
    );
    let report = run_script(&op, &mut script);
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(names(&out), ["lib", "main.rs"]);
    assert_eq!(names(&out.join("lib")), ["a.rs", "b.rs"]);
    assert_eq!(read(out.join("lib/a.rs")), "a");
    assert_eq!(script.last.files_total, 3, "only the picked files count");
    assert!(!out.join("lib/other.rs").exists(), "not the root's lib");
}

#[test]
fn extract_entries_as_renames_the_one_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = source_zip(tmp.path());
    let target = tmp.path().join("new/dir/renamed.rs");
    let op = entries_op(
        &zip,
        &["src"],
        &["main.rs"],
        Destination::As(target.clone()),
    );
    let report = run_script(&op, &mut Script::default());
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(read(&target), "m");
    assert_eq!(names(&tmp.path().join("new/dir")), ["renamed.rs"]);

    let folder = tmp.path().join("copy");
    let op = entries_op(&zip, &[], &["src"], Destination::As(folder.clone()));
    run_script(&op, &mut Script::default());
    assert_eq!(read(folder.join("lib/a.rs")), "a");
    assert_eq!(names(&folder), ["lib", "main.rs"]);
}

#[test]
fn extract_entries_as_needs_one_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = source_zip(tmp.path());
    let target = tmp.path().join("x");
    let op = entries_op(
        &zip,
        &[],
        &["src", "README"],
        Destination::As(target.clone()),
    );
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report.failures.len(), 1);
    assert!(!target.exists());
}

#[test]
fn extract_entries_from_a_tar_asks_on_conflicts_and_cancels() {
    let tmp = tempfile::tempdir().unwrap();
    let tar = tmp.path().join("pkg.tar.xz");
    make_tar(
        &tar,
        Format::TarXz,
        &[
            T::Dir("d/"),
            T::File("d/x", "new", 0o644),
            T::File("d/y", "y", 0o644),
        ],
    );
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("x"), b"old").unwrap();
    let op = entries_op(&tar, &["d"], &["x", "y"], Destination::Into(out.clone()));
    let mut script = Script::answering(&[Answer::Skip]);
    let report = run_script(&op, &mut script);
    assert_eq!(report.skipped, 1);
    assert_eq!(
        (read(out.join("x")), read(out.join("y"))),
        ("old".into(), "y".into())
    );

    fs::remove_file(out.join("y")).unwrap();
    let mut script = Script::answering(&[Answer::Cancel]);
    let report = run_script(&op, &mut script);
    assert!(report.cancelled);
    assert!(!out.join("y").exists(), "stopped at the conflict");
}

#[test]
fn a_hard_link_to_an_entry_not_picked_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let tar = tmp.path().join("h.tar");
    make_tar(
        &tar,
        Format::Tar,
        &[T::File("x", "1", 0o644), T::Hard("y", b"x")],
    );
    let out = tmp.path().join("out");
    let op = entries_op(&tar, &[], &["y"], Destination::Into(out.clone()));
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert!(report.failures[0].message.contains("not copied"));
    assert!(!out.join("x").exists() && !out.join("y").exists());

    let op = entries_op(&tar, &[], &["x", "y"], Destination::Into(out.clone()));
    let report = run_script(&op, &mut Script::default());
    assert!(report.failures.is_empty(), "both picked: {report:?}");
    assert_eq!(read(out.join("y")), "1");
}

#[test]
fn only_picked_encrypted_entries_ask_for_the_password() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = tmp.path().join("locked.zip");
    make_locked_zip(
        &zip,
        "pw",
        false,
        &[("open.txt", "o", false), ("secret.txt", "s", true)],
        &[],
    );
    let out = tmp.path().join("out");
    let op = entries_op(&zip, &[], &["open.txt"], Destination::Into(out.clone()));
    let mut script = Script::default();
    run_script(&op, &mut script);
    assert!(script.password_questions.is_empty());
    assert_eq!(read(out.join("open.txt")), "o");

    let op = entries_op(&zip, &[], &["secret.txt"], Destination::Into(out.clone()));
    let mut script = Script {
        passwords: [pw("pw")].into(),
        ..Script::default()
    };
    run_script(&op, &mut script);
    assert_eq!(read(out.join("secret.txt")), "s");
}

#[test]
fn unsafe_names_are_never_picked() {
    let tmp = tempfile::tempdir().unwrap();
    let tar = tmp.path().join("evil.tar");
    make_tar(
        &tar,
        Format::Tar,
        &[T::Raw(b"../x"), T::File("ok", "1", 0o644)],
    );
    let out = tmp.path().join("out");
    let op = entries_op(&tar, &[], &["ok"], Destination::Into(out.clone()));
    let report = run_script(&op, &mut Script::default());
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(names(&out), ["ok"]);
    assert!(!tmp.path().join("x").exists());
}

#[test]
fn a_picked_name_no_longer_in_the_archive_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = source_zip(tmp.path());
    let out = tmp.path().join("out");
    let op = entries_op(
        &zip,
        &["src"],
        &["gone.rs", "main.rs"],
        Destination::Into(out.clone()),
    );
    let report = run_script(&op, &mut Script::default());
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert!(
        report.failures[0]
            .message
            .contains("no longer in the archive")
    );
    assert!(
        report.failures[0]
            .path
            .display()
            .to_string()
            .ends_with("src/gone.rs")
    );
    assert_eq!(names(&out), ["main.rs"], "the rest still copied");
}

#[test]
fn a_picked_entry_stored_twice_arrives_once_as_the_last_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let tar = tmp.path().join("twice.tar");
    make_tar(
        &tar,
        Format::Tar,
        &[T::File("x", "old", 0o644), T::File("x", "new", 0o644)],
    );
    let target = tmp.path().join("copy/x");
    let op = entries_op(&tar, &[], &["x"], Destination::As(target.clone()));
    let mut script = Script::default(); // no conflict answers: none asked
    let report = run_script(&op, &mut script);
    assert!(report.failures.is_empty(), "{report:?}");
    assert!(script.conflicts.is_empty());
    assert_eq!(read(&target), "new");
    assert_eq!(script.last.files_total, 1);
}
