//! Renaming and creating directories, with name validation, and a rename
//! that never replaces an existing entry.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

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
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
pub(crate) fn same_file(_: &Path, _: &Path) -> bool {
    false
}

/// Renames `from` to `to`, failing with `AlreadyExists` if `to` exists.
///
/// Atomic on Linux (`renameat2` with `RENAME_NOREPLACE`). Elsewhere, and on
/// Linux filesystems without that flag, it checks and then renames, so another
/// process could still create `to` in between.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use nix::errno::Errno;
        use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
        match renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE) {
            Err(Errno::EINVAL) => {} // not supported by this filesystem
            result => return result.map_err(io::Error::from),
        }
    }
    if to.symlink_metadata().is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    fs::rename(from, to)
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
    if same_file(&source, &target) {
        return fs::rename(&source, &target);
    }
    rename_noreplace(&source, &target).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => io::Error::new(e.kind(), format!("“{to}” already exists")),
        _ => e,
    })
}

/// Checks a `/`-separated path typed by the user (F7, Shift-F4): relative,
/// and every part a valid name, so nothing lands outside the directory.
fn validate_relative(relative: &str) -> io::Result<()> {
    if relative.is_empty() {
        return Err(invalid("the name is empty"));
    }
    if Path::new(relative).is_absolute() {
        return Err(invalid("give a name inside the current directory"));
    }
    relative.split('/').try_for_each(validate_name)
}

/// The first part of a validated relative path: the entry that appears in
/// the directory, for the cursor.
fn first_component(relative: &str) -> OsString {
    match Path::new(relative).components().next() {
        Some(Component::Normal(first)) => first.to_os_string(),
        _ => unreachable!("validated names are normal components"),
    }
}

/// What Shift-F4 made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NewFile {
    pub path: PathBuf,
    /// False if the file already existed (it is then just opened).
    pub created: bool,
    /// The entry in the directory to put the cursor on.
    pub first: OsString,
}

/// Creates an empty file at `relative` (a name, or a path like `a/b/c.txt`
/// whose missing directories are created) inside `dir`. An existing file is
/// left as it is (Shift-F4 then just opens it); a directory is an error.
pub(crate) fn create_file(dir: &Path, relative: &str) -> io::Result<NewFile> {
    validate_relative(relative)?;
    let path = dir.join(relative);
    if let Some(parent) = path.parent().filter(|p| *p != dir) {
        fs::create_dir_all(parent)?;
    }
    let created = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && !path.is_dir() => false,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::Error::new(
                e.kind(),
                format!("“{relative}” is a directory"),
            ));
        }
        Err(e) => return Err(e),
    };
    Ok(NewFile {
        path,
        created,
        first: first_component(relative),
    })
}

/// Creates `relative` (one or more `/`-separated names) inside `dir` and
/// returns its first component, the entry that appears in `dir`.
pub(crate) fn make_directory(dir: &Path, relative: &str) -> io::Result<OsString> {
    let relative = relative.trim_end_matches('/');
    validate_relative(relative)?;
    let target = dir.join(relative);
    if target.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("“{relative}” already exists"),
        ));
    }
    fs::create_dir_all(&target)?;
    Ok(first_component(relative))
}

/// The first readable directory among `path` and its ancestors, else
/// `home`. Used to reopen a remembered folder that may have been deleted or
/// locked since.
pub fn nearest_dir(path: &Path, home: &Path) -> PathBuf {
    path.ancestors()
        .find(|p| !p.as_os_str().is_empty() && std::fs::read_dir(p).is_ok())
        .map_or_else(|| home.to_path_buf(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
    fn rename_noreplace_moves_or_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("c"),
        );
        fs::write(&a, b"a").unwrap();
        fs::create_dir(&b).unwrap(); // empty: plain rename(2) would replace it
        assert_eq!(kind(rename_noreplace(&a, &b)), io::ErrorKind::AlreadyExists);
        rename_noreplace(&a, &c).unwrap();
        assert!(!a.exists() && c.is_file() && b.is_dir());
        assert_eq!(kind(rename_noreplace(&a, &c)), io::ErrorKind::NotFound);
    }

    #[test]
    fn create_file_makes_an_empty_file_or_keeps_an_existing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let new = create_file(tmp.path(), "new.txt").unwrap();
        assert!(new.created);
        assert_eq!(new.path, tmp.path().join("new.txt"));
        assert_eq!(new.first, "new.txt");
        assert_eq!(fs::read(&new.path).unwrap(), b"");
        fs::write(&new.path, b"keep").unwrap();
        assert!(!create_file(tmp.path(), "new.txt").unwrap().created);
        assert_eq!(fs::read(&new.path).unwrap(), b"keep");

        fs::create_dir(tmp.path().join("dir")).unwrap();
        assert_eq!(
            kind(create_file(tmp.path(), "dir")),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            kind(create_file(&tmp.path().join("missing"), "x")),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn create_file_accepts_a_relative_path_and_makes_its_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let new = create_file(tmp.path(), "notes/2026/todo.md").unwrap();
        assert!(new.created);
        assert_eq!(new.path, tmp.path().join("notes/2026/todo.md"));
        assert_eq!(new.first, "notes");
        assert!(new.path.is_file());
        // An existing directory on the way is fine.
        assert!(create_file(tmp.path(), "notes/other.md").unwrap().created);
        for bad in ["/etc/x", "../x", "a/../../x", "a//b", "a/", ""] {
            assert_eq!(
                kind(create_file(tmp.path(), bad)),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
        fs::write(tmp.path().join("file"), b"").unwrap();
        assert!(
            create_file(tmp.path(), "file/x").is_err(),
            "a file in the way"
        );
        assert!(!tmp.path().join("x").exists());
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

    #[test]
    fn nearest_dir_keeps_a_readable_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(nearest_dir(tmp.path(), Path::new("/home")), tmp.path());
    }

    #[test]
    fn nearest_dir_walks_up_from_a_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("a/b/c");
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        assert_eq!(nearest_dir(&gone, Path::new("/home")), tmp.path().join("a"));
    }

    #[test]
    fn nearest_dir_skips_files_and_unreadable_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(nearest_dir(&file, Path::new("/home")), tmp.path());

        let locked = tmp.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&locked).is_ok(); // root reads anything
        let found = nearest_dir(&locked, Path::new("/home"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !readable {
            assert_eq!(found, tmp.path());
        }
    }

    #[test]
    fn nearest_dir_falls_back_to_home() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            nearest_dir(Path::new("relative/missing"), home.path()),
            home.path()
        );
        assert_eq!(nearest_dir(Path::new(""), home.path()), home.path());
    }
}
