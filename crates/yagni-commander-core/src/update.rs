//! Help > Check for updates: asks GitHub which release is the latest and
//! compares it with this build. One HTTPS request, nothing sent but the
//! request itself.

use std::fmt;
use std::time::Duration;

/// Redirects to the latest release's tag page (`.../releases/tag/v1.2.3`).
/// GitHub's "latest" skips drafts and prereleases. Reading the redirect
/// needs no API (60 requests an hour without a token) and no JSON.
pub const LATEST_URL: &str = "https://github.com/sebbarg/yagni-commander/releases/latest";

/// How to install a release, for the "available" message.
pub const INSTALL_URL: &str = "https://github.com/sebbarg/yagni-commander#installing";

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
}
