//! F4 on a remote folder: the file is edited in a local copy, and each save
//! is uploaded ([`upload_edit`]) while the remote file is still the one
//! that was downloaded ([`RemoteStamp`]); a remote file changed meanwhile
//! is left alone until the user decides.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::vfs::{self, Fs, LocalFs};

/// What a remote file was when it was last read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteStamp {
    pub size: u64,
    /// Seconds since the epoch.
    pub mtime: Option<u32>,
}

/// How an upload of an edited file went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uploaded {
    /// Replaced; the remote file is now this.
    Done(RemoteStamp),
    /// The remote file is not what was expected (changed, or gone):
    /// nothing was written.
    Changed,
}

/// The remote file's stamp now.
pub fn remote_stamp(remote: &Path) -> io::Result<RemoteStamp> {
    let mut fs = vfs::open(remote)?;
    stamp_of(fs.as_mut(), remote)
}

fn stamp_of(fs: &mut dyn Fs, path: &Path) -> io::Result<RemoteStamp> {
    let meta = fs.lstat(path)?;
    Ok(RemoteStamp {
        size: meta.size,
        mtime: meta.mtime,
    })
}

/// Uploads `local` over `remote` if `remote` is still `expected` (any
/// state with `None`): written beside it and renamed over it, so the remote
/// file is never half written. Keeps the remote file's permissions; takes
/// the local file's time.
pub fn upload_edit(
    local: &Path,
    remote: &Path,
    expected: Option<RemoteStamp>,
) -> io::Result<Uploaded> {
    let mut fs = vfs::open(remote)?;
    let now = stamp_of(fs.as_mut(), remote);
    let mode = fs.lstat(remote).ok().and_then(|m| m.mode).unwrap_or(0o644);
    match (&now, expected) {
        (Ok(now), Some(expected)) if *now != expected => return Ok(Uploaded::Changed),
        (Err(e), Some(_)) if e.kind() == io::ErrorKind::NotFound => return Ok(Uploaded::Changed),
        (Err(e), _) if e.kind() != io::ErrorKind::NotFound => {
            return Err(io::Error::new(e.kind(), e.to_string()));
        }
        _ => {}
    }
    let mtime = u32::try_from(fs::metadata(local)?.mtime()).ok();
    let name = vfs::file_name(remote).unwrap_or_default();
    let folder = vfs::parent(remote).unwrap_or_default();
    let (temp, mut writer) = (0..100)
        .find_map(|n| {
            let temp: PathBuf = folder.join(format!(".{}.yagni-{n}.tmp", name.to_string_lossy()));
            match fs.create(&temp) {
                Ok(writer) => Some(Ok((temp, writer))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .unwrap_or_else(|| Err(io::ErrorKind::AlreadyExists.into()))?;
    let written = LocalFs.read(local, &mut |offset, piece| {
        fs.write(&mut writer, offset, piece)?;
        Ok(true)
    });
    if let Err(e) = written {
        fs.abandon(writer, &temp);
        return Err(e);
    }
    let renamed = fs
        .finish(writer, mtime, Some(mode & 0o7777))
        .and_then(|()| fs.rename_over(&temp, remote));
    if let Err(e) = renamed {
        let _ = fs.remove_file(&temp);
        return Err(e);
    }
    stamp_of(fs.as_mut(), remote).map(Uploaded::Done)
}
