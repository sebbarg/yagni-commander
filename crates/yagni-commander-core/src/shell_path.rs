//! The `PATH` of the user's login shell, for the programs we start.
//!
//! An app started from Finder gets launchd's minimal `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), so `editor = "code"` is not found
//! although it works in a terminal. Like Zed and VS Code, the app asks the
//! login shell once at startup ([`start`]) and every program it starts is
//! looked up in, and inherits, that `PATH` ([`command`]). Only when
//! launchd started the app (Finder, Dock, Spotlight, `open`): started from
//! a shell, the inherited `PATH` is already the user's, as VS Code also
//! assumes. Only `PATH`:
//! setting the whole environment would need `std::env::set_var`, which is
//! `unsafe`.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

/// How long the shell may take: profiles that run version managers can be
/// slow. Programs started before then use the inherited `PATH`.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Brackets the shell's `env` output, so anything its profile prints
/// around it is ignored.
const MARKER: &str = "_YAGNI_COMMANDER_ENV_";

static LOGIN_PATH: OnceLock<OsString> = OnceLock::new();

/// Reads the login shell's `PATH` on a background thread if launchd
/// started the app; [`command`] uses it from then on. Called once, at
/// startup on macOS.
pub fn start() {
    if !started_by_launchd(nix::unistd::getppid()) {
        return;
    }
    std::thread::spawn(|| {
        // Without it, programs keep the inherited PATH.
        if let Some(path) = read(&login_shell(), TIMEOUT) {
            let _ = LOGIN_PATH.set(path);
        }
    });
}

/// launchd is process 1: the parent of every app LaunchServices opens.
fn started_by_launchd(parent: nix::unistd::Pid) -> bool {
    parent == nix::unistd::Pid::from_raw(1)
}

/// The `PATH` programs are started with: the login shell's once read,
/// else this process's own.
pub fn current() -> Option<OsString> {
    LOGIN_PATH
        .get()
        .cloned()
        .or_else(|| std::env::var_os("PATH"))
}

/// A [`Command`] for `program`, looked up in and given the login shell's
/// `PATH` once it is known.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    with_path(program, LOGIN_PATH.get().map(OsString::as_os_str))
}

fn with_path(program: impl AsRef<OsStr>, path: Option<&OsStr>) -> Command {
    let mut command = Command::new(program);
    // On Unix a `PATH` set on the command is also where std looks the
    // program up.
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command
}

/// `$SHELL`, else the account's shell, else zsh (macOS's default).
fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::getuid())
                .ok()
                .flatten()
                .map(|user| user.shell)
        })
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

/// Runs `shell` as an interactive login shell (both, since people set
/// `PATH` in either kind of profile) and returns the `PATH` it reports,
/// or `None` if it fails, reports none or takes longer than `timeout`.
pub fn read(shell: &std::path::Path, timeout: Duration) -> Option<OsString> {
    let script = format!("echo {MARKER}; /usr/bin/env; echo {MARKER}");
    let mut child = Command::new(shell)
        .args(["-i", "-l", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    // A program the profile starts in the background could keep the pipe
    // open: this thread may then never finish, but nobody waits for it.
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = sender.send(out);
    });
    let out = receiver.recv_timeout(timeout).ok();
    let _ = child.kill();
    let _ = child.wait();
    parse(&out?)
}

/// The `PATH=` line between the markers of `read`'s script.
fn parse(out: &[u8]) -> Option<OsString> {
    let mut lines = out.split(|&b| b == b'\n');
    lines.by_ref().find(|line| *line == MARKER.as_bytes())?;
    lines
        .take_while(|line| *line != MARKER.as_bytes())
        .find_map(|line| line.strip_prefix(b"PATH="))
        .filter(|path| !path.is_empty())
        .map(|path| OsStr::from_bytes(path).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// An executable script `name` in `dir` with `body` after the `#!`.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test's fork can hold a just-written script open for
        // writing for a moment (ETXTBSY); wait until it runs.
        while Command::new(&path)
            .args(["-c", "true"])
            .stdout(Stdio::null())
            .status()
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        path
    }

    #[test]
    fn only_a_launchd_start_asks_the_shell() {
        use nix::unistd::Pid;
        assert!(started_by_launchd(Pid::from_raw(1)));
        assert!(!started_by_launchd(Pid::from_raw(4242)));
        // Tests run under cargo, not launchd: `start` does nothing.
        start();
        assert!(LOGIN_PATH.get().is_none());
    }

    #[test]
    fn parse_takes_path_between_the_markers_only() {
        let out = format!(
            "PATH=/before\nWelcome!\n{MARKER}\nHOME=/h\nPATH=/opt/bin:/usr/bin\nX=1\n{MARKER}\nPATH=/after\n"
        );
        assert_eq!(parse(out.as_bytes()), Some("/opt/bin:/usr/bin".into()));
    }

    #[test]
    fn parse_finds_nothing_without_a_path_or_markers() {
        assert_eq!(parse(b"PATH=/x\n"), None);
        assert_eq!(
            parse(format!("{MARKER}\nHOME=/h\n{MARKER}\n").as_bytes()),
            None
        );
        assert_eq!(
            parse(format!("{MARKER}\nPATH=\n{MARKER}\n").as_bytes()),
            None
        );
        // The value may contain `=`.
        assert_eq!(
            parse(format!("{MARKER}\nPATH=/a=b\n{MARKER}").as_bytes()),
            Some("/a=b".into())
        );
    }

    #[test]
    fn read_runs_the_shell_as_an_interactive_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        // A fake shell: reports PATH only when given -i -l -c, like the
        // script it was handed would.
        let shell = script(
            dir.path(),
            "shell",
            r#"[ "$1 $2 $3" = "-i -l -c" ] || exit 1
echo profile noise
PATH=/login/bin:/usr/bin sh -c "$4""#,
        );
        assert_eq!(
            read(&shell, Duration::from_secs(10)),
            Some("/login/bin:/usr/bin".into())
        );
    }

    #[test]
    fn read_works_with_a_real_shell() {
        let path = read(std::path::Path::new("/bin/sh"), TIMEOUT).unwrap();
        assert!(
            path.as_bytes()
                .split(|&b| b == b':')
                .any(|dir| dir == b"/bin" || dir == b"/usr/bin")
        );
    }

    #[test]
    fn read_gives_up_on_a_failing_or_slow_shell() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&dir.path().join("missing"), TIMEOUT), None);
        let failing = script(dir.path(), "failing", "exit 3");
        assert_eq!(read(&failing, TIMEOUT), None);
        let slow = script(
            dir.path(),
            "slow",
            r#"[ "$1" = -c ] && exit 0; exec sleep 30"#,
        );
        let started = Instant::now();
        assert_eq!(read(&slow, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_finds_its_program_in_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        script(dir.path(), "only-here", r#"echo "$PATH""#);
        let out = with_path("only-here", Some(dir.path().as_os_str()))
            .output()
            .unwrap();
        // Found there, and the program inherits that PATH.
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            dir.path().to_str().unwrap()
        );
        assert!(with_path("only-here", None).output().is_err());
    }

    #[test]
    fn without_a_login_path_commands_keep_the_inherited_one() {
        // `start` is never called in tests.
        assert_eq!(current(), std::env::var_os("PATH"));
        let out = command("sh")
            .args(["-c", r#"printf %s "$PATH""#])
            .output()
            .unwrap();
        assert_eq!(
            OsStr::from_bytes(&out.stdout),
            std::env::var_os("PATH").unwrap()
        );
    }
}
