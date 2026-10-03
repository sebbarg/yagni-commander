//! Writes below one folder (an extraction target) without ever following a
//! symlink: every folder on the way is opened from the root down with
//! `O_NOFOLLOW`, and files are created with `O_NOFOLLOW | O_EXCL`. An
//! archive that creates `a -> /etc` and then writes `a/passwd` gets an
//! error instead of writing to /etc.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nix::dir::Dir;
use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat, renameat};
use nix::sys::stat::{Mode, SFlag, fchmod, fstat, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, linkat, symlinkat, unlinkat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

pub(crate) struct SafeDir {
    root: Dir,
    path: PathBuf,
}

const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

const FILE_FLAGS: OFlag = OFlag::O_WRONLY
    .union(OFlag::O_CREAT)
    .union(OFlag::O_EXCL)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// A nix error as an io error. A link or a file where a folder is needed
/// (`O_NOFOLLOW | O_DIRECTORY` gives ENOTDIR for both, ELOOP on some
/// systems) gets a readable message.
fn io_err(e: Errno) -> io::Error {
    match e {
        Errno::ELOOP | Errno::ENOTDIR => {
            io::Error::other("a symbolic link or a file is in the way")
        }
        e => io::Error::from(e),
    }
}

impl SafeDir {
    /// The extraction folder itself may be reached through links: it is
    /// the folder the user chose.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let root = Dir::open(
            path,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        Ok(Self {
            root,
            path: path.to_path_buf(),
        })
    }

    pub(crate) fn path_of(&self, parts: &[OsString]) -> PathBuf {
        parts
            .iter()
            .fold(self.path.clone(), |path, part| path.join(part))
    }

    /// The open folder `parts`, each step opened without following links;
    /// with `create`, missing folders are made on the way.
    pub(crate) fn dir(&self, parts: &[OsString], create: bool) -> io::Result<Dir> {
        let mut current = Dir::openat(&self.root, ".", DIR_FLAGS, Mode::empty()).map_err(io_err)?;
        for part in parts {
            check_part(part)?;
            if create {
                match mkdirat(&current, part.as_os_str(), Mode::from_bits_truncate(0o777)) {
                    Ok(()) | Err(Errno::EEXIST) => {}
                    Err(e) => return Err(io_err(e)),
                }
            }
            current = Dir::openat(&current, part.as_os_str(), DIR_FLAGS, Mode::empty())
                .map_err(io_err)?;
        }
        Ok(current)
    }

    /// The open folder holding `parts`' last component, and that name.
    fn parent<'p>(&self, parts: &'p [OsString], create: bool) -> io::Result<(Dir, &'p OsStr)> {
        let (name, folders) = parts
            .split_last()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        check_part(name)?;
        Ok((self.dir(folders, create)?, name.as_os_str()))
    }

    /// What is at `parts`, without following a link there; `None` if
    /// nothing (or a folder on the way is missing).
    pub(crate) fn kind(&self, parts: &[OsString]) -> io::Result<Option<Kind>> {
        let (dir, name) = match self.parent(parts, false) {
            Ok(found) => found,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match fstatat(&dir, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(
                match SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT {
                    SFlag::S_IFREG => Kind::File,
                    SFlag::S_IFDIR => Kind::Dir,
                    SFlag::S_IFLNK => Kind::Symlink,
                    _ => Kind::Other,
                },
            )),
            Err(Errno::ENOENT) => Ok(None),
            Err(e) => Err(io_err(e)),
        }
    }

    /// A new file (missing folders created); `mode`'s rwx bits only.
    pub(crate) fn create_file(&self, parts: &[OsString], mode: Option<u32>) -> io::Result<File> {
        let (dir, name) = self.parent(parts, true)?;
        create_in(&dir, name, mode)
    }

    /// A new hidden file next to `parts` (`.<name>.yagni-<n>.tmp`), to be
    /// renamed over it with [`SafeDir::replace`].
    pub(crate) fn create_temp_beside(
        &self,
        parts: &[OsString],
        mode: Option<u32>,
    ) -> io::Result<(OsString, File)> {
        let (dir, name) = self.parent(parts, true)?;
        for n in 0..100 {
            let mut temp = OsString::from(".");
            temp.push(name);
            temp.push(format!(".yagni-{n}.tmp"));
            match create_in(&dir, &temp, mode) {
                Ok(file) => return Ok((temp, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }

    pub(crate) fn replace(&self, parts: &[OsString], temp: &OsStr) -> io::Result<()> {
        let (dir, name) = self.parent(parts, false)?;
        renameat(&dir, temp, &dir, name).map_err(io_err)
    }

    pub(crate) fn remove_file(&self, parts: &[OsString]) -> io::Result<()> {
        let (dir, name) = self.parent(parts, false)?;
        unlinkat(&dir, name, UnlinkatFlags::NoRemoveDir).map_err(io_err)
    }

    /// Removes the temp file `temp` next to `parts` (a cancelled overwrite).
    pub(crate) fn remove_temp(&self, parts: &[OsString], temp: &OsStr) -> io::Result<()> {
        let (dir, _) = self.parent(parts, false)?;
        unlinkat(&dir, temp, UnlinkatFlags::NoRemoveDir).map_err(io_err)
    }

    pub(crate) fn symlink(&self, parts: &[OsString], target: &OsStr) -> io::Result<()> {
        let (dir, name) = self.parent(parts, true)?;
        symlinkat(target, &dir, name).map_err(io_err)
    }

    /// A hard link at `parts` to the file at `existing`, neither followed.
    pub(crate) fn hard_link(&self, existing: &[OsString], parts: &[OsString]) -> io::Result<()> {
        let (from_dir, from_name) = self.parent(existing, false)?;
        let (to_dir, to_name) = self.parent(parts, true)?;
        linkat(&from_dir, from_name, &to_dir, to_name, AtFlags::empty()).map_err(io_err)
    }

    /// For a folder after its contents are in (they would change its time,
    /// and a read-only mode would block them). Best effort.
    pub(crate) fn set_dir_mode_and_time(
        &self,
        parts: &[OsString],
        mode: Option<u32>,
        modified: Option<SystemTime>,
    ) {
        let Ok(dir) = self.dir(parts, false) else {
            return;
        };
        let Ok(fd) = openat(
            &dir,
            ".",
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) else {
            return;
        };
        if let Some(mode) = mode {
            // The folder was made with 0o777 minus the umask: keep that mask.
            let made =
                fstat(&fd).map_or(Mode::all(), |stat| Mode::from_bits_truncate(stat.st_mode));
            let _ = fchmod(&fd, rwx(mode) & made);
        }
        if let Some(modified) = modified {
            let _ = File::from(fd).set_modified(modified);
        }
    }
}

/// The rwx bits of an archive's mode (`mode_t` is u16 on macOS).
fn rwx(mode: u32) -> Mode {
    Mode::from_bits_truncate((mode & 0o777) as _)
}

/// One path component: never empty, `.` or `..`, which would name the
/// folder itself or its parent (`O_NOFOLLOW` doesn't stop `..`).
fn check_part(part: &OsStr) -> io::Result<()> {
    match part.as_encoded_bytes() {
        b"" | b"." | b".." => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "“..” in an archive path is not allowed",
        )),
        _ => Ok(()),
    }
}

/// Creates `name` in `dir`: never through a link, never over an existing
/// entry. With `mode`, its rwx bits minus the umask (setuid and friends are
/// dropped), like tar and unzip; without, 0o666 minus the umask.
fn create_in(dir: &Dir, name: &OsStr, mode: Option<u32>) -> io::Result<File> {
    let fd = openat(dir, name, FILE_FLAGS, rwx(mode.unwrap_or(0o666))).map_err(io_err)?;
    Ok(File::from(fd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn p(path: &str) -> Vec<OsString> {
        path.split('/').map(OsString::from).collect()
    }

    #[test]
    fn creates_folders_and_files_below_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        root.dir(&p("a/b"), true).unwrap();
        let mut f = root.create_file(&p("a/b/c.txt"), Some(0o4755)).unwrap();
        f.write_all(b"hi").unwrap();
        let file = tmp.path().join("a/b/c.txt");
        assert_eq!(fs::read(&file).unwrap(), b"hi");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o755,
            "setuid dropped"
        );
        assert_eq!(root.kind(&p("a/b/c.txt")).unwrap(), Some(Kind::File));
        assert_eq!(root.kind(&p("a/b")).unwrap(), Some(Kind::Dir));
        assert_eq!(root.kind(&p("a/x")).unwrap(), None);
        assert_eq!(root.kind(&p("missing/x")).unwrap(), None);
        assert_eq!(root.path_of(&p("a/b")), tmp.path().join("a/b"));
        // Missing folders on the way are created for a file too.
        root.create_file(&p("new/deep/f"), None).unwrap();
        assert!(tmp.path().join("new/deep/f").is_file());
    }

    #[test]
    fn never_writes_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        root.symlink(&p("a"), outside.path().as_os_str()).unwrap();
        assert_eq!(root.kind(&p("a")).unwrap(), Some(Kind::Symlink));
        let err = root.create_file(&p("a/passwd"), None).unwrap_err();
        assert_eq!(err.to_string(), "a symbolic link or a file is in the way");
        assert!(root.dir(&p("a/sub"), true).is_err());
        assert!(root.symlink(&p("a/x"), OsStr::new("/")).is_err());
        assert!(root.kind(&p("a/x")).is_err());
        assert_eq!(
            fs::read_dir(outside.path()).unwrap().count(),
            0,
            "nothing outside"
        );
        // The link itself is never opened as a file either.
        assert!(root.create_file(&p("a"), None).is_err());
    }

    #[test]
    fn a_file_on_the_way_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("f"), b"").unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        assert!(root.dir(&p("f/x"), true).is_err());
        assert!(root.create_file(&p("f/x"), None).is_err());
    }

    #[test]
    fn replace_and_remove_work_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("t"), b"old").unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        let (temp, mut f) = root.create_temp_beside(&p("t"), Some(0o600)).unwrap();
        assert!(temp.to_string_lossy().starts_with(".t.yagni-"), "{temp:?}");
        f.write_all(b"new").unwrap();
        // A second temp gets another name.
        let (other, _) = root.create_temp_beside(&p("t"), None).unwrap();
        assert_ne!(other, temp);
        root.remove_temp(&p("t"), &other).unwrap();
        root.replace(&p("t"), &temp).unwrap();
        assert_eq!(fs::read(tmp.path().join("t")).unwrap(), b"new");
        root.remove_file(&p("t")).unwrap();
        let left: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn hard_links_stay_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        root.create_file(&p("a"), None).unwrap();
        root.hard_link(&p("a"), &p("d/b")).unwrap();
        assert_eq!(fs::metadata(tmp.path().join("d/b")).unwrap().nlink(), 2);
        assert!(root.hard_link(&p("missing"), &p("c")).is_err());
    }

    #[test]
    fn folder_mode_and_time_are_set_afterwards() {
        let tmp = tempfile::tempdir().unwrap();
        let root = SafeDir::open(tmp.path()).unwrap();
        root.dir(&p("d"), true).unwrap();
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        root.set_dir_mode_and_time(&p("d"), Some(0o2750), Some(when));
        let meta = fs::metadata(tmp.path().join("d")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o750, "setgid dropped");
        assert_eq!(meta.modified().unwrap(), when);
        // A missing folder is ignored (best effort).
        root.set_dir_mode_and_time(&p("missing"), Some(0o700), None);
    }

    #[test]
    fn dot_and_dot_dot_parts_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().join("root");
        fs::create_dir(&root_dir).unwrap();
        let root = SafeDir::open(&root_dir).unwrap();
        assert!(root.dir(&p(".."), true).is_err());
        assert!(root.create_file(&p("../x"), None).is_err());
        assert!(root.create_file(&p("./x"), None).is_err());
        assert!(root.create_file(&[OsString::new()], None).is_err());
        assert!(!tmp.path().join("x").exists());
    }

    #[test]
    fn modes_respect_the_umask() {
        let tmp = tempfile::tempdir().unwrap();
        // What the umask leaves of 0o777, seen on a fresh folder.
        fs::create_dir(tmp.path().join("probe")).unwrap();
        let allowed = fs::metadata(tmp.path().join("probe"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let root = SafeDir::open(tmp.path()).unwrap();
        root.create_file(&p("f"), Some(0o777)).unwrap();
        assert_eq!(mode_of(&tmp.path().join("f")), 0o777 & allowed);
        root.dir(&p("d"), true).unwrap();
        root.set_dir_mode_and_time(&p("d"), Some(0o777), None);
        assert_eq!(mode_of(&tmp.path().join("d")), 0o777 & allowed);
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }
}
