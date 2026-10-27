//! Questions from ssh (passwords, key passphrases, an unknown host key)
//! answered in the app's own dialog.
//!
//! ssh asks through `SSH_ASKPASS`, a program it runs with the question as
//! its argument and whose output is the answer. That program is our own
//! binary: [`run_helper`] at the top of `main` sees [`SOCKET_VAR`] and
//! [`TOKEN_VAR`] in its environment, hands the question to the running app
//! over a Unix socket and prints the answer. The app's [`Server`] listens
//! on that socket, in a folder only the user can open, and checks the token
//! before it takes a question; the UI answers each [`Question`].
//!
//! Answers are never written anywhere but the helper's pipe to ssh. ssh's
//! connection sharing ([`crate::remote::ssh_command`]) keeps one
//! authenticated connection per host, so each host asks once.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

pub const SOCKET_VAR: &str = "YAGNI_ASKPASS_SOCKET";
pub const TOKEN_VAR: &str = "YAGNI_ASKPASS_TOKEN";

/// Longest question or answer taken (ssh's prompts are a few lines).
const MAX_MESSAGE: u64 = 64 * 1024;

/// What a question wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A password, passphrase or code: typed, never shown.
    Secret,
    /// Yes or no (an unknown host key, a key that asks to be confirmed).
    Confirm,
}

/// A question from ssh, waiting for the UI.
#[derive(Debug)]
pub struct Question {
    /// ssh's text, as it gave it (may span lines).
    pub prompt: String,
    pub kind: Kind,
    reply: Sender<Option<String>>,
}

impl Question {
    /// A question the UI answers through `reply` (app tests).
    pub fn new(prompt: &str, kind: Kind) -> (Self, Receiver<Option<String>>) {
        let (reply, answer) = mpsc::channel();
        (
            Self {
                prompt: prompt.to_owned(),
                kind,
                reply,
            },
            answer,
        )
    }

    /// The answer; `None` cancels (ssh then fails or tries something else).
    /// A [`Kind::Confirm`] answers `Some("yes".into())` for yes.
    pub fn answer(self, answer: Option<String>) {
        let _ = self.reply.send(answer);
    }
}

/// How ssh asks: `yes/no` questions are confirmations, the rest secrets.
/// `SSH_ASKPASS_PROMPT=confirm` (a key added with `ssh-add -c`) also is.
pub fn kind_of(prompt: &str, prompt_var: Option<&str>) -> Kind {
    if prompt_var == Some("confirm") || prompt.contains("(yes/no") {
        Kind::Confirm
    } else {
        Kind::Secret
    }
}

/// The folder for the socket and ssh's shared connections: private to the
/// user and with a short path. `$XDG_RUNTIME_DIR/yagni-commander` on Linux,
/// else (or when that is too long, see [`fits_control_path`]) in the temp
/// folder (per user on macOS), else `/tmp`.
pub fn runtime_dir() -> PathBuf {
    let uid = nix::unistd::getuid();
    let candidates = [
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(|dir| PathBuf::from(dir).join("yagni-commander")),
        Some(std::env::temp_dir().join(format!("yc-{uid}"))),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| fits_control_path(dir))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/yc-{uid}")))
}

/// Makes `dir` the user's private folder, or checks that it is: a real
/// folder (not a symlink), owned by the user, readable by nobody else. In
/// a shared `/tmp` another user could have made it first, to read the
/// questions or plant ssh's connection sockets; then it is refused.
pub fn secure_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if let Some(parent) = dir.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let refused = |why: &str| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: {why}", dir.display()),
        )
    };
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(refused("not a folder of its own"));
    }
    if meta.uid() != nix::unistd::getuid().as_raw() {
        return Err(refused("owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The longest Unix socket path, without its final NUL (macOS's `sun_path`
/// is 104 bytes, Linux's 108).
const SOCKET_PATH_MAX: usize = 103;

/// Whether ssh's shared-connection sockets fit in `dir`: `cm-` and the
/// 40-character `%C` hash, plus the 17 characters ssh adds to a socket
/// while it sets one up.
pub fn fits_control_path(dir: &Path) -> bool {
    dir.as_os_str().len() + "/cm-".len() + 40 + 17 <= SOCKET_PATH_MAX
}

/// The app's end: owns the socket (removed when dropped). The questions
/// arrive on the receiver [`Server::start`] returns.
pub struct Server {
    socket: PathBuf,
    token: String,
}

impl Server {
    /// Listens on a socket of this process in `dir` (see [`secure_dir`]).
    pub fn start(dir: &Path) -> io::Result<(Self, Receiver<Question>)> {
        secure_dir(dir)?;
        let socket = dir.join(format!("ask-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        let token = random_token()?;
        let (send, questions) = mpsc::channel();
        let expected = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (send, expected) = (send.clone(), expected.clone());
                // One thread per question: it waits for the user.
                std::thread::spawn(move || serve(stream, &expected, &send));
            }
        });
        Ok((Self { socket, token }, questions))
    }

    /// The environment ssh needs to ask through this server.
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        let helper = std::env::current_exe().unwrap_or_default();
        vec![
            ("SSH_ASKPASS".into(), helper.into()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (SOCKET_VAR.into(), self.socket.clone().into()),
            (TOKEN_VAR.into(), self.token.clone().into()),
        ]
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// One helper's question: token, prompt and kind in, the answer out.
fn serve(mut stream: UnixStream, expected: &str, send: &Sender<Question>) {
    let mut message = Vec::new();
    if (&mut stream)
        .take(MAX_MESSAGE)
        .read_to_end(&mut message)
        .is_err()
    {
        return;
    }
    let mut parts = message.split(|&b| b == 0);
    let (Some(token), Some(prompt), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
        return;
    };
    if token != expected.as_bytes() {
        return;
    }
    let kind = if kind == b"confirm" {
        Kind::Confirm
    } else {
        Kind::Secret
    };
    let (question, answer) = Question::new(&String::from_utf8_lossy(prompt), kind);
    if send.send(question).is_err() {
        return;
    }
    let reply = match answer.recv() {
        Ok(Some(text)) => [b"1".as_slice(), text.as_bytes()].concat(),
        _ => b"0".to_vec(),
    };
    let _ = stream.write_all(&reply);
}

/// Asks the app at `socket`: `Ok(None)` when the user cancelled.
pub fn ask(socket: &Path, token: &str, prompt: &str, kind: Kind) -> io::Result<Option<String>> {
    let mut stream = UnixStream::connect(socket)?;
    let kind = match kind {
        Kind::Confirm => "confirm",
        Kind::Secret => "secret",
    };
    let message = [token.as_bytes(), prompt.as_bytes(), kind.as_bytes()].join(&0);
    stream.write_all(&message)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = Vec::new();
    stream.take(MAX_MESSAGE).read_to_end(&mut reply)?;
    Ok(match reply.split_first() {
        Some((b'1', answer)) => Some(String::from_utf8_lossy(answer).into_owned()),
        _ => None,
    })
}

/// `main`'s first step: when ssh runs this binary as its askpass helper,
/// asks the app and returns the exit code (0 with the answer printed, 1 when
/// cancelled or the app is gone). `None`: a normal start.
pub fn run_helper() -> Option<i32> {
    let socket = std::env::var_os(SOCKET_VAR)?;
    let token = std::env::var(TOKEN_VAR).ok()?;
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let prompt_var = std::env::var("SSH_ASKPASS_PROMPT").ok();
    let kind = kind_of(&prompt, prompt_var.as_deref());
    match ask(Path::new(&socket), &token, &prompt, kind) {
        Ok(Some(answer)) => {
            let mut out = io::stdout().lock();
            let _ = writeln!(out, "{answer}");
            Some(0)
        }
        _ => Some(1),
    }
}

/// 128 random bits as hex.
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_reaches_the_server_and_the_answer_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let (server, questions) = Server::start(&dir.path().join("run")).unwrap();
        let socket = server.socket.clone();
        let token = server.token.clone();
        let asking = std::thread::spawn(move || {
            ask(&socket, &token, "me@box's password: ", Kind::Secret).unwrap()
        });
        let question = questions.recv().unwrap();
        assert_eq!(question.prompt, "me@box's password: ");
        assert_eq!(question.kind, Kind::Secret);
        question.answer(Some("hunter2".into()));
        assert_eq!(asking.join().unwrap().as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_cancelled_question_answers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (server, questions) = Server::start(dir.path()).unwrap();
        let (socket, token) = (server.socket.clone(), server.token.clone());
        let asking =
            std::thread::spawn(move || ask(&socket, &token, "Continue (yes/no)?", Kind::Confirm));
        let question = questions.recv().unwrap();
        assert_eq!(question.kind, Kind::Confirm);
        question.answer(None);
        assert_eq!(asking.join().unwrap().unwrap(), None);
    }

    #[test]
    fn a_wrong_token_is_never_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (server, questions) = Server::start(dir.path()).unwrap();
        let answer = ask(&server.socket, "wrong", "password: ", Kind::Secret).unwrap();
        assert_eq!(answer, None);
        assert!(questions.try_recv().is_err());
    }

    #[test]
    fn the_folder_is_private_and_the_socket_goes_with_the_server() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let (server, _questions) = Server::start(&run).unwrap();
        let mode = std::fs::metadata(&run).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let socket = server.socket.clone();
        assert!(socket.exists());
        let env = server.env();
        assert!(
            env.iter()
                .any(|(k, v)| k == "SSH_ASKPASS_REQUIRE" && v == "force")
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == SOCKET_VAR && v == socket.as_os_str())
        );
        drop(server);
        assert!(!socket.exists());
    }

    #[test]
    fn yes_no_questions_are_confirmations() {
        let host_key = "The authenticity of host 'box' can't be established.\n\
                        Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(kind_of(host_key, None), Kind::Confirm);
        assert_eq!(kind_of("Allow use of key?", Some("confirm")), Kind::Confirm);
        assert_eq!(
            kind_of("Enter passphrase for key '/k': ", None),
            Kind::Secret
        );
        assert_eq!(kind_of("Verification code: ", Some("")), Kind::Secret);
    }

    #[test]
    fn control_paths_must_fit_a_socket() {
        assert!(fits_control_path(Path::new(
            "/run/user/1000/yagni-commander"
        )));
        assert!(!fits_control_path(&Path::new("/x").join("y".repeat(60))));
        assert!(fits_control_path(&runtime_dir()) || runtime_dir().starts_with("/tmp"));
    }

    #[test]
    fn a_folder_that_is_a_symlink_or_open_to_others_is_handled() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let err = secure_dir(&link).unwrap_err();
        assert!(
            err.to_string().ends_with("not a folder of its own"),
            "{err}"
        );
        assert!(Server::start(&link).is_err());
        let open = tmp.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        secure_dir(&open).unwrap();
        let mode = std::fs::metadata(&open).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        secure_dir(&tmp.path().join("a/b")).unwrap();
        assert!(tmp.path().join("a/b").is_dir());
    }

    #[test]
    fn no_app_no_answer() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ask(&dir.path().join("gone"), "t", "p", Kind::Secret).is_err());
    }

    #[test]
    fn tokens_are_random() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
