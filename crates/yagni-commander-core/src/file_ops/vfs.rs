//! The file systems an operation with a remote end works on ([`Fs`]): the
//! local one and a host over SFTP. Paths are panel paths (`/x/y` or
//! `sftp://host/x/y`), so messages and the log name them as the user sees
//! them. Operations between two local folders don't come here: they keep
//! the engine's own local code (`copy_file_range`, the `openat` delete).

use std::ffi::OsString;
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::remote::{self, Session};
use crate::sftp::{Attrs, Upload};

/// Bytes read from a local file at a time.
const LOCAL_CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Dir,
    File,
    Symlink,
    Other,
}

/// An entry's own attributes (a symlink's, not its target's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Meta {
    pub kind: Kind,
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: Option<u32>,
    /// Permission bits, without the file type.
    pub mode: Option<u32>,
}

impl Meta {
    pub fn modified(&self) -> Option<SystemTime> {
        self.mtime.map(remote::time_of)
    }
}

/// A file being written.
pub(super) enum Writer {
    Local(File),
    Remote(Upload),
}

/// What an operation needs from a file system. `make_dir`, `create` and
/// `rename` fail with `AlreadyExists` when the target exists.
pub(super) trait Fs {
    /// The host, for a remote one: two `Fs` with the same host can rename
    /// between them.
    fn host(&self) -> Option<&str>;
    fn lstat(&mut self, path: &Path) -> io::Result<Meta>;
    /// The entries of `dir`, by name; a remote server's names are checked
    /// to be single components.
    fn read_dir(&mut self, dir: &Path) -> io::Result<Vec<(OsString, Meta)>>;
    fn read_link(&mut self, path: &Path) -> io::Result<PathBuf>;
    /// A new folder with permissions `mode` (less the server's umask).
    fn make_dir(&mut self, path: &Path, mode: u32) -> io::Result<()>;
    /// A new file (never an existing one), readable by its owner only
    /// until [`Fs::finish`] gives it its mode.
    fn create(&mut self, path: &Path) -> io::Result<Writer>;
    fn write(&mut self, writer: &mut Writer, offset: u64, data: &[u8]) -> io::Result<()>;
    /// Ends the writing, with the time and mode given; `sync` flushes the
    /// file to disk first (a move deletes its original next).
    fn finish(
        &mut self,
        writer: Writer,
        mtime: Option<u32>,
        mode: Option<u32>,
        sync: bool,
    ) -> io::Result<()>;
    /// Ends the writing and removes `path`.
    fn abandon(&mut self, writer: Writer, path: &Path);
    /// Reads file `path` into `sink` (offset, piece; false stops).
    /// `Ok(false)`: stopped.
    fn read(
        &mut self,
        path: &Path,
        sink: &mut dyn FnMut(u64, &[u8]) -> io::Result<bool>,
    ) -> io::Result<bool>;
    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()>;
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    /// Renames `from` over an existing `to`.
    fn rename_over(&mut self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&mut self, path: &Path) -> io::Result<()>;
    fn remove_dir(&mut self, path: &Path) -> io::Result<()>;
    fn set_mode(&mut self, path: &Path, mode: u32) -> io::Result<()>;
}

/// The file system `path` is on: a new session for a remote one.
pub(super) fn open(path: &Path) -> io::Result<Box<dyn Fs>> {
    match remote::parse(path) {
        Some(location) => Ok(Box::new(SftpFs {
            host: location.host.to_owned(),
            session: remote::connect(location.host)?,
        })),
        None if remote::is_remote(path) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a remote folder: {}", path.display()),
        )),
        None => Ok(Box::new(LocalFs)),
    }
}

/// The name a path ends in, local or remote; `None` for a root.
pub(super) fn file_name(path: &Path) -> Option<OsString> {
    if remote::is_remote(path) {
        remote::file_name(path)
    } else {
        path.file_name().map(OsString::from)
    }
}

/// The folder holding a path, local or remote.
pub(super) fn parent(path: &Path) -> Option<PathBuf> {
    if remote::is_remote(path) {
        remote::parent(path)
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

pub(super) struct LocalFs;

impl LocalFs {
    fn meta(meta: &fs::Metadata) -> Meta {
        use std::os::unix::fs::MetadataExt;
        let kind = if meta.is_symlink() {
            Kind::Symlink
        } else if meta.is_dir() {
            Kind::Dir
        } else if meta.is_file() {
            Kind::File
        } else {
            Kind::Other
        };
        Meta {
            kind,
            size: meta.len(),
            mtime: u32::try_from(meta.mtime()).ok(),
            mode: Some(meta.mode() & 0o7777),
        }
    }
}

impl Fs for LocalFs {
    fn host(&self) -> Option<&str> {
        None
    }

    fn lstat(&mut self, path: &Path) -> io::Result<Meta> {
        fs::symlink_metadata(path).map(|m| Self::meta(&m))
    }

    fn read_dir(&mut self, dir: &Path) -> io::Result<Vec<(OsString, Meta)>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            entries.push((entry.file_name(), Self::meta(&entry.metadata()?)));
        }
        Ok(entries)
    }

    fn read_link(&mut self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path)
    }

    fn make_dir(&mut self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(mode).create(path)
    }

    fn create(&mut self, path: &Path) -> io::Result<Writer> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map(Writer::Local)
    }

    fn write(&mut self, writer: &mut Writer, offset: u64, data: &[u8]) -> io::Result<()> {
        match writer {
            Writer::Local(file) => file.write_all_at(data, offset),
            Writer::Remote(_) => unreachable!("a local file system writes local files"),
        }
    }

    fn finish(
        &mut self,
        writer: Writer,
        mtime: Option<u32>,
        mode: Option<u32>,
        sync: bool,
    ) -> io::Result<()> {
        let Writer::Local(file) = writer else {
            unreachable!("a local file system writes local files")
        };
        // Best effort, as for a local copy: some file systems store neither.
        if let Some(mtime) = mtime {
            let _ = file.set_times(FileTimes::new().set_modified(remote::time_of(mtime)));
        }
        if let Some(mode) = mode {
            let _ = file.set_permissions(fs::Permissions::from_mode(mode));
        }
        if sync {
            file.sync_all()?;
        }
        Ok(())
    }

    fn abandon(&mut self, writer: Writer, path: &Path) {
        drop(writer);
        let _ = fs::remove_file(path);
    }

    fn read(
        &mut self,
        path: &Path,
        sink: &mut dyn FnMut(u64, &[u8]) -> io::Result<bool>,
    ) -> io::Result<bool> {
        let mut file = File::open(path)?;
        let mut buf = vec![0; LOCAL_CHUNK];
        let mut offset = 0;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => return Ok(true),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if !sink(offset, &buf[..n])? {
                return Ok(false);
            }
            offset += n as u64;
        }
    }

    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        crate::fs_ops::rename_noreplace(from, to)
    }

    fn rename_over(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    fn set_mode(&mut self, path: &Path, mode: u32) -> io::Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
}

/// A host over its own SFTP session.
pub(super) struct SftpFs {
    host: String,
    session: Session,
}

impl SftpFs {
    /// The path on the host; a path elsewhere is refused.
    fn at<'a>(&self, path: &'a Path) -> io::Result<&'a [u8]> {
        match remote::parse(path) {
            Some(location) if location.host == self.host && !location.path.is_empty() => {
                Ok(location.path)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not on {}: {}", self.host, path.display()),
            )),
        }
    }

    fn meta(attrs: &Attrs) -> Meta {
        let kind = if attrs.is_dir() {
            Kind::Dir
        } else if attrs.is_file() {
            Kind::File
        } else if attrs.is_symlink() {
            Kind::Symlink
        } else {
            Kind::Other
        };
        Meta {
            kind,
            size: attrs.size.unwrap_or(0),
            mtime: attrs.mtime,
            mode: attrs.permissions.map(|m| m & 0o7777),
        }
    }

    /// `result`, with ssh's own words once the connection broke.
    fn explain<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        result.map_err(|e| self.session.explain(e))
    }

    /// A failure of a create, rename or mkdir, as `AlreadyExists` when
    /// `path` exists (OpenSSH reports that as a plain failure).
    fn exists_or(&mut self, e: io::Error, path: &[u8]) -> io::Error {
        if self.session.client.is_broken() {
            return self.session.explain(e);
        }
        if e.kind() != io::ErrorKind::PermissionDenied && self.session.client.lstat(path).is_ok() {
            return io::ErrorKind::AlreadyExists.into();
        }
        e
    }
}

impl Fs for SftpFs {
    fn host(&self) -> Option<&str> {
        Some(&self.host)
    }

    fn lstat(&mut self, path: &Path) -> io::Result<Meta> {
        let at = self.at(path)?;
        let result = self.session.client.lstat(at);
        self.explain(result).map(|a| Self::meta(&a))
    }

    fn read_dir(&mut self, dir: &Path) -> io::Result<Vec<(OsString, Meta)>> {
        let at = self.at(dir)?;
        let mut entries = Vec::new();
        let result = self.session.client.read_dir(at, |name| {
            if remote::valid_name(&name.name) {
                entries.push((OsString::from_vec(name.name), Self::meta(&name.attrs)));
            }
        });
        self.explain(result).map(|()| entries)
    }

    fn read_link(&mut self, path: &Path) -> io::Result<PathBuf> {
        let at = self.at(path)?;
        let result = self.session.client.readlink(at);
        self.explain(result)
            .map(|t| PathBuf::from(OsString::from_vec(t)))
    }

    fn make_dir(&mut self, path: &Path, mode: u32) -> io::Result<()> {
        let at = self.at(path)?;
        match self.session.client.mkdir(at, mode) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.exists_or(e, at)),
        }
    }

    fn create(&mut self, path: &Path) -> io::Result<Writer> {
        let at = self.at(path)?;
        match self.session.client.open_write(at, 0o600, true) {
            Ok(handle) => Ok(Writer::Remote(Upload::new(handle))),
            Err(e) => Err(self.exists_or(e, at)),
        }
    }

    fn write(&mut self, writer: &mut Writer, offset: u64, data: &[u8]) -> io::Result<()> {
        let Writer::Remote(upload) = writer else {
            unreachable!("a remote file system writes remote files")
        };
        // Pieces of at most the write size every server takes.
        let mut result = Ok(());
        for (ix, piece) in data.chunks(crate::sftp::WRITE_SIZE).enumerate() {
            let at = offset + (ix * crate::sftp::WRITE_SIZE) as u64;
            result = upload.write(&mut self.session.client, at, piece);
            if result.is_err() {
                break;
            }
        }
        self.explain(result)
    }

    fn finish(
        &mut self,
        writer: Writer,
        mtime: Option<u32>,
        mode: Option<u32>,
        sync: bool,
    ) -> io::Result<()> {
        let Writer::Remote(upload) = writer else {
            unreachable!("a remote file system writes remote files")
        };
        let result = upload.finish(&mut self.session.client, mtime, mode, sync);
        self.explain(result)
    }

    fn abandon(&mut self, writer: Writer, path: &Path) {
        if let Writer::Remote(upload) = writer {
            upload.abandon(&mut self.session.client);
        }
        if let Ok(at) = self.at(path) {
            let _ = self.session.client.remove(at);
        }
    }

    fn read(
        &mut self,
        path: &Path,
        sink: &mut dyn FnMut(u64, &[u8]) -> io::Result<bool>,
    ) -> io::Result<bool> {
        let at = self.at(path)?;
        let client = &mut self.session.client;
        let result = client.open_read(at).and_then(|handle| {
            let read = client.download(&handle, sink);
            let closed = client.close(handle);
            read.and_then(|done| closed.map(|()| done))
        });
        self.explain(result)
    }

    fn symlink(&mut self, target: &Path, link: &Path) -> io::Result<()> {
        let at = self.at(link)?;
        match self
            .session
            .client
            .symlink(target.as_os_str().as_bytes(), at)
        {
            Ok(()) => Ok(()),
            Err(e) => Err(self.exists_or(e, at)),
        }
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (self.at(from)?, self.at(to)?);
        match self.session.client.rename(from, to) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.exists_or(e, to)),
        }
    }

    fn rename_over(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (self.at(from)?, self.at(to)?);
        let result = self.session.client.rename_over(from, to);
        self.explain(result)
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        let at = self.at(path)?;
        let result = self.session.client.remove(at);
        self.explain(result)
    }

    fn remove_dir(&mut self, path: &Path) -> io::Result<()> {
        let at = self.at(path)?;
        let result = self.session.client.rmdir(at);
        self.explain(result)
    }

    fn set_mode(&mut self, path: &Path, mode: u32) -> io::Result<()> {
        let at = self.at(path)?;
        let result = self.session.client.setstat(at, None, Some(mode));
        self.explain(result)
    }
}
