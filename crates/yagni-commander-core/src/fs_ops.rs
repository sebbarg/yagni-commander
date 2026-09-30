//! Renaming and creating directories, with name validation.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Checks a single path component typed by the user.
fn validate_name(name: &str) -> io::Result<()> {
    if name.is_empty() {
        return Err(invalid("the name is empty"));
    }
    if name == "." || name == ".." {
        return Err(invalid(format!("“{name}” is not a valid name")));
    }
    if name.contains('/') || name.contains('\0') {
        return Err(invalid(format!("“{name}” contains “/”")));
    }
    Ok(())
}

/// True if both paths are the same file, e.g. `foo` and `FOO` on a
/// case-insensitive filesystem.
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_: &Path, _: &Path) -> bool {
    false
}

/// Renames `from` to `to` within `dir`. Refuses to replace an existing entry,
/// except when `to` is the same file (a case-only rename).
pub(crate) fn rename(dir: &Path, from: &OsStr, to: &str) -> io::Result<()> {
    validate_name(to)?;
    if from == OsStr::new(to) {
        return Ok(());
    }
    let source = dir.join(from);
    let target = dir.join(to);
    if target.symlink_metadata().is_ok() && !same_file(&source, &target) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("“{to}” already exists"),
        ));
    }
    fs::rename(&source, &target)
}

/// Creates `relative` (one or more `/`-separated names) inside `dir` and
/// returns its first component, the entry that appears in `dir`.
pub(crate) fn make_directory(dir: &Path, relative: &str) -> io::Result<OsString> {
    let relative = relative.trim_end_matches('/');
    if relative.is_empty() {
        return Err(invalid("the name is empty"));
    }
    if Path::new(relative).is_absolute() {
        return Err(invalid("give a name inside the current directory"));
    }
    for part in relative.split('/') {
        validate_name(part)?;
    }
    let target = dir.join(relative);
    if target.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("“{relative}” already exists"),
        ));
    }
    fs::create_dir_all(&target)?;
    match Path::new(relative).components().next() {
        Some(Component::Normal(first)) => Ok(first.to_os_string()),
        _ => unreachable!("validated names are normal components"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(result: io::Result<impl std::fmt::Debug>) -> io::ErrorKind {
        result.unwrap_err().kind()
    }

    #[test]
    fn rejects_invalid_names() {
        for name in ["", ".", "..", "a/b", "nul\0"] {
            assert_eq!(
                kind(validate_name(name)),
                io::ErrorKind::InvalidInput,
                "{name:?}"
            );
        }
        assert!(validate_name("ok name.txt").is_ok());
        assert!(validate_name(".hidden").is_ok());
    }

    #[test]
    fn renames_file_and_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), b"x").unwrap();
        fs::create_dir(tmp.path().join("dir")).unwrap();

        rename(tmp.path(), OsStr::new("a.txt"), "b.txt").unwrap();
        rename(tmp.path(), OsStr::new("dir"), "renamed").unwrap();
        assert_eq!(fs::read(tmp.path().join("b.txt")).unwrap(), b"x");
        assert!(tmp.path().join("renamed").is_dir());
        assert!(!tmp.path().join("a.txt").exists());
    }

    #[test]
    fn rename_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"a").unwrap();
        fs::write(tmp.path().join("b"), b"b").unwrap();
        let err = rename(tmp.path(), OsStr::new("a"), "b").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("“b” already exists"));
        assert_eq!(fs::read(tmp.path().join("b")).unwrap(), b"b");
    }

    #[test]
    fn rename_to_same_name_is_a_no_op_and_bad_names_fail() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), b"a").unwrap();
        rename(tmp.path(), OsStr::new("a"), "a").unwrap();
        assert_eq!(
            kind(rename(tmp.path(), OsStr::new("a"), "x/y")),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            kind(rename(tmp.path(), OsStr::new("missing"), "z")),
            io::ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn same_file_detects_hard_links_and_rejects_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        fs::write(&a, b"a").unwrap();
        fs::hard_link(&a, tmp.path().join("link")).unwrap();
        assert!(same_file(&a, &tmp.path().join("link")));
        assert!(!same_file(&a, &tmp.path().join("missing")));
    }

    #[test]
    fn makes_single_and_nested_directories() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(make_directory(tmp.path(), "new").unwrap(), "new");
        assert!(tmp.path().join("new").is_dir());
        assert_eq!(make_directory(tmp.path(), "a/b/c/").unwrap(), "a");
        assert!(tmp.path().join("a/b/c").is_dir());
    }

    #[test]
    fn make_directory_rejects_escapes_and_existing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("file"), b"").unwrap();
        for bad in ["", "/", "/etc/x", "../out", "a/../b", "a//b", "./a"] {
            assert_eq!(
                kind(make_directory(tmp.path(), bad)),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
        assert_eq!(
            kind(make_directory(tmp.path(), "file")),
            io::ErrorKind::AlreadyExists
        );
        assert!(!tmp.path().parent().unwrap().join("out").exists());
    }
}
