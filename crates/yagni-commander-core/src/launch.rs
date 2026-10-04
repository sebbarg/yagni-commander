//! Starting external programs: the F4 editor, and for Enter on a file
//! either the file itself (a program) or the system's opener.

use std::io;
use std::path::Path;
use std::process::Stdio;

use crate::shell_path;

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
    let mut child = shell_path::command(&program)
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

/// The platform's "open with the default application" command. Never runs
/// a file itself: the opener decides (a script usually opens in an editor).
pub const SYSTEM_OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};

/// Enter on a file: runs `opener` (normally [`SYSTEM_OPENER`]) with `path`,
/// without waiting. Failing to start is an error at once; an opener that
/// exits unsuccessfully (no application for the file type) is reported to
/// `on_failure` from a background thread. Its output is not captured: a
/// program it starts could hold the pipe open for its whole lifetime.
pub fn open_with(
    opener: &str,
    path: &Path,
    on_failure: impl FnOnce(String) + Send + 'static,
) -> io::Result<()> {
    let mut child = shell_path::command(opener)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot start “{opener}”: {e}")))?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let opener = opener.to_owned();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let how = status.code().map_or_else(
                || status.to_string(),
                |code| format!("exited with code {code}"),
            );
            on_failure(format!(
                "No application could open “{name}” ({opener} {how})."
            ))
        }
        Err(e) => on_failure(format!("Opening “{name}” failed: {e}")),
    });
    Ok(())
}

/// Whether Enter runs `path` instead of opening it: an executable regular
/// file (any execute bit, links followed) that starts like a program, an
/// ELF or Mach-O binary or a `#!` script. The execute bit alone is not
/// enough: NTFS, FAT and SMB mounts set it on every file.
pub fn is_program(path: &Path) -> bool {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let mut head = [0u8; 4];
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut read = 0;
    let mut file = file.take(4);
    while let Ok(n) = file.read(&mut head[read..]) {
        if n == 0 {
            break;
        }
        read += n;
    }
    let head = &head[..read];
    const MAGIC: [&[u8]; 6] = [
        b"\x7fELF",
        &[0xfe, 0xed, 0xfa, 0xce], // Mach-O 32-bit
        &[0xfe, 0xed, 0xfa, 0xcf], // Mach-O 64-bit
        &[0xce, 0xfa, 0xed, 0xfe], // the same, little-endian
        &[0xcf, 0xfa, 0xed, 0xfe],
        &[0xca, 0xfe, 0xba, 0xbe], // universal (fat) binary
    ];
    head.starts_with(b"#!") || MAGIC.contains(&head)
}

/// Enter on a program: runs it directly (no shell), in `dir`, without
/// waiting. Only failing to start is an error; how it exits is its own
/// business (a GUI app may exit non-zero when closed).
pub fn run_program(path: &Path, dir: &Path) -> io::Result<()> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut child = shell_path::command(path)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot run “{name}”: {e}")))?;
    // Reap it when it exits, so finished programs don't linger as zombies.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;
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

    /// Another test's fork may still hold a freshly written script open for
    /// writing, so exec fails with "text file busy" (ETXTBSY) for a moment.
    /// Runs it once until that is over.
    fn warm_up(script: &Path) {
        for _ in 0..200 {
            match Command::new(script)
                .arg("warm-up")
                .current_dir(script.parent().unwrap())
                .status()
            {
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                _ => break,
            }
        }
    }

    /// Waits for `on_failure`'s message, or `None` once the opener's
    /// thread has dropped it (success).
    fn failure(rx: std::sync::mpsc::Receiver<String>) -> Option<String> {
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    #[test]
    fn the_opener_gets_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let opener = tmp.path().join("opener");
        std::fs::write(
            &opener,
            format!("#!/bin/sh\necho \"$1\" > {}\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        warm_up(&opener);
        std::fs::remove_file(&out).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let file = Path::new("/some path/file.pdf");
        open_with(opener.to_str().unwrap(), file, move |m| tx.send(m).unwrap()).unwrap();
        assert_eq!(failure(rx), None);
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "/some path/file.pdf\n"
        );
    }

    #[test]
    fn an_opener_that_fails_reports_it() {
        let (tx, rx) = std::sync::mpsc::channel();
        open_with("false", Path::new("/x/doc.odd"), move |m| {
            tx.send(m).unwrap()
        })
        .unwrap();
        let message = failure(rx).expect("reported");
        assert!(message.contains("doc.odd"), "{message}");
        assert!(message.contains("false exited with code 1"), "{message}");
    }

    #[test]
    fn a_missing_opener_is_an_error_at_once() {
        let err = open_with("no-such-opener-xyz", Path::new("/x"), |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("no-such-opener-xyz"));
    }

    fn file(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(mode))
            .unwrap();
        path
    }

    #[test]
    fn programs_are_executable_binaries_and_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert!(is_program(&file(d, "elf", b"\x7fELF\x02\x01", 0o755)));
        assert!(is_program(&file(
            d,
            "macho",
            &[0xcf, 0xfa, 0xed, 0xfe, 7],
            0o755
        )));
        assert!(is_program(&file(
            d,
            "fat",
            &[0xca, 0xfe, 0xba, 0xbe, 0],
            0o755
        )));
        assert!(is_program(&file(d, "script", b"#!/bin/sh\necho\n", 0o744)));
        // Executable only for the group or others still counts.
        assert!(is_program(&file(d, "group", b"#!/bin/sh\n", 0o650)));
        let link = d.join("link");
        std::os::unix::fs::symlink(d.join("elf"), &link).unwrap();
        assert!(is_program(&link), "a link to a program");
    }

    #[test]
    fn the_execute_bit_alone_is_not_enough() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        // NTFS, FAT and SMB mounts mark every file executable.
        assert!(!is_program(&file(d, "doc.pdf", b"%PDF-1.7", 0o777)));
        assert!(!is_program(&file(d, "empty", b"", 0o755)));
        assert!(!is_program(&file(d, "short", b"#", 0o755)));
        // A program without the execute bit is opened, not run.
        assert!(!is_program(&file(d, "plain-elf", b"\x7fELF\x02", 0o644)));
        assert!(!is_program(&file(d, "plain-script", b"#!/bin/sh\n", 0o644)));
        std::fs::create_dir(d.join("dir")).unwrap();
        assert!(!is_program(&d.join("dir")));
        assert!(!is_program(&d.join("missing")));
    }

    #[test]
    fn a_program_runs_in_the_given_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let script = file(tmp.path(), "prog", b"#!/bin/sh\npwd > out\n", 0o755);
        warm_up(&script);
        let _ = std::fs::remove_file(tmp.path().join("out"));
        run_program(&script, tmp.path()).unwrap();
        let out = tmp.path().join("out");
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&out).map_or(true, |s| s.is_empty()) {
            assert!(Instant::now() < deadline, "program did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
        let pwd = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            Path::new(pwd.trim_end()).canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        // Looks like a script, but its interpreter doesn't exist.
        let script = file(tmp.path(), "broken", b"#!/no/such/interpreter\n", 0o755);
        warm_up(&script);
        let err = run_program(&script, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("broken"), "{err}");
    }

    #[test]
    fn the_system_opener_is_the_platform_one() {
        let expected = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(SYSTEM_OPENER, expected);
    }
}
