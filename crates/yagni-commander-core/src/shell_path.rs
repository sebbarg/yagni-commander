//! The environment of the user's login shell, for the programs we start.
//!
//! An app started from Finder gets launchd's minimal environment (`PATH`
//! is `/usr/bin:/bin:/usr/sbin:/sbin`), so `editor = "code"` is not found
//! although it works in a terminal. A Linux desktop session often lacks
//! what a shell profile sets, such as `~/.local/bin` on `PATH` or a
//! `DOCKER_HOST`. Like Zed and VS Code, the app asks the login shell once
//! at startup ([`start`]) and every program it starts gets those variables
//! ([`command`]); a `PATH` set on a command is also where std looks the
//! program up. On macOS only when launchd started the app (Finder, Dock,
//! Spotlight, `open`), and the shell's values win: started from a shell,
//! the inherited environment is already the user's, as VS Code also
//! assumes. On Linux always, since a launcher start can't be told apart
//! by the parent, and the inherited values win ([`combine`]): `PATH` gets
//! the shell's folders after its own ([`merged`]), other variables only
//! where the app has none, so a terminal's environment (a venv, direnv)
//! still comes first. The app's own environment is never changed: that
//! would need `std::env::set_var`, which is `unsafe`.
//!
//! The shell prints nothing itself: it runs this binary as a helper
//! ([`run_helper`]), which writes its environment NUL-separated, so a value
//! with a newline (bash's exported functions, a token) can't break it.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

/// How long the shell may take: profiles that run version managers can be
/// slow. Programs started before then get the inherited environment.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Brackets the helper's output, so anything the shell's profile prints
/// before it is ignored.
const MARKER: &str = "_YAGNI_COMMANDER_ENV_";

/// Set on the shell to this binary's path: run with it, the binary is the
/// helper that prints the environment.
const HELPER_VAR: &str = "YAGNI_COMMANDER_ENV_HELPER";

/// The shell's own bookkeeping and our helper's variable: never passed on.
const SKIPPED: [&str; 5] = ["SHLVL", "PWD", "OLDPWD", "_", HELPER_VAR];

/// A variable's name and value.
type Var = (OsString, OsString);

/// What every program we start gets on top of the inherited environment.
static LOGIN: OnceLock<Vec<Var>> = OnceLock::new();

/// Reads the login shell's environment on a background thread (on macOS
/// only if launchd started the app); [`command`] uses it from then on.
/// Called once, at startup.
pub fn start() {
    let macos = cfg!(target_os = "macos");
    if !asks_the_shell(macos, nix::unistd::getppid()) {
        return;
    }
    let Ok(helper) = std::env::current_exe() else {
        return;
    };
    std::thread::spawn(move || {
        // Without it, programs keep the inherited environment.
        if let Some(shell) = read(&login_shell(), &helper, TIMEOUT) {
            let inherited: Vec<Var> = std::env::vars_os().collect();
            let _ = LOGIN.set(combine(&inherited, shell, macos));
        }
    });
}

/// Whether [`start`] asks the shell: always on Linux, on macOS when
/// launchd started the app.
fn asks_the_shell(macos: bool, parent: nix::unistd::Pid) -> bool {
    !macos || started_by_launchd(parent)
}

/// launchd is process 1: the parent of every app LaunchServices opens.
fn started_by_launchd(parent: nix::unistd::Pid) -> bool {
    parent == nix::unistd::Pid::from_raw(1)
}

/// Run first in `main`: when the login shell started this binary as the
/// helper, writes the environment for [`read`] and returns the exit code.
pub fn run_helper() -> Option<i32> {
    helper(std::env::vars_os(), &mut std::io::stdout().lock())
}

/// [`run_helper`] on any environment and output: `None` without
/// `HELPER_VAR`.
fn helper(vars: impl Iterator<Item = Var>, out: &mut impl Write) -> Option<i32> {
    let vars: Vec<Var> = vars.collect();
    vars.iter().any(|(name, _)| name == HELPER_VAR).then(|| {
        let written = write_env(vars.into_iter(), out).and_then(|()| out.flush());
        if written.is_ok() { 0 } else { 1 }
    })
}

/// `MARKER`, each `name=value`, `MARKER`, every one ended by a NUL.
fn write_env(vars: impl Iterator<Item = Var>, out: &mut impl Write) -> std::io::Result<()> {
    out.write_all(MARKER.as_bytes())?;
    out.write_all(b"\0")?;
    for (name, value) in vars {
        out.write_all(name.as_bytes())?;
        out.write_all(b"=")?;
        out.write_all(value.as_bytes())?;
        out.write_all(b"\0")?;
    }
    out.write_all(MARKER.as_bytes())?;
    out.write_all(b"\0")
}

/// What programs get from the `shell`'s environment, given the app's
/// `inherited` one. With `shell_wins` (macOS after a launchd start) every
/// variable is the shell's; else `PATH` is [`merged`] and any other
/// variable is taken only if the app has none of that name. `SKIPPED`
/// variables and an empty `PATH` never are.
fn combine(inherited: &[Var], shell: Vec<Var>, shell_wins: bool) -> Vec<Var> {
    let inherited_value = |name: &OsStr| {
        inherited
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_os_str())
    };
    shell
        .into_iter()
        .filter(|(name, _)| !SKIPPED.iter().any(|skipped| name == skipped))
        .filter_map(|(name, value)| {
            if name == "PATH" {
                if value.is_empty() {
                    None
                } else if shell_wins {
                    Some((name, value))
                } else {
                    let path = merged(inherited_value(&name), &value);
                    Some((name, path))
                }
            } else if shell_wins || inherited_value(&name).is_none() {
                Some((name, value))
            } else {
                None
            }
        })
        .collect()
}

/// The `inherited` folders in their order, then the `login` shell's
/// folders that aren't among them. Empty entries (which mean the current
/// folder) are dropped from the shell's part.
fn merged(inherited: Option<&OsStr>, login: &OsStr) -> OsString {
    let mut folders: Vec<PathBuf> = inherited
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    for folder in std::env::split_paths(login) {
        if !folder.as_os_str().is_empty() && !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    // Split at ':' first, so no folder can contain one.
    std::env::join_paths(folders).unwrap_or_else(|_| login.to_owned())
}

/// The value `name` has for programs we start: the shell's once read,
/// else this process's own.
pub fn var(name: &str) -> Option<OsString> {
    LOGIN
        .get()
        .and_then(|vars| vars.iter().find(|(n, _)| n == name))
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os(name))
}

/// The `PATH` programs are started with.
pub fn current() -> Option<OsString> {
    var("PATH")
}

/// A [`Command`] for `program`, looked up in and given the login shell's
/// environment once it is known. Settings made on it afterwards win.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    with_vars(program, LOGIN.get().map_or(&[], Vec::as_slice))
}

fn with_vars(program: impl AsRef<OsStr>, vars: &[Var]) -> Command {
    let mut command = Command::new(program);
    // On Unix a `PATH` set on the command is also where std looks the
    // program up.
    command.envs(vars.iter().map(|(name, value)| (name, value)));
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
/// variables in either kind of profile), which runs `helper`, and returns
/// the environment the helper reports, or `None` if the shell fails, the
/// helper's output is cut short or it takes longer than `timeout`.
pub fn read(shell: &Path, helper: &Path, timeout: Duration) -> Option<Vec<Var>> {
    // The helper's path goes through a variable, never into the script, so
    // it needs no quoting for any shell (fish quotes differently).
    let script = format!("exec \"${HELPER_VAR}\"");
    let mut child = Command::new(shell)
        .args(["-i", "-l", "-c", &script])
        .env(HELPER_VAR, helper)
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

/// The variables between the markers of [`write_env`]'s output, or `None`
/// without both markers.
fn parse(out: &[u8]) -> Option<Vec<Var>> {
    let start = [MARKER.as_bytes(), b"\0"].concat();
    let at = out
        .windows(start.len())
        .position(|w| w == start.as_slice())?;
    let mut vars = Vec::new();
    for entry in out[at + start.len()..].split(|&b| b == 0) {
        if entry == MARKER.as_bytes() {
            return Some(vars);
        }
        // A name is never empty; the value may contain `=`.
        if let Some(eq) = entry.iter().skip(1).position(|&b| b == b'=') {
            let (name, value) = entry.split_at(eq + 1);
            vars.push((
                OsString::from_vec(name.to_vec()),
                OsString::from_vec(value[1..].to_vec()),
            ));
        }
    }
    None
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
    fn macos_asks_only_after_a_launchd_start_linux_always() {
        use nix::unistd::Pid;
        assert!(asks_the_shell(true, Pid::from_raw(1)));
        assert!(!asks_the_shell(true, Pid::from_raw(4242)));
        assert!(asks_the_shell(false, Pid::from_raw(1)));
        assert!(asks_the_shell(false, Pid::from_raw(4242)));
    }

    // Tests run under cargo, not launchd: on macOS `start` does nothing.
    // (On Linux it would run the owner's shell profile, so it isn't called.)
    #[cfg(target_os = "macos")]
    #[test]
    fn start_does_nothing_unless_launchd_started_the_app() {
        start();
        assert!(LOGIN.get().is_none());
    }

    #[test]
    fn merged_keeps_the_inherited_order_and_adds_the_shells_folders() {
        let path = merged(
            Some(OsStr::new("/venv/bin:/usr/bin:/bin")),
            OsStr::new("/home/u/.local/bin:/usr/bin::/bin:/opt/x"),
        );
        assert_eq!(path, "/venv/bin:/usr/bin:/bin:/home/u/.local/bin:/opt/x");
    }

    #[test]
    fn merged_without_an_inherited_path_is_the_shells() {
        assert_eq!(
            merged(None, OsStr::new("/home/u/.local/bin:/usr/bin")),
            "/home/u/.local/bin:/usr/bin"
        );
    }

    fn vars(list: &[(&str, &str)]) -> Vec<Var> {
        list.iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect()
    }

    #[test]
    fn on_linux_the_apps_values_win_and_path_is_merged() {
        let inherited = vars(&[("PATH", "/venv/bin:/usr/bin"), ("EDITOR", "kate")]);
        let shell = vars(&[
            ("PATH", "/home/u/.local/bin:/usr/bin"),
            ("EDITOR", "nvim"),
            ("DOCKER_HOST", "unix:///run/docker.sock"),
            ("SHLVL", "2"),
            ("PWD", "/home/u"),
            ("OLDPWD", "/"),
            ("_", "/usr/bin/env"),
            (HELPER_VAR, "/bin/yc"),
        ]);
        assert_eq!(
            combine(&inherited, shell, false),
            vars(&[
                ("PATH", "/venv/bin:/usr/bin:/home/u/.local/bin"),
                ("DOCKER_HOST", "unix:///run/docker.sock"),
            ])
        );
    }

    #[test]
    fn after_a_launchd_start_the_shells_values_win() {
        let inherited = vars(&[("PATH", "/usr/bin:/bin"), ("LANG", "C")]);
        let shell = vars(&[
            ("PATH", "/opt/homebrew/bin:/usr/bin:/bin"),
            ("LANG", "da_DK.UTF-8"),
            ("SHLVL", "1"),
        ]);
        assert_eq!(
            combine(&inherited, shell, true),
            vars(&[
                ("PATH", "/opt/homebrew/bin:/usr/bin:/bin"),
                ("LANG", "da_DK.UTF-8"),
            ])
        );
    }

    #[test]
    fn an_empty_shell_path_is_never_taken() {
        for shell_wins in [false, true] {
            let shell = vars(&[("PATH", ""), ("X", "1")]);
            assert_eq!(combine(&[], shell, shell_wins), vars(&[("X", "1")]));
        }
    }

    fn written(list: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        write_env(vars(list).into_iter(), &mut out).unwrap();
        out
    }

    #[test]
    fn parse_reads_what_the_helper_writes_after_any_profile_noise() {
        let list = [("PATH", "/a=b:/c"), ("MULTI", "one\ntwo"), ("EMPTY", "")];
        let mut out = b"Welcome!\nPATH=/noise\n".to_vec();
        out.extend(written(&list));
        assert_eq!(parse(&out), Some(vars(&list)));
    }

    #[test]
    fn the_helper_writes_only_when_the_shell_started_it() {
        let mut out = Vec::new();
        assert_eq!(helper(vars(&[("X", "1")]).into_iter(), &mut out), None);
        assert!(out.is_empty());
        let env = vars(&[("X", "1"), (HELPER_VAR, "/bin/yc")]);
        assert_eq!(helper(env.clone().into_iter(), &mut out), Some(0));
        assert_eq!(parse(&out), Some(env));
        let mut full = [0u8; 4];
        let mut short = &mut full[..];
        assert_eq!(
            helper(vars(&[(HELPER_VAR, "/bin/yc")]).into_iter(), &mut short),
            Some(1)
        );
    }

    #[test]
    fn parse_finds_nothing_without_both_markers() {
        assert_eq!(parse(b"PATH=/x\n"), None);
        let mut cut = written(&[("PATH", "/x")]);
        cut.truncate(cut.len() - MARKER.len() - 1);
        assert_eq!(parse(&cut), None);
        assert_eq!(parse(&written(&[])), Some(Vec::new()));
    }

    /// A stand-in for this binary as the helper: writes `PATH` and a value
    /// with a newline the way [`write_env`] does.
    fn helper_script(dir: &Path) -> PathBuf {
        script(
            dir,
            "helper",
            &format!(
                r#"[ "$1" = -c ] && exit 0
printf '%s\0' {MARKER} "PATH=$PATH" "MULTI=a
b" {MARKER}"#
            ),
        )
    }

    fn value<'a>(vars: &'a [Var], name: &str) -> Option<&'a OsStr> {
        vars.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_os_str())
    }

    #[test]
    fn read_runs_the_shell_as_an_interactive_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        // A fake shell: runs the script only when given -i -l -c.
        let shell = script(
            dir.path(),
            "shell",
            r#"[ "$1 $2 $3" = "-i -l -c" ] || exit 1
echo profile noise
PATH=/login/bin:/usr/bin exec sh -c "$4""#,
        );
        let got = read(&shell, &helper_script(dir.path()), Duration::from_secs(10)).unwrap();
        assert_eq!(value(&got, "PATH"), Some(OsStr::new("/login/bin:/usr/bin")));
        assert_eq!(value(&got, "MULTI"), Some(OsStr::new("a\nb")));
    }

    #[test]
    fn read_works_with_a_real_shell() {
        let dir = tempfile::tempdir().unwrap();
        let got = read(Path::new("/bin/sh"), &helper_script(dir.path()), TIMEOUT).unwrap();
        let path = value(&got, "PATH").unwrap();
        assert!(
            path.as_bytes()
                .split(|&b| b == b':')
                .any(|dir| dir == b"/bin" || dir == b"/usr/bin")
        );
    }

    #[test]
    fn read_gives_up_on_a_failing_or_slow_shell() {
        let dir = tempfile::tempdir().unwrap();
        let helper = helper_script(dir.path());
        assert_eq!(read(&dir.path().join("missing"), &helper, TIMEOUT), None);
        let failing = script(dir.path(), "failing", "exit 3");
        assert_eq!(read(&failing, &helper, TIMEOUT), None);
        let slow = script(
            dir.path(),
            "slow",
            r#"[ "$1" = -c ] && exit 0; exec sleep 30"#,
        );
        let started = Instant::now();
        assert_eq!(read(&slow, &helper, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_gets_the_variables_and_finds_its_program_in_their_path() {
        let dir = tempfile::tempdir().unwrap();
        script(
            dir.path(),
            "only-here",
            r#"printf '%s %s' "$PATH" "$EXTRA""#,
        );
        let path = dir.path().to_str().unwrap();
        let out = with_vars("only-here", &vars(&[("PATH", path), ("EXTRA", "yes")]))
            .output()
            .unwrap();
        // Found there, and the program inherits the variables.
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{path} yes")
        );
        assert!(with_vars("only-here", &[]).output().is_err());
    }

    #[test]
    fn without_a_login_environment_commands_keep_the_inherited_one() {
        // `start` is never called in tests.
        assert_eq!(current(), std::env::var_os("PATH"));
        assert_eq!(var("HOME"), std::env::var_os("HOME"));
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
