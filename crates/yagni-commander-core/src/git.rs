//! The Git branch in a panel's path header (see `SPEC.md`, Git): the
//! repository a folder is in ([`find_root`]) and its branch, its distance
//! from the upstream and whether anything changed ([`status`]).
//!
//! Runs the system's `git` (like `ssh` for remote folders: no library, no
//! `-sys` crate). A repository's own config is read by git, so a hostile
//! one (an unpacked archive with a `.git` folder) could name a program;
//! `core.fsmonitor` is the one `git status` would run, and is turned off.
//! `--no-optional-locks` keeps status from writing the index, so it never
//! collides with the user's own git commands.

use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::shell_path;

/// How long one `git status` may take before it is stopped and the
/// repository given up on (until Ctrl-R).
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// Environment variables that would point git somewhere else than the
/// folder it runs in.
const REDIRECTS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// What the header shows of a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub head: Head,
    /// Commits ahead of and behind the upstream; `None` without one.
    pub ahead_behind: Option<(u32, u32)>,
    /// Anything staged, changed, deleted, untracked or in conflict.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// On a branch (also one without commits yet).
    Branch(String),
    /// A detached HEAD: the commit's first 7 hex digits.
    Detached(String),
}

impl Status {
    /// The header's text: `main ↑2 ↓1`, `(a1b2c3d)` when detached. Zero
    /// counts are left out. The app draws the dot for [`Status::dirty`]
    /// itself, so its size doesn't depend on a font.
    pub fn label(&self) -> String {
        let mut text = match &self.head {
            Head::Branch(name) => name.clone(),
            Head::Detached(oid) => format!("({oid})"),
        };
        if let Some((ahead, behind)) = self.ahead_behind {
            if ahead > 0 {
                text.push_str(&format!(" ↑{ahead}"));
            }
            if behind > 0 {
                text.push_str(&format!(" ↓{behind}"));
            }
        }
        text
    }
}

/// The folder holding the `.git` (a folder, or a file in a worktree or
/// submodule) that `dir` is in: `dir` or its nearest parent with one. Like
/// git, the search stops at a filesystem boundary. Only stats, no git.
pub fn find_root(dir: &Path) -> Option<PathBuf> {
    let device = dir.metadata().ok()?.dev();
    for folder in dir.ancestors() {
        if folder.metadata().ok()?.dev() != device {
            return None;
        }
        if folder.join(".git").symlink_metadata().is_ok() {
            return Some(folder.to_path_buf());
        }
    }
    None
}

/// Runs `git status` in `root` and reads the result. Fails if git is
/// missing, refuses the repository (another user's, say) or takes longer
/// than `timeout` ([`io::ErrorKind::TimedOut`]; git is stopped).
pub fn status(root: &Path, timeout: Duration) -> io::Result<Status> {
    let mut command = shell_path::command("git");
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "--no-renames",
            "-z",
        ])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0");
    for name in REDIRECTS {
        command.env_remove(name);
    }
    run(command, timeout)
}

fn run(mut command: Command, timeout: Duration) -> io::Result<Status> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no output"))?;
    let (sender, receiver) = mpsc::channel();
    // Reads until git exits (or is killed), so a large output never fills
    // the pipe.
    std::thread::spawn(move || {
        let _ = sender.send(parse(BufReader::new(stdout)));
    });
    let parsed = receiver.recv_timeout(timeout);
    if parsed.is_err() {
        let _ = child.kill();
    }
    let exit = child.wait()?;
    let parsed = parsed.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    if !exit.success() {
        return Err(io::Error::other(format!("git status failed: {exit}")));
    }
    Ok(parsed)
}

/// Reads `git status --porcelain=v2 --branch -z`: the `# branch.*`
/// headers, and whether any entry follows. Entries are never kept, so a
/// huge list of changes costs no memory.
fn parse(mut out: impl BufRead) -> io::Result<Status> {
    let mut oid = None;
    let mut head = None;
    let mut ahead_behind = None;
    let mut dirty = false;
    let mut record = Vec::new();
    loop {
        record.clear();
        if out.read_until(0, &mut record)? == 0 {
            break;
        }
        if record.last() == Some(&0) {
            record.pop();
        }
        // Entries start with their kind (`1`, `2`, `u`, `?`), never `#`.
        let Some(header) = record.strip_prefix(b"#") else {
            dirty |= !record.is_empty();
            continue;
        };
        let header = header.strip_prefix(b" ").unwrap_or(header);
        let header = String::from_utf8_lossy(header);
        let Some((key, value)) = header.split_once(' ') else {
            continue;
        };
        match key {
            "branch.oid" => oid = Some(value.to_owned()),
            "branch.head" => head = Some(value.to_owned()),
            "branch.ab" => ahead_behind = parse_ab(value),
            _ => {}
        }
    }
    let head = match head {
        Some(head) if head == "(detached)" => {
            let oid = oid.unwrap_or_default();
            Head::Detached(oid.chars().take(7).collect())
        }
        Some(name) => Head::Branch(name),
        None => return Err(io::Error::other("git reported no branch")),
    };
    Ok(Status {
        head,
        ahead_behind,
        dirty,
    })
}

/// `+2 -1` to `(2, 1)`.
fn parse_ab(value: &str) -> Option<(u32, u32)> {
    let (ahead, behind) = value.split_once(' ')?;
    Some((
        ahead.strip_prefix('+')?.parse().ok()?,
        behind.strip_prefix('-')?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parsed(text: &str) -> io::Result<Status> {
        parse(text.replace('\n', "\0").as_bytes())
    }

    fn branch(name: &str) -> Head {
        Head::Branch(name.into())
    }

    #[test]
    fn a_clean_branch_with_an_upstream() {
        let status = parsed(
            "# branch.oid 1234567890abcdef\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n",
        )
        .unwrap();
        assert_eq!(
            status,
            Status {
                head: branch("main"),
                ahead_behind: Some((2, 1)),
                dirty: false
            }
        );
        assert_eq!(status.label(), "main ↑2 ↓1");
    }

    #[test]
    fn any_entry_makes_it_dirty() {
        let header = "# branch.oid 1234567\n# branch.head dev\n";
        for entry in [
            "1 .M N... 100644 100644 100644 abc abc src/a.rs",
            "? new file.txt",
            "u UU N... 100644 100644 100644 100644 a b c conflict.rs",
        ] {
            let status = parsed(&format!("{header}{entry}\n")).unwrap();
            assert!(status.dirty, "{entry}");
            assert_eq!(status.label(), "dev");
        }
    }

    #[test]
    fn labels_leave_out_what_is_zero_or_missing() {
        let status = |ahead_behind| Status {
            head: branch("main"),
            ahead_behind,
            dirty: false,
        };
        assert_eq!(status(None).label(), "main");
        assert_eq!(status(Some((0, 0))).label(), "main");
        assert_eq!(status(Some((3, 0))).label(), "main ↑3");
        assert_eq!(status(Some((0, 4))).label(), "main ↓4");
    }

    #[test]
    fn a_detached_head_shows_its_commit() {
        let status = parsed("# branch.oid 1234567890abcdef\n# branch.head (detached)\n").unwrap();
        assert_eq!(status.head, Head::Detached("1234567".into()));
        assert_eq!(status.label(), "(1234567)");
    }

    #[test]
    fn a_new_repository_shows_its_branch() {
        let status = parsed("# branch.oid (initial)\n# branch.head main\n").unwrap();
        assert_eq!(status.label(), "main");
    }

    #[test]
    fn output_without_a_branch_is_an_error() {
        assert!(parsed("").is_err());
        assert!(parsed("# branch.oid 123\n").is_err());
    }

    #[test]
    fn odd_headers_are_ignored() {
        let status = parsed("# branch.head main\n# stash 3\n#\n# branch.ab broken\n").unwrap();
        assert_eq!(status.label(), "main");
        assert_eq!(parse_ab("+1"), None);
        assert_eq!(parse_ab("1 -2"), None);
    }

    #[test]
    fn the_root_is_the_nearest_folder_with_a_git_entry() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let deep = repo.join("a/b");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_root(&deep), None);
        fs::create_dir(repo.join(".git")).unwrap();
        assert_eq!(find_root(&deep), Some(repo.clone()));
        assert_eq!(find_root(&repo), Some(repo.clone()));
        // A worktree or submodule has a `.git` file.
        let sub = repo.join("a");
        fs::write(sub.join(".git"), "gitdir: ../.git/modules/a\n").unwrap();
        assert_eq!(find_root(&deep), Some(sub));
        assert_eq!(find_root(&dir.path().join("missing")), None);
    }

    /// Runs git in `dir` for test setup, with a fixed identity.
    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn status_of_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin");
        fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q"]);
        fs::write(origin.join("a"), "a").unwrap();
        git(&origin, &["add", "a"]);
        git(&origin, &["commit", "-qm", "a"]);
        let st = status(&origin, TIMEOUT).unwrap();
        assert_eq!(st.label(), "main");

        git(dir.path(), &["clone", "-q", "origin", "clone"]);
        let clone = dir.path().join("clone");
        assert_eq!(status(&clone, TIMEOUT).unwrap().label(), "main");
        fs::write(clone.join("b"), "b").unwrap();
        let untracked = status(&clone, TIMEOUT).unwrap();
        assert!(untracked.dirty);
        assert_eq!(untracked.label(), "main");
        git(&clone, &["add", "b"]);
        git(&clone, &["commit", "-qm", "b"]);
        assert_eq!(status(&clone, TIMEOUT).unwrap().label(), "main ↑1");
        fs::write(origin.join("a"), "changed").unwrap();
        git(&origin, &["commit", "-qam", "a2"]);
        git(&clone, &["fetch", "-q"]);
        assert_eq!(status(&clone, TIMEOUT).unwrap().label(), "main ↑1 ↓1");

        git(&origin, &["checkout", "-q", "--detach"]);
        assert!(status(&origin, TIMEOUT).unwrap().label().starts_with('('));
    }

    #[test]
    fn status_outside_a_repository_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(status(dir.path(), TIMEOUT).is_err());
    }

    #[test]
    fn a_repositorys_fsmonitor_is_never_run() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let marker = dir.path().join("ran");
        let hook = dir.path().join("hook.sh");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        git(&repo, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
        // The hook is what plain git would run.
        let _ = Command::new("git")
            .args(["status"])
            .current_dir(&repo)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        assert!(marker.exists(), "the test's hook works");
        fs::remove_file(&marker).unwrap();

        status(&repo, TIMEOUT).unwrap();
        assert!(!marker.exists(), "status ran the repository's fsmonitor");
    }

    #[test]
    fn status_leaves_the_index_alone() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("a"), "a").unwrap();
        git(dir.path(), &["add", "a"]);
        let index = dir.path().join(".git/index");
        let before = fs::metadata(&index).unwrap().modified().unwrap();
        // A newer mtime would make plain git status refresh the index.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.path().join("a"), "a").unwrap();
        status(dir.path(), TIMEOUT).unwrap();
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn a_slow_git_is_stopped() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10"]);
        let started = std::time::Instant::now();
        let error = run(command, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_failing_git_is_an_error() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf '# branch.head main\\0'; exit 1"]);
        assert!(run(command, TIMEOUT).is_err());
        let mut command = Command::new("sh");
        command.args(["-c", "printf '# branch.head main\\0'"]);
        assert_eq!(run(command, TIMEOUT).unwrap().label(), "main");
    }
}
