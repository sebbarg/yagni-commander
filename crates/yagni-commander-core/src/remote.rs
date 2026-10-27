//! Remote folders over SFTP (a prototype).
//!
//! A remote folder is the panel path `sftp://host/abs/path`; `sftp://host`
//! alone is the login folder, resolved when it is read. `host` is what the
//! user would type after `ssh` (a `~/.ssh/config` alias, `user@host`). As a
//! local path such a path is relative and names nothing, so a local
//! operation that strays onto it fails instead of touching local files; the
//! UI refuses the writing commands on a remote panel anyway.
//!
//! The transport is the system's `ssh` ([`connect`]): `~/.ssh/config`, the
//! agent, `known_hosts` and `ProxyJump` work as in a terminal, and no
//! cryptography is ours. `BatchMode` is on, so a password or an unknown host
//! key fails instead of asking on a terminal nobody sees. Over its standard
//! input and output we speak SFTP ([`crate::sftp`]).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use crate::entry::{Entry, EntryKind};
use crate::listing::Listing;
use crate::sftp::{Attrs, Client, Name};

pub const SCHEME: &str = "sftp://";

/// Why a command that writes refuses on a remote panel.
pub const ON_REMOTE: &str = "Not available on a remote folder.";

/// The last bytes of ssh's error output kept for a failure message.
const STDERR_KEPT: usize = 4096;

/// A remote path taken apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location<'a> {
    pub host: &'a str,
    /// The absolute path on the host; empty for the login folder.
    pub path: &'a [u8],
}

/// `path` taken apart, if it is a remote path with a usable host.
pub fn parse(path: &Path) -> Option<Location<'_>> {
    let rest = path
        .as_os_str()
        .as_bytes()
        .strip_prefix(SCHEME.as_bytes())?;
    let end = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
    let host = std::str::from_utf8(&rest[..end]).ok()?;
    valid_host(host).then_some(Location {
        host,
        path: &rest[end..],
    })
}

pub fn is_remote(path: &Path) -> bool {
    path.as_os_str().as_bytes().starts_with(SCHEME.as_bytes())
}

/// A host ssh takes as a destination and nothing else: never an option
/// (`-oProxyCommand=...`), no spaces or control characters.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-@:[]%".contains(c))
}

/// The panel path of `remote` (absolute) on `host`.
pub fn path_of(host: &str, remote: &[u8]) -> PathBuf {
    let mut bytes = format!("{SCHEME}{host}").into_bytes();
    bytes.extend_from_slice(remote);
    PathBuf::from(OsString::from_vec(bytes))
}

/// The folder above a remote path; `None` at the host's root.
pub fn parent(path: &Path) -> Option<PathBuf> {
    let location = parse(path)?;
    let trimmed = trim_end_slashes(location.path);
    if trimmed.is_empty() {
        return None;
    }
    let cut = trimmed.iter().rposition(|&b| b == b'/')?;
    let parent = if cut == 0 { b"/" } else { &trimmed[..cut] };
    Some(path_of(location.host, parent))
}

/// The last name of a remote path; `None` at the root.
pub fn file_name(path: &Path) -> Option<OsString> {
    let trimmed = trim_end_slashes(parse(path)?.path);
    let start = trimmed.iter().rposition(|&b| b == b'/')? + 1;
    (start < trimmed.len()).then(|| OsString::from_vec(trimmed[start..].to_vec()))
}

fn trim_end_slashes(mut path: &[u8]) -> &[u8] {
    while let [rest @ .., b'/'] = path {
        path = rest;
    }
    path
}

/// A name a server may give for an entry: one plain component.
pub(crate) fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

/// F2 on a remote folder: `name` must be one plain name, as locally.
pub fn check_name(name: &str) -> io::Result<()> {
    crate::fs_ops::validate_name(name)
}

/// F7 and Shift-F4 on a remote folder: a relative path of plain names
/// (`a/b`), as locally.
pub fn check_relative(path: &str) -> io::Result<()> {
    crate::fs_ops::validate_relative(path)
}

/// `name` inside remote folder `dir`.
pub(crate) fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = trim_end_slashes(dir).to_vec();
    path.push(b'/');
    path.extend_from_slice(name);
    path
}

/// Builds the command whose standard input and output speak SFTP to `host`;
/// with `share` false it uses no shared connection (see [`connect_unshared`]).
pub type Connector = fn(&str, bool) -> Command;

static CONNECTOR: RwLock<Connector> = RwLock::new(ssh_command);

/// Replaces how sessions are started: tests run a local `sftp-server`.
pub fn set_connector(connector: Connector) {
    *CONNECTOR.write().unwrap_or_else(PoisonError::into_inner) = connector;
}

/// How ssh may ask the user (see [`crate::askpass`]): set once by the app.
#[derive(Debug, Clone)]
pub struct Asking {
    /// `SSH_ASKPASS` and the variables our helper needs.
    pub env: Vec<(OsString, OsString)>,
    /// Where ssh keeps the shared connections (`ControlPath`).
    pub control_dir: PathBuf,
}

static ASKING: RwLock<Option<Asking>> = RwLock::new(None);

/// Lets ssh ask through the app and share one connection per host.
pub fn set_asking(asking: Asking) {
    *ASKING.write().unwrap_or_else(PoisonError::into_inner) = Some(asking);
}

/// How long a shared connection outlives its last use.
const CONTROL_PERSIST: &str = "ControlPersist=600";

/// `ssh -s host sftp`. With [`Asking`] set, ssh asks the user through the
/// app and, with `share`, shares one authenticated connection per host, so
/// a host asks once; without it ssh never asks (`BatchMode`).
pub fn ssh_command(host: &str, share: bool) -> Command {
    let asking = ASKING
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    ssh_command_with(host, asking.as_ref(), share)
}

fn ssh_command_with(host: &str, asking: Option<&Asking>, share: bool) -> Command {
    let mut command = crate::shell_path::command("ssh");
    command.args([
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
    ]);
    match asking {
        Some(asking) => {
            command.envs(asking.env.iter().map(|(k, v)| (k, v)));
            // Too long a folder: no sharing (each connection asks again)
            // rather than no connection at all.
            if !share {
                // Not even an existing shared connection (it may be dead).
                command.args(["-o", "ControlPath=none"]);
            } else if crate::askpass::fits_control_path(&asking.control_dir) {
                let path = asking.control_dir.join(control_name(std::process::id()));
                let mut control_path = OsString::from("ControlPath=");
                control_path.push(path.as_os_str());
                command
                    .args(["-o", "ControlMaster=auto", "-o", CONTROL_PERSIST, "-o"])
                    .arg(control_path);
            }
        }
        None => {
            command.args(["-o", "BatchMode=yes"]);
        }
    }
    command.args(["-s", "--", host, "sftp"]);
    command
}

/// The shared connections' socket name in the control folder: this app's
/// process id, so a quit closes only its own (another instance shares the
/// folder), then ssh's `%C` hash of the destination.
fn control_name(pid: u32) -> String {
    format!("cm-{pid}-%C")
}

/// Quit: ends this app's shared connections in `control_dir` (`ssh -O
/// exit`), which would otherwise outlive it by `ControlPersist`. Waits at
/// most two seconds in all.
pub fn close_shared(control_dir: &Path) {
    let mut running = Vec::new();
    for socket in own_sockets(control_dir, std::process::id()) {
        let mut command = crate::shell_path::command("ssh");
        command
            .arg("-S")
            .arg(&socket)
            .args(["-O", "exit", "shared"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(child) = command.spawn() {
            running.push(child);
        }
    }
    let until = std::time::Instant::now() + Duration::from_secs(2);
    for mut child in running {
        while matches!(child.try_wait(), Ok(None)) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The shared-connection sockets of process `pid` in `dir`.
fn own_sockets(dir: &Path, pid: u32) -> Vec<PathBuf> {
    let prefix = format!("cm-{pid}-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().as_bytes().starts_with(prefix.as_bytes()))
        .map(|e| e.path())
        .collect()
}

/// One SFTP session: the ssh process and the client on its pipes. Dropping
/// it ends the process.
#[derive(Debug)]
pub struct Session {
    pub client: Client,
    child: Child,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    /// What ssh wrote to its error output, as a message.
    fn stderr_text(&self) -> String {
        let bytes = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        ssh_message(&bytes)
    }

    /// `e` with ssh's own explanation, once the connection broke.
    pub fn explain(&mut self, e: io::Error) -> io::Error {
        if !self.client.is_broken() {
            return e;
        }
        // ssh has exited or is about to; its last words say why.
        let _ = self.child.kill();
        let _ = self.child.wait();
        match self.stderr_text() {
            text if text.is_empty() => e,
            text => io::Error::new(e.kind(), text),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// ssh's error output as a message: its lines, without the notice that a
/// host key was just added (it comes first and explains nothing).
fn ssh_message(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Starts a new session with `host`. Each file operation has its own, so a
/// long copy doesn't hold up browsing.
pub fn connect(host: &str) -> io::Result<Session> {
    if !valid_host(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a host name: {host}"),
        ));
    }
    let connector = *CONNECTOR.read().unwrap_or_else(PoisonError::into_inner);
    connect_with(host, connector(host, true))
}

/// [`connect`] without ssh's shared connection: after a kept session died,
/// the shared connection likely died with it, and a session through it
/// would hang just the same. A password host asks again.
pub fn connect_unshared(host: &str) -> io::Result<Session> {
    if !valid_host(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a host name: {host}"),
        ));
    }
    let connector = *CONNECTOR.read().unwrap_or_else(PoisonError::into_inner);
    connect_with(host, connector(host, false))
}

/// [`connect`] through `command`.
fn connect_with(host: &str, mut command: Command) -> io::Result<Session> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot start ssh: {e}")))?;
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let reading = keep_stderr(child.stderr.take().expect("piped"), stderr.clone());
    let reader = child.stdout.take().expect("piped");
    let writer = child.stdin.take().expect("piped");
    match Client::new(Box::new(reader), Box::new(writer)) {
        Ok(client) => Ok(Session {
            client,
            child,
            stderr,
        }),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reading.join();
            let text = ssh_message(&stderr.lock().unwrap_or_else(PoisonError::into_inner));
            Err(if no_sftp_server(&text) {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "{host} has no SFTP server: install one there \
                         (OpenSSH's sftp-server, e.g. the openssh-sftp-server package). ssh said: {text}"
                    ),
                )
            } else if !text.is_empty() {
                io::Error::new(io::ErrorKind::ConnectionRefused, text)
            } else if e.kind() == io::ErrorKind::InvalidData {
                // The classic cause: a login script that prints text.
                io::Error::new(
                    e.kind(),
                    format!("{e} (does the login shell on {host} print text?)"),
                )
            } else {
                e
            })
        }
    }
}

/// Whether ssh's words mean the host has no SFTP server: its configured
/// `sftp-server` is missing, or no SFTP subsystem is set up at all.
fn no_sftp_server(text: &str) -> bool {
    let text = text.to_lowercase();
    (text.contains("sftp-server") && (text.contains("not found") || text.contains("no such file")))
        || text.contains("subsystem request failed")
}

/// Collects `stderr` into `kept` (its last [`STDERR_KEPT`] bytes) until it
/// closes.
fn keep_stderr(
    mut stderr: impl Read + Send + 'static,
    kept: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0; 1024];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
            kept.extend_from_slice(&buf[..n]);
            let excess = kept.len().saturating_sub(STDERR_KEPT);
            kept.drain(..excess);
        }
    })
}

/// A host's shared session.
#[derive(Default)]
struct Slot {
    session: Mutex<Option<Session>>,
    /// Being connected (maybe waiting at a password prompt): others wait
    /// for it rather than connecting, and asking, themselves.
    connecting: AtomicBool,
}

/// The sessions panels browse with, one per host. A broken one is dropped
/// and the next use connects again.
type Sessions = Mutex<HashMap<String, Arc<Slot>>>;
static SESSIONS: std::sync::LazyLock<Sessions> = std::sync::LazyLock::new(Sessions::default);

/// How long a read waits for the shared session before it takes one of its
/// own: the one in use may be stuck on a dead connection (ssh gives up on
/// it after about 45 s). Not while the session is being made.
const BUSY_WAIT: Duration = Duration::from_secs(2);

/// Runs `f` on the shared session with `host`, connecting first if there is
/// none. Uses of one host wait for each other, up to [`BUSY_WAIT`] once
/// connected; other hosts don't. A kept session found dead (the computer
/// slept, the network changed) is replaced and `f` runs once more; a
/// session made for this call is not retried.
pub fn with_session<T>(
    host: &str,
    mut f: impl FnMut(&mut Client) -> io::Result<T>,
) -> io::Result<T> {
    let slot = SESSIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(host.to_owned())
        .or_default()
        .clone();
    let started = std::time::Instant::now();
    let mut session = loop {
        match slot.session.try_lock() {
            Ok(session) => break session,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock)
                if started.elapsed() >= BUSY_WAIT && !slot.connecting.load(Ordering::Relaxed) =>
            {
                // Usually a big listing, not a dead link: through the shared
                // connection, so a password host doesn't ask again.
                let mut own = connect(host)?;
                return f(&mut own.client).map_err(|e| own.explain(e));
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let kept = session.is_some();
    for attempt in 0..2 {
        if session.is_none() {
            slot.connecting.store(true, Ordering::Relaxed);
            // The retry after a dead session: not through the shared
            // connection, which likely died with it.
            let connected = if attempt == 0 {
                connect(host)
            } else {
                connect_unshared(host)
            };
            slot.connecting.store(false, Ordering::Relaxed);
            *session = Some(connected?);
        }
        let shared = session.as_mut().expect("connected above");
        let e = match f(&mut shared.client) {
            Ok(value) => return Ok(value),
            Err(e) => shared.explain(e),
        };
        if !shared.client.is_broken() {
            return Err(e);
        }
        *session = None;
        if !kept || attempt == 1 {
            return Err(e);
        }
    }
    unreachable!("the second attempt returns")
}

/// The time an SFTP `mtime` stands for.
pub(crate) fn time_of(mtime: u32) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(mtime))
}

/// Reads remote folder `path` for a panel.
pub(crate) fn read_listing(
    path: &Path,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> io::Result<Listing> {
    let location = parse(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a remote folder: {}", path.display()),
        )
    })?;
    let (dir, names, targets) = with_session(location.host, |client| {
        // Again on a retry: the count starts over with the listing.
        progress.store(0, Ordering::Relaxed);
        let dir = if location.path.is_empty() {
            client.realpath(b".")?
        } else {
            location.path.to_vec()
        };
        // Anything else would change what the panel path names.
        if dir.first() != Some(&b'/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the server gave a login folder that is not an absolute path",
            ));
        }
        let mut names = Vec::new();
        client.read_dir(&dir, |name| {
            if valid_name(&name.name) {
                progress.fetch_add(1, Ordering::Relaxed);
                names.push(name);
            }
        })?;
        if cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // A symlink shows as what it points to, like a local one: all of
        // them asked at once, not one round trip each.
        let links: Vec<&Name> = names.iter().filter(|n| n.attrs.is_symlink()).collect();
        let paths: Vec<Vec<u8>> = links.iter().map(|n| join(&dir, &n.name)).collect();
        let targets: HashMap<Vec<u8>, Attrs> = links
            .iter()
            .zip(client.stat_many(&paths)?)
            .filter_map(|(name, target)| Some((name.name.clone(), target.ok()?)))
            .collect();
        Ok((dir, names, targets))
    })?;
    let mut owners = HashMap::new();
    let mut entries = Vec::with_capacity(names.len() + 1);
    if !trim_end_slashes(&dir).is_empty() {
        entries.push(Entry::parent());
    }
    for name in names {
        let target = targets.get(&name.name).copied();
        entries.push(entry_of(name, target, &mut owners));
    }
    Ok(Listing {
        path: path_of(location.host, &dir),
        entries,
        archive: None,
        results: None,
        modified: None,
    })
}

/// The panel entry for a listed name; `target` is a symlink's target.
fn entry_of(name: Name, target: Option<Attrs>, owners: &mut HashMap<String, Arc<str>>) -> Entry {
    let is_symlink = name.attrs.is_symlink();
    let shown = if is_symlink {
        target.unwrap_or(name.attrs)
    } else {
        name.attrs
    };
    let kind = if shown.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let owner = owner_of(&name.long_name, name.attrs).map(|owner| {
        owners
            .entry(owner)
            .or_insert_with_key(|owner| owner.as_str().into())
            .clone()
    });
    Entry::listed(
        OsString::from_vec(name.name),
        kind,
        is_symlink,
        (kind == EntryKind::File).then_some(shown.size).flatten(),
        shown.mtime.map(time_of),
        name.attrs.permissions,
        owner,
    )
}

/// `user:group` from the server's `ls -l` line (OpenSSH writes the names
/// there), else the numeric ids.
fn owner_of(long_name: &[u8], attrs: Attrs) -> Option<String> {
    let line = String::from_utf8_lossy(long_name);
    let mut fields = line.split_whitespace();
    if let (Some(mode), Some(_links), Some(user), Some(group)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
        && mode.len() >= 10
    {
        return Some(format!("{user}:{group}"));
    }
    attrs.uid_gid.map(|(uid, gid)| format!("{uid}:{gid}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every remote host is this machine, through a local `sftp-server`;
    /// on host `dies-soon` the server ends after 0.3 s.
    pub(crate) fn use_local_server() {
        fn local(host: &str, _share: bool) -> Command {
            let server = crate::sftp::tests::server_program();
            if host == "dies-soon" {
                let mut command = Command::new("timeout");
                command.arg("0.3").arg(server);
                return command;
            }
            Command::new(server)
        }
        set_connector(local);
    }

    /// The remote panel path of local folder `dir` on test host `host`.
    pub(crate) fn remote(host: &str, dir: &Path) -> PathBuf {
        path_of(host, dir.as_os_str().as_bytes())
    }

    #[test]
    fn parses_hosts_and_paths() {
        let loc = parse(Path::new("sftp://me@box/home/me")).unwrap();
        assert_eq!(loc.host, "me@box");
        assert_eq!(loc.path, b"/home/me");
        assert_eq!(parse(Path::new("sftp://box")).unwrap().path, b"");
        assert_eq!(parse(Path::new("sftp://box/")).unwrap().path, b"/");
        assert!(parse(Path::new("sftp://-oProxyCommand=x/")).is_none());
        assert!(parse(Path::new("sftp:///x")).is_none());
        assert!(parse(Path::new("sftp://a b/x")).is_none());
        assert!(parse(Path::new("/home/me")).is_none());
        assert!(is_remote(Path::new("sftp://box")));
        assert!(!is_remote(Path::new("/sftp://box")));
        assert!(parse(Path::new("sftp://[::1]:22/x")).is_some());
    }

    #[test]
    fn a_remote_path_is_never_absolute_locally() {
        assert!(Path::new("sftp://box/etc").is_relative());
    }

    #[test]
    fn parent_and_file_name_stop_at_the_root() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(parent(&p("sftp://b/x/y")), Some(p("sftp://b/x")));
        assert_eq!(parent(&p("sftp://b/x/y/")), Some(p("sftp://b/x")));
        assert_eq!(
            parent(&p("sftp://b/x")).unwrap().as_os_str(),
            "sftp://b/",
            "the root keeps its slash"
        );
        assert_eq!(parent(&p("sftp://b/")), None);
        assert_eq!(parent(&p("sftp://b")), None);
        assert_eq!(file_name(&p("sftp://b/x/y")), Some("y".into()));
        assert_eq!(file_name(&p("sftp://b/")), None);
        assert_eq!(p("sftp://b/").join("x").as_os_str(), "sftp://b/x");
    }

    #[test]
    fn names_from_a_server_are_single_components() {
        assert!(valid_name(b"a.txt"));
        for bad in [&b""[..], b".", b"..", b"a/b", b"/etc", b"a\0b"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert_eq!(join(b"/", b"x"), b"/x");
        assert_eq!(join(b"/a/", b"x"), b"/a/x");
    }

    #[test]
    fn hosts_never_look_like_options() {
        assert!(valid_host("server"));
        assert!(valid_host("me@server.example.com"));
        assert!(!valid_host("-oProxyCommand=evil"));
        assert!(!valid_host(""));
        assert!(!valid_host("a;b"));
        assert!(!valid_host("a b"));
    }

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn without_asking_ssh_never_prompts_and_options_end_before_the_host() {
        let args = args(&ssh_command_with("box", None, true));
        assert!(args.iter().any(|a| a == "BatchMode=yes"));
        assert!(!args.iter().any(|a| a.starts_with("ControlMaster")));
        let dashes = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[dashes + 1..], ["box", "sftp"]);
    }

    #[test]
    fn with_asking_ssh_asks_through_the_app_and_shares_connections() {
        let asking = Asking {
            env: vec![("SSH_ASKPASS".into(), "/bin/helper".into())],
            control_dir: PathBuf::from("/run/yc"),
        };
        let command = ssh_command_with("box", Some(&asking), true);
        let args = args(&command);
        assert!(!args.iter().any(|a| a == "BatchMode=yes"));
        assert!(args.iter().any(|a| a == "ControlMaster=auto"));
        let own = format!("ControlPath=/run/yc/cm-{}-%C", std::process::id());
        assert!(args.contains(&own), "{args:?}");
        assert!(args.iter().any(|a| a == CONTROL_PERSIST));
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.contains(&("SSH_ASKPASS".as_ref(), Some("/bin/helper".as_ref()))));
        assert_eq!(args[args.len() - 3..], ["--", "box", "sftp"]);
    }

    #[test]
    fn reads_a_remote_folder() {
        use_local_server();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::os::unix::fs::symlink("sub", tmp.path().join("link")).unwrap();
        let path = remote("reads", tmp.path());
        let progress = AtomicUsize::new(0);
        let listing = read_listing(&path, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(listing.path, path);
        let mut entries: Vec<_> = listing
            .entries
            .iter()
            .map(|e| (e.label.as_str(), e.kind, e.is_symlink, e.size))
            .collect();
        entries.sort_by_key(|e| e.0);
        assert_eq!(
            entries,
            [
                ("..", EntryKind::Parent, false, None),
                ("a.txt", EntryKind::File, false, Some(5)),
                ("link", EntryKind::Dir, true, None),
                ("sub", EntryKind::Dir, false, None),
            ]
        );
        assert_eq!(progress.load(Ordering::Relaxed), 3);
        let file = listing.entries.iter().find(|e| e.label == "a.txt").unwrap();
        let user = nix::unistd::User::from_uid(nix::unistd::getuid())
            .unwrap()
            .unwrap()
            .name;
        assert!(
            file.owner
                .as_deref()
                .unwrap()
                .starts_with(&format!("{user}:"))
        );
        assert_eq!(file.mode.unwrap() & 0o170_000, 0o100_000);
        assert!(file.modified.is_some());
    }

    #[test]
    fn the_login_folder_and_the_root() {
        use_local_server();
        let none = AtomicBool::new(false);
        let home = read_listing(Path::new("sftp://login"), &AtomicUsize::new(0), &none).unwrap();
        let here = std::env::current_dir().unwrap();
        assert_eq!(home.path, remote("login", &here));
        let root = read_listing(Path::new("sftp://login/"), &AtomicUsize::new(0), &none).unwrap();
        assert_eq!(root.path.as_os_str(), "sftp://login/");
        assert!(root.entries.iter().all(|e| e.kind != EntryKind::Parent));
    }

    #[test]
    fn a_missing_remote_folder_is_not_found_and_the_session_stays() {
        use_local_server();
        let tmp = tempfile::tempdir().unwrap();
        let none = AtomicBool::new(false);
        let gone = remote("missing", &tmp.path().join("gone"));
        let err = read_listing(&gone, &AtomicUsize::new(0), &none).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let ok = read_listing(&remote("missing", tmp.path()), &AtomicUsize::new(0), &none);
        assert!(ok.is_ok());
    }

    #[test]
    fn a_failed_connection_says_what_ssh_said() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'Permission denied (publickey).' >&2; exit 255"]);
        let err = connect_with("x", command).unwrap_err();
        assert_eq!(err.to_string(), "Permission denied (publickey).");
    }

    #[test]
    fn the_host_key_notice_is_left_out_of_messages() {
        let stderr = b"Warning: Permanently added 'box' (ED25519) to the list of known hosts.\r\n\
                       Permission denied (publickey,password).\n";
        assert_eq!(
            ssh_message(stderr),
            "Permission denied (publickey,password)."
        );
    }

    #[test]
    fn an_unshared_session_uses_no_shared_connection() {
        let asking = Asking {
            env: Vec::new(),
            control_dir: PathBuf::from("/run/yc"),
        };
        let args = args(&ssh_command_with("box", Some(&asking), false));
        assert!(args.iter().any(|a| a == "ControlPath=none"));
        assert!(!args.iter().any(|a| a.starts_with("ControlMaster")));
    }

    #[test]
    fn a_too_long_control_folder_turns_sharing_off() {
        let asking = Asking {
            env: Vec::new(),
            control_dir: Path::new("/x").join("y".repeat(80)),
        };
        let args = args(&ssh_command_with("box", Some(&asking), true));
        assert!(!args.iter().any(|a| a.starts_with("Control")));
        assert!(!args.iter().any(|a| a == "BatchMode=yes"), "still asks");
    }

    #[test]
    fn a_host_without_an_sftp_server_says_so() {
        for said in [
            "sh: /opt/libexec/sftp-server: not found",
            "bash: /usr/lib/sftp-server: No such file or directory",
            "subsystem request failed on channel 0",
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", &format!("echo '{said}' >&2; exit 1")]);
            let err = connect_with("router", command).unwrap_err();
            let text = err.to_string();
            assert!(
                text.starts_with("router has no SFTP server: install one"),
                "{text}"
            );
            assert!(text.ends_with(&format!("ssh said: {said}")), "{text}");
        }
        assert!(!no_sftp_server("Permission denied (publickey)."));
    }

    #[test]
    fn a_quit_closes_only_this_apps_shared_connections() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["cm-42-abc", "cm-42-def", "cm-420-xyz", "cm-7-abc", "ask-42"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let mut own: Vec<_> = own_sockets(dir.path(), 42)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        own.sort();
        assert_eq!(own, ["cm-42-abc", "cm-42-def"]);
        assert!(own_sockets(&dir.path().join("gone"), 42).is_empty());
        // Not sockets: ssh fails on each, quickly, and nothing is left running.
        let started = std::time::Instant::now();
        close_shared(dir.path());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_login_script_that_prints_text_is_named() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'Welcome to box!'; cat >/dev/null"]);
        let err = connect_with("box", command).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("login shell on box"), "{err}");
    }

    #[test]
    fn a_busy_session_is_not_waited_for_long() {
        use_local_server();
        let tmp = tempfile::tempdir().unwrap();
        let path = remote("busy", tmp.path());
        let none = AtomicBool::new(false);
        read_listing(&path, &AtomicUsize::new(0), &none).unwrap();
        // Hold the shared session as a stuck read would.
        let (held, release) = std::sync::mpsc::channel::<()>();
        let (started, running) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            with_session("busy", |_| {
                started.send(()).unwrap();
                let _ = release.recv();
                Ok(())
            })
        });
        running.recv().unwrap();
        let begun = std::time::Instant::now();
        let listing = read_listing(&path, &AtomicUsize::new(0), &none).unwrap();
        assert_eq!(listing.path, path);
        assert!(begun.elapsed() < BUSY_WAIT * 3);
        held.send(()).unwrap();
        holder.join().unwrap().unwrap();
    }

    #[test]
    fn a_kept_session_that_died_is_replaced_without_an_error() {
        use_local_server();
        let tmp = tempfile::tempdir().unwrap();
        let path = remote("revive", tmp.path());
        let none = AtomicBool::new(false);
        read_listing(&path, &AtomicUsize::new(0), &none).unwrap();
        // The server behind the kept session goes away, as after a sleep.
        let slot = SESSIONS.lock().unwrap().get("revive").unwrap().clone();
        let child_id = {
            let mut session = slot.session.lock().unwrap();
            let session = session.as_mut().unwrap();
            let _ = session.child.kill();
            let _ = session.child.wait();
            session.child.id()
        };
        let listing = read_listing(&path, &AtomicUsize::new(0), &none).unwrap();
        assert_eq!(listing.path, path);
        let now = slot.session.lock().unwrap().as_ref().unwrap().child.id();
        assert_ne!(now, child_id, "a new session");
    }

    #[test]
    fn a_new_session_that_dies_is_not_retried() {
        use std::sync::atomic::AtomicU32;
        use_local_server();
        let calls = AtomicU32::new(0);
        let result = with_session("dies-soon", |client| {
            calls.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(600));
            client.stat(b"/")
        });
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn owners_come_from_the_long_name_or_the_ids() {
        let attrs = Attrs {
            uid_gid: Some((1000, 100)),
            ..Attrs::default()
        };
        assert_eq!(
            owner_of(
                b"-rw-r--r--    1 seb      users        5 Oct 27 12:00 a",
                attrs
            ),
            Some("seb:users".into())
        );
        assert_eq!(owner_of(b"a", attrs), Some("1000:100".into()));
        assert_eq!(owner_of(b"", Attrs::default()), None);
    }

    #[test]
    fn invalid_hosts_never_start_a_process() {
        let err = connect("-oProxyCommand=touch /tmp/x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
