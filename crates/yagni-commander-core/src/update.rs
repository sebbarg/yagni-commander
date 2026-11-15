//! Asks GitHub which release is the latest and compares it with this build
//! (one HTTPS request, nothing sent but the request itself), and installs a
//! release over this copy.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Redirects to the latest release's tag page (`.../releases/tag/v1.2.3`).
/// GitHub's "latest" skips drafts and prereleases. Reading the redirect
/// needs no API (60 requests an hour without a token) and no JSON.
pub const LATEST_URL: &str = "https://github.com/sebbarg/yagni-commander/releases/latest";

/// How to install a release, for the "available" message.
pub const INSTALL_URL: &str = "https://github.com/sebbarg/yagni-commander#installing";

/// A release's files are at `<this>/v<version>/<file>`.
pub const RELEASES_URL: &str = "https://github.com/sebbarg/yagni-commander/releases/download";

/// Where `install/install.sh` puts the program, under the home folder.
const INSTALLED_EXE: &str = if cfg!(target_os = "macos") {
    "Applications/yagni-commander.app/Contents/MacOS/yagni-commander"
} else {
    ".local/bin/yagni-commander"
};

/// The whole check gives up after this long.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A release version, `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// `1.2.3` or `v1.2.3`; nothing else (release tags are `v<version>`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse().ok())
                .flatten()
        });
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// This build's version.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// What a check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// This build is the latest release (or newer: a development build).
    UpToDate(Version),
    /// A newer release is out.
    Available { latest: Version, current: Version },
}

impl Check {
    pub fn new(current: Version, latest: Version) -> Self {
        if latest > current {
            Self::Available { latest, current }
        } else {
            Self::UpToDate(current)
        }
    }
}

/// Asks `url` (`LATEST_URL`, or a test server) for the latest release and
/// compares it with this build. Blocks for up to `TIMEOUT`: run it on a
/// thread of its own. The error is a sentence for the message box.
pub fn check(url: &str) -> Result<Check, String> {
    Ok(Check::new(Version::current(), latest(url)?))
}

fn latest(url: &str) -> Result<Version, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("yagni-commander/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let response = agent
        .head(url)
        .call()
        .map_err(|e| format!("Cannot reach GitHub: {e}"))?;
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok());
    location
        .and_then(|l| l.rsplit_once("/tag/"))
        .and_then(|(_, tag)| Version::parse(tag))
        .ok_or_else(|| match location {
            Some(l) => format!("GitHub sent an unexpected address: {l}"),
            None => format!("GitHub sent no release (status {}).", response.status()),
        })
}

/// What to start after an update, if `exe` is where the install script puts
/// the program (the installed binary; on macOS its `.app`). A copy anywhere
/// else (a build, a bundle dragged to `/Applications`) isn't the one the
/// script would replace, so it can't update itself.
pub fn installed_app(exe: &Path, home: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().ok()?;
    if exe != home.canonicalize().ok()?.join(INSTALLED_EXE) {
        return None;
    }
    if cfg!(target_os = "macos") {
        exe.ancestors().nth(3).map(Path::to_owned)
    } else {
        Some(exe)
    }
}

/// A step the installer reports while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Fetching a release file (`install.sh`, `SHA256SUMS`, the app).
    Downloading(String),
    /// Replacing the installed copy with the one downloaded, at this place.
    /// No longer cancellable: the script ignores the signal from here on.
    Copying(String),
}

impl Step {
    /// A line the release's `install.sh` writes to its output; any other
    /// line (the Linux tarball's own script talks too) is none.
    fn parse(line: &str) -> Option<Self> {
        if let Some(file) = line.strip_prefix("downloading ") {
            Some(Self::Downloading(file.to_owned()))
        } else {
            line.strip_prefix("copying ")
                .map(|place| Self::Copying(place.to_owned()))
        }
    }
}

/// Why an install didn't happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// `cancel` was set before the copying began. Nothing was changed.
    Cancelled,
    /// A sentence for the error box.
    Failed(String),
}

/// How often a running install looks at `cancel` and at its child.
const POLL: Duration = Duration::from_millis(50);

/// Installs release `version` as the README's one-liner does: the release's
/// own `install.sh`, fetched with `curl` and run by `sh`, which checks the
/// download and replaces the installed copy. Blocks: run it on a thread.
/// `step` hears each download and the start of the copying.
///
/// Setting `cancel` stops it (every process it started, curl included)
/// until the script says it is copying; from there the script ignores the
/// signal, so an install can't stop halfway, and it finishes as if `cancel`
/// had not been set.
///
/// Always through `curl`: macOS quarantines what an app with
/// `LSFileQuarantineEnabled` writes, and neither curl nor we set that, so
/// the unnotarized app opens afterwards.
pub fn install(
    releases: &str,
    version: Version,
    cancel: &AtomicBool,
    mut step: impl FnMut(Step),
) -> Result<(), InstallError> {
    let url = format!("{releases}/v{version}/install.sh");
    step(Step::Downloading("install.sh".to_owned()));
    let mut curl = crate::shell_path::command("curl");
    curl.args(["-fsSL", "--max-time", "60", &url]);
    let script = run_cancellable(curl, None, cancel, |_| {})
        .map_err(|e| e.or_failed(|e| format!("Cannot run curl: {e}")))?;
    if !script.success {
        return Err(InstallError::Failed(
            github_trouble(&script.stderr).unwrap_or_else(|| {
                format!("Cannot download the installer: {}", script.stderr.trim())
            }),
        ));
    }
    let mut sh = crate::shell_path::command("sh");
    sh.arg("-s")
        .env("YAGNI_COMMANDER_RELEASES", releases)
        // Would move a Linux install away from the copy being replaced.
        .env_remove("PREFIX");
    let mut copying = false;
    let done = run_cancellable(sh, Some(script.stdout), cancel, |line| {
        if let Some(found) = Step::parse(line) {
            copying |= matches!(found, Step::Copying(_));
            step(found);
        }
    });
    let done = match done {
        Ok(done) => done,
        // Too late to cancel: a failure while copying is a failure.
        Err(RunError::Cancelled(done)) if copying => done,
        Err(e) => return Err(e.or_failed(|e| format!("Cannot run the installer: {e}"))),
    };
    if done.success {
        Ok(())
    } else {
        Err(InstallError::Failed(
            github_trouble(&done.stderr)
                .unwrap_or_else(|| format!("The installer failed: {}", done.stderr.trim())),
        ))
    }
}

/// What a finished child left.
struct Finished {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}

enum RunError {
    /// Signalled and failed: what it left, in case it was too late.
    Cancelled(Finished),
    Io(std::io::Error),
}

impl RunError {
    fn or_failed(self, sentence: impl FnOnce(std::io::Error) -> String) -> InstallError {
        match self {
            Self::Cancelled(_) => InstallError::Cancelled,
            Self::Io(e) => InstallError::Failed(sentence(e)),
        }
    }
}

/// Runs `command` in a process group of its own, feeding it `input`, and
/// hands each line of its output to `line` as it comes. Once `cancel` is
/// set, the group gets SIGTERM; a child that ignores it (the script while
/// copying) runs to its end, and its result counts.
fn run_cancellable(
    mut command: std::process::Command,
    input: Option<Vec<u8>>,
    cancel: &AtomicBool,
    mut line: impl FnMut(&str),
) -> Result<Finished, RunError> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::sync::mpsc;

    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(RunError::Io)?;
    let group = Pid::from_raw(child.id() as i32);
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut buf = [0; 8192];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut err = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = err.read_to_end(&mut text);
        String::from_utf8_lossy(&text).into_owned()
    });
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        // On a thread: a script that writes before it has read all of its
        // input must not wait for us. One that stops early shows up in its
        // exit status.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut stdout = Vec::new();
    let mut seen = 0;
    let mut signalled = false;
    let mut take = |chunk: Vec<u8>, stdout: &mut Vec<u8>, seen: &mut usize| {
        stdout.extend_from_slice(&chunk);
        while let Some(end) = stdout[*seen..].iter().position(|&b| b == b'\n') {
            line(String::from_utf8_lossy(&stdout[*seen..*seen + end]).trim_end());
            *seen += end + 1;
        }
    };
    let status = loop {
        if cancel.load(Ordering::Relaxed) && !signalled {
            signalled = true;
            let _ = killpg(group, Signal::SIGTERM);
        }
        match rx.recv_timeout(POLL) {
            Ok(chunk) => take(chunk, &mut stdout, &mut seen),
            // The output closed: the child is ending, or left it to others.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(status) = child.try_wait().map_err(RunError::Io)? {
                    break status;
                }
                std::thread::sleep(POLL);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    let _ = reader.join();
    while let Ok(chunk) = rx.try_recv() {
        take(chunk, &mut stdout, &mut seen);
    }
    let stderr = errors.join().unwrap_or_default();
    let finished = Finished {
        success: status.success(),
        stdout,
        stderr,
    };
    // Signalled and failed: cancelled, whatever else it said.
    if signalled && !finished.success {
        return Err(RunError::Cancelled(finished));
    }
    Ok(finished)
}

/// A sentence for curl's error text when the cause is GitHub or the network,
/// not the update. Only curl's messages match, and every download comes
/// before the install script touches the installed copy, so nothing changed.
/// An unknown wording just keeps the technical message.
fn github_trouble(stderr: &str) -> Option<String> {
    // curl: DNS, connect, timeout, TLS, empty reply, cut off, receive.
    const UNREACHABLE: [&str; 7] = [
        "curl: (6)",
        "curl: (7)",
        "curl: (28)",
        "curl: (35)",
        "curl: (52)",
        "curl: (18)",
        "curl: (56)",
    ];
    if let Some((_, rest)) = stderr.split_once("returned error: 5") {
        let digits = rest.get(..2)?;
        if digits.bytes().all(|b| b.is_ascii_digit()) {
            return Some(format!(
                "GitHub is having trouble serving this release (HTTP 5{digits}). Nothing was changed. Try again later."
            ));
        }
    }
    UNREACHABLE
        .iter()
        .any(|code| stderr.contains(code))
        .then(|| {
            "Cannot reach GitHub. Nothing was changed. Check the connection and try again."
                .to_owned()
        })
}

/// Starts `app` once this process has ended. The caller quits right after,
/// so the state file is saved before the new copy reads it (and macOS's
/// `open` would only bring the old copy to the front).
pub fn relaunch(app: &Path) -> Result<(), String> {
    let command: Vec<&OsStr> = if cfg!(target_os = "macos") {
        vec![OsStr::new("open"), app.as_os_str()]
    } else {
        vec![app.as_os_str()]
    };
    relaunch_after(std::process::id(), &command)
}

/// Waits for `pid` to end, 10 s at most, then runs `command`, from a shell
/// that outlives us.
#[allow(clippy::zombie_processes)] // It is meant to outlive us.
fn relaunch_after(pid: u32, command: &[&OsStr]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    const WAIT: &str = r#"i=0; while kill -0 "$0" 2>/dev/null && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i+1)); done; exec "$@""#;
    let mut args: Vec<OsString> = vec!["-c".into(), WAIT.into(), pid.to_string().into()];
    args.extend(command.iter().map(|part| part.to_os_string()));
    crate::shell_path::command("sh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Cannot restart: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn versions_parse_with_or_without_v_and_nothing_else() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("10.0.12"), Some(Version(10, 0, 12)));
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "v1.2.3-rc1",
            "1..3",
            "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn only_a_newer_release_is_available() {
        let current = Version(1, 2, 3);
        for older_or_same in [Version(1, 2, 3), Version(1, 2, 2), Version(0, 9, 9)] {
            assert_eq!(Check::new(current, older_or_same), Check::UpToDate(current));
        }
        for newer in [Version(1, 2, 4), Version(1, 3, 0), Version(2, 0, 0)] {
            assert_eq!(
                Check::new(current, newer),
                Check::Available {
                    latest: newer,
                    current
                }
            );
        }
        // 1.10 is newer than 1.9: numbers, not text.
        assert!(Version(1, 10, 0) > Version(1, 9, 0));
    }

    /// A server that answers one request with `response` and returns the
    /// request's text.
    fn serve(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, thread)
    }

    #[test]
    fn the_latest_release_comes_from_the_redirect() {
        let (url, server) = serve(
            "HTTP/1.1 302 Found\r\n\
             Location: https://github.com/sebbarg/yagni-commander/releases/tag/v999.0.0\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert_eq!(
            check(&url),
            Ok(Check::Available {
                latest: Version(999, 0, 0),
                current: Version::current()
            })
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("HEAD /releases/latest "), "{request}");
        let agent = concat!("user-agent: yagni-commander/", env!("CARGO_PKG_VERSION"));
        assert!(request.to_lowercase().contains(agent), "{request}");
    }

    #[test]
    fn an_older_or_equal_release_is_up_to_date() {
        let (url, _) = serve(
            "HTTP/1.1 302 Found\r\n\
             Location: https://github.com/sebbarg/yagni-commander/releases/tag/v0.0.1\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert_eq!(check(&url), Ok(Check::UpToDate(Version::current())));
    }

    #[test]
    fn odd_answers_are_errors() {
        let (url, _) = serve("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        assert_eq!(
            check(&url),
            Err("GitHub sent no release (status 200 OK).".into())
        );
        let (url, _) = serve(
            "HTTP/1.1 302 Found\r\nLocation: https://github.com/login\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(
            check(&url),
            Err("GitHub sent an unexpected address: https://github.com/login".into())
        );
        let (url, _) = serve("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        let error = check(&url).unwrap_err();
        assert!(error.starts_with("Cannot reach GitHub: "), "{error}");
    }

    /// The real request, over the network: `cargo test -p
    /// yagni-commander-core -- --ignored github`.
    #[test]
    #[ignore = "uses the network"]
    fn github_answers_with_a_release() {
        let result = check(LATEST_URL);
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn no_server_is_an_error() {
        // Bound, then dropped: nothing listens there.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let error = check(&format!("http://127.0.0.1:{port}/")).unwrap_err();
        assert!(error.starts_with("Cannot reach GitHub: "), "{error}");
    }

    #[test]
    fn only_the_install_scripts_location_can_update_itself() {
        let home = tempfile::tempdir().unwrap();
        let exe = home.path().join(INSTALLED_EXE);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let elsewhere = home.path().join("yagni-commander");
        std::fs::write(&elsewhere, b"").unwrap();
        let found = installed_app(&exe, home.path()).unwrap();
        let wanted = home
            .path()
            .canonicalize()
            .unwrap()
            .join(if cfg!(target_os = "macos") {
                "Applications/yagni-commander.app"
            } else {
                INSTALLED_EXE
            });
        assert_eq!(found, wanted);
        assert_eq!(installed_app(&elsewhere, home.path()), None);
        assert_eq!(installed_app(&home.path().join("gone"), home.path()), None);
    }

    /// A release folder with `script` as v1.2.3's install.sh.
    fn releases(script: &str) -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("v1.2.3")).unwrap();
        std::fs::write(tmp.path().join("v1.2.3/install.sh"), script).unwrap();
        let url = format!("file://{}", tmp.path().display());
        (tmp, url)
    }

    /// `install` with no cancel, the steps dropped.
    fn run_install(url: &str, version: Version) -> Result<(), InstallError> {
        install(url, version, &AtomicBool::new(false), |_| {})
    }

    fn failed(result: Result<(), InstallError>) -> String {
        match result {
            Err(InstallError::Failed(e)) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_scripts_downloads_and_copying_are_steps() {
        let (_tmp, url) = releases(
            "echo 'installing yagni-commander 1.2.3'\n\
             echo 'downloading SHA256SUMS'\n\
             echo 'copying /some/place'\n\
             echo 'installed /some/place/bin'\n",
        );
        let mut steps = Vec::new();
        install(&url, Version(1, 2, 3), &AtomicBool::new(false), |s| {
            steps.push(s)
        })
        .unwrap();
        assert_eq!(
            steps,
            [
                Step::Downloading("install.sh".into()),
                Step::Downloading("SHA256SUMS".into()),
                Step::Copying("/some/place".into()),
            ]
        );
    }

    #[test]
    fn cancel_before_the_copying_stops_every_process() {
        let (tmp, url) = releases("");
        let mark = tmp.path().join("ran on");
        let script = format!(
            "echo 'downloading big'\n(sleep 2; touch '{0}') &\nsleep 2; touch '{0}'\n",
            mark.display()
        );
        std::fs::write(tmp.path().join("v1.2.3/install.sh"), script).unwrap();
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let result = install(&url, Version(1, 2, 3), &cancel, |s| {
            if s == Step::Downloading("big".into()) {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(result, Err(InstallError::Cancelled));
        assert!(started.elapsed() < Duration::from_millis(1500));
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!mark.exists(), "a child outlived the cancel");
    }

    #[test]
    fn cancel_while_copying_comes_too_late() {
        let (tmp, url) = releases("");
        let mark = tmp.path().join("copied");
        let script = format!(
            "trap '' TERM\necho 'copying x'\nsleep 0.3; touch '{}'\n",
            mark.display()
        );
        std::fs::write(tmp.path().join("v1.2.3/install.sh"), script).unwrap();
        let cancel = AtomicBool::new(false);
        let result = install(&url, Version(1, 2, 3), &cancel, |s| {
            if matches!(s, Step::Copying(_)) {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(result, Ok(()));
        assert!(mark.exists());
    }

    #[test]
    fn a_failure_while_copying_is_no_cancel() {
        let (_tmp, url) =
            releases("trap '' TERM\necho 'copying x'\nsleep 0.3\necho 'disk full' >&2; exit 1\n");
        let cancel = AtomicBool::new(false);
        let result = install(&url, Version(1, 2, 3), &cancel, |s| {
            if matches!(s, Step::Copying(_)) {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(failed(result), "The installer failed: disk full");
    }

    /// The real `install/install.sh` on a release folder whose tarball's
    /// own `install.sh` is `inner`.
    #[cfg(target_os = "linux")]
    fn real_script_release(inner: &str) -> (tempfile::TempDir, String) {
        let (tmp, url) = releases("");
        let name = "yagni-commander-1.2.3-linux-x86_64";
        let dir = tmp.path().join("v1.2.3");
        let unpacked = tmp.path().join(name);
        std::fs::create_dir(&unpacked).unwrap();
        std::fs::write(unpacked.join("install.sh"), inner).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            unpacked.join("install.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let tarball = format!("{name}.tar.gz");
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.join(&tarball))
            .arg("-C")
            .arg(tmp.path())
            .arg(name)
            .status()
            .unwrap();
        assert!(status.success());
        let sum = std::process::Command::new("sha256sum")
            .arg(dir.join(&tarball))
            .output()
            .unwrap();
        let sum = String::from_utf8(sum.stdout).unwrap();
        let sum = sum.split_whitespace().next().unwrap();
        std::fs::write(dir.join("SHA256SUMS"), format!("{sum}  {tarball}\n")).unwrap();
        // Its machine check let through, so this runs on any Linux.
        let script = include_str!("../../../install/install.sh")
            .replace("@VERSION@", "1.2.3")
            .replace(r#"[ "$(uname -m)" = x86_64 ] ||"#, "true ||");
        std::fs::write(dir.join("install.sh"), script).unwrap();
        (tmp, url)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_install_script_reports_its_steps_and_finishes_a_cancelled_copy() {
        let marks = tempfile::tempdir().unwrap();
        let mark = marks.path().join("copied");
        let inner = format!("#!/bin/sh\nsleep 0.5\ntouch '{}'\n", mark.display());
        let (_tmp, url) = real_script_release(&inner);
        let cancel = AtomicBool::new(false);
        let mut steps = Vec::new();
        let result = install(&url, Version(1, 2, 3), &cancel, |s| {
            if matches!(s, Step::Copying(_)) {
                cancel.store(true, Ordering::Relaxed);
            }
            steps.push(s);
        });
        assert_eq!(result, Ok(()));
        assert!(mark.exists(), "the copy ran to its end");
        assert_eq!(steps.len(), 4, "{steps:?}");
        assert_eq!(
            steps[..3],
            [
                Step::Downloading("install.sh".into()),
                Step::Downloading("SHA256SUMS".into()),
                Step::Downloading("yagni-commander-1.2.3-linux-x86_64.tar.gz".into()),
            ]
        );
        assert!(matches!(&steps[3], Step::Copying(_)), "{steps:?}");
    }

    #[test]
    fn install_runs_the_releases_script_and_points_it_at_the_releases() {
        let (tmp, url) = releases("");
        let mark = tmp.path().join("ran");
        let script = format!(
            "echo \"$YAGNI_COMMANDER_RELEASES\" > '{}'\n",
            mark.display()
        );
        std::fs::write(tmp.path().join("v1.2.3/install.sh"), script).unwrap();
        run_install(&url, Version(1, 2, 3)).unwrap();
        assert_eq!(std::fs::read_to_string(mark).unwrap().trim(), url);
    }

    #[test]
    fn a_failing_script_says_why() {
        let (_tmp, url) = releases("echo 'install.sh: broken' >&2; exit 1\n");
        let error = failed(run_install(&url, Version(1, 2, 3)));
        assert_eq!(error, "The installer failed: install.sh: broken");
    }

    #[test]
    fn github_and_network_trouble_get_a_plain_sentence() {
        let served =
            "curl: (22) The requested URL returned error: 504\ninstall.sh: could not download x";
        assert_eq!(
            github_trouble(served).unwrap(),
            "GitHub is having trouble serving this release (HTTP 504). Nothing was changed. Try again later."
        );
        for line in [
            "curl: (6) Could not resolve host: github.com",
            "curl: (7) Failed to connect",
            "curl: (28) Operation too slow",
            "curl: (56) Recv failure",
        ] {
            assert!(
                github_trouble(line)
                    .unwrap()
                    .starts_with("Cannot reach GitHub."),
                "{line}"
            );
        }
        for other in [
            "curl: (22) The requested URL returned error: 404",
            "install.sh: x does not match its checksum",
            "returned error: 5",
        ] {
            assert_eq!(github_trouble(other), None, "{other}");
        }
    }

    #[test]
    fn a_script_stopped_by_github_says_so() {
        let (_tmp, url) =
            releases("echo 'curl: (22) The requested URL returned error: 503' >&2; exit 1\n");
        let error = failed(run_install(&url, Version(1, 2, 3)));
        assert!(error.contains("(HTTP 503)"), "{error}");
    }

    #[test]
    fn a_missing_release_runs_nothing() {
        let (_tmp, url) = releases("");
        let error = failed(run_install(&url, Version(9, 9, 9)));
        assert!(
            error.starts_with("Cannot download the installer: "),
            "{error}"
        );
    }

    #[test]
    fn relaunch_waits_for_the_old_process_then_runs_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let mark = tmp.path().join("started");
        let mut old = std::process::Command::new("sleep")
            .arg("0.4")
            .spawn()
            .unwrap();
        let line = format!("echo up > '{}'", mark.display());
        let parts = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new(&line)];
        relaunch_after(old.id(), &parts).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(!mark.exists(), "the old process still runs");
        old.wait().unwrap();
        for _ in 0..100 {
            if mark.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("never started");
    }
}
