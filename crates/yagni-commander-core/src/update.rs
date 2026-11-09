//! Asks GitHub which release is the latest and compares it with this build
//! (one HTTPS request, nothing sent but the request itself), and installs a
//! release over this copy.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

pub fn latest(url: &str) -> Result<Version, String> {
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

/// TEMPORARY (the macOS test button): what `installed_app` would return for
/// a copy where the install script puts it, wherever this one runs.
pub fn default_app(home: &Path) -> PathBuf {
    let exe = home.join(INSTALLED_EXE);
    if cfg!(target_os = "macos") {
        exe.ancestors().nth(3).map(Path::to_owned).unwrap_or(exe)
    } else {
        exe
    }
}

/// Installs release `version` as the README's one-liner does: the release's
/// own `install.sh`, fetched with `curl` and run by `sh`, which checks the
/// download and replaces the installed copy. Blocks: run it on a thread.
///
/// Always through `curl`: macOS quarantines what an app with
/// `LSFileQuarantineEnabled` writes, and neither curl nor we set that, so
/// the unnotarized app opens afterwards. The error is a sentence.
pub fn install(releases: &str, version: Version) -> Result<(), String> {
    let url = format!("{releases}/v{version}/install.sh");
    let script = crate::shell_path::command("curl")
        .args(["-fsSL", "--max-time", "60", &url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Cannot run curl: {e}"))?;
    if !script.status.success() {
        let stderr = String::from_utf8_lossy(&script.stderr);
        return Err(github_trouble(&stderr)
            .unwrap_or_else(|| format!("Cannot download the installer: {}", stderr.trim())));
    }
    let mut child = crate::shell_path::command("sh")
        .arg("-s")
        .env("YAGNI_COMMANDER_RELEASES", releases)
        // Would move a Linux install away from the copy being replaced.
        .env_remove("PREFIX")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot run the installer: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A script that stops early shows up in its exit status.
        let _ = stdin.write_all(&script.stdout);
    }
    let done = child
        .wait_with_output()
        .map_err(|e| format!("The installer stopped unexpectedly: {e}"))?;
    if done.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&done.stderr);
        Err(github_trouble(&stderr)
            .unwrap_or_else(|| format!("The installer failed: {}", stderr.trim())))
    }
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

    #[test]
    fn install_runs_the_releases_script_and_points_it_at_the_releases() {
        let (tmp, url) = releases("");
        let mark = tmp.path().join("ran");
        let script = format!(
            "echo \"$YAGNI_COMMANDER_RELEASES\" > '{}'\n",
            mark.display()
        );
        std::fs::write(tmp.path().join("v1.2.3/install.sh"), script).unwrap();
        install(&url, Version(1, 2, 3)).unwrap();
        assert_eq!(std::fs::read_to_string(mark).unwrap().trim(), url);
    }

    #[test]
    fn a_failing_script_says_why() {
        let (_tmp, url) = releases("echo 'install.sh: broken' >&2; exit 1\n");
        let error = install(&url, Version(1, 2, 3)).unwrap_err();
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
        let error = install(&url, Version(1, 2, 3)).unwrap_err();
        assert!(error.contains("(HTTP 503)"), "{error}");
    }

    #[test]
    fn a_missing_release_runs_nothing() {
        let (_tmp, url) = releases("");
        let error = install(&url, Version(9, 9, 9)).unwrap_err();
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
