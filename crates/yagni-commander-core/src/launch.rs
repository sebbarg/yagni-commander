//! Starting external programs (the F4 editor).

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Runs `command` (split like a shell would, e.g. `code --wait` or
/// `"/Applications/My Editor.app/Contents/MacOS/editor"`) with `path` as its
/// last argument, without waiting for it to exit.
pub fn open_in_editor(command: &str, path: &Path) -> io::Result<()> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_owned());
    let mut words = shlex::split(command)
        .ok_or_else(|| invalid("the editor command has unbalanced quotes"))?
        .into_iter();
    let program = words
        .next()
        .ok_or_else(|| invalid("the editor command is empty"))?;
    let mut child = Command::new(&program)
        .args(words)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot start “{program}”: {e}")))?;
    // Reap it when it exits, so finished editors don't linger as zombies.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn passes_arguments_and_path() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let script = format!("sh -c 'echo \"$@\" > {}' sh --flag", out.display());
        open_in_editor(&script, Path::new("/some path/file")).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&out).map_or(true, |s| s.is_empty()) {
            assert!(Instant::now() < deadline, "editor did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "--flag /some path/file\n"
        );
    }

    #[test]
    fn reports_bad_commands() {
        let kind = |cmd: &str| open_in_editor(cmd, Path::new("/")).unwrap_err().kind();
        assert_eq!(kind(""), io::ErrorKind::InvalidInput);
        assert_eq!(kind("   "), io::ErrorKind::InvalidInput);
        assert_eq!(kind("code 'unclosed"), io::ErrorKind::InvalidInput);
        let err = open_in_editor("no-such-editor-xyz", Path::new("/")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("no-such-editor-xyz"));
    }
}
