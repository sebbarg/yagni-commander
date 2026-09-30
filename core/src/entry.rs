use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The synthetic ".." entry.
    Parent,
    Dir,
    File,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Raw name, used for navigation. Not necessarily valid UTF-8.
    pub name: OsString,
    /// Lossy UTF-8 name for display.
    pub label: String,
    /// Lowercased `label`, precomputed so sorting doesn't allocate per comparison.
    pub(crate) sort_name: String,
    pub kind: EntryKind,
    pub is_symlink: bool,
    /// File size in bytes. `None` for directories and unreadable entries.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// Unix mode bits (file type and permissions) of the entry itself, not a symlink's target.
    pub mode: Option<u32>,
    /// `user:group` of the entry itself. Shared between entries with the same owner.
    pub owner: Option<Arc<str>>,
}

impl Entry {
    pub fn parent() -> Self {
        Self {
            name: OsString::from(".."),
            label: "..".into(),
            sort_name: "..".into(),
            kind: EntryKind::Parent,
            is_symlink: false,
            size: None,
            modified: None,
            mode: None,
            owner: None,
        }
    }

    /// True for entries that Enter navigates into (including "..").
    pub fn is_navigable(&self) -> bool {
        matches!(self.kind, EntryKind::Parent | EntryKind::Dir)
    }
}

/// Reads `dir` and returns its entries, unsorted (see [`crate::sort_entries`]).
///
/// A ".." entry is included unless `dir` is a filesystem root.
/// Entries whose metadata cannot be read are still listed, as files without size.
/// Symlinks report kind, size and modification time of their target, but mode
/// and owner of the link itself, like `ls -l`.
pub fn read_entries(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut owners = OwnerCache::default();
    if dir.parent().is_some() {
        entries.push(Entry::parent());
    }

    for dirent in fs::read_dir(dir)? {
        let Ok(dirent) = dirent else { continue };
        let name = dirent.file_name();
        let label = name.to_string_lossy().into_owned();
        let link_meta = dirent.metadata().ok();
        let is_symlink = link_meta
            .as_ref()
            .is_some_and(|m| m.file_type().is_symlink());
        let mode = link_meta.as_ref().and_then(unix_mode);
        let owner = link_meta.as_ref().and_then(|m| owners.get(m));
        // Follow symlinks so a link to a directory behaves like a directory.
        let meta = if is_symlink {
            fs::metadata(dirent.path()).ok()
        } else {
            link_meta
        };
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        entries.push(Entry {
            name,
            sort_name: label.to_lowercase(),
            label,
            kind: if is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            is_symlink,
            size: meta.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()),
            modified: meta.and_then(|m| m.modified().ok()),
            mode,
            owner,
        });
    }

    Ok(entries)
}

#[cfg(unix)]
fn unix_mode(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.mode())
}

#[cfg(not(unix))]
fn unix_mode(_: &fs::Metadata) -> Option<u32> {
    None
}

/// Resolves uid/gid to `user:group` once per distinct pair. A directory
/// usually has one or two owners, so this avoids a passwd lookup per entry.
#[derive(Default)]
struct OwnerCache(HashMap<(u32, u32), Arc<str>>);

impl OwnerCache {
    #[cfg(unix)]
    fn get(&mut self, meta: &fs::Metadata) -> Option<Arc<str>> {
        use nix::unistd::{Gid, Group, Uid, User};
        use std::os::unix::fs::MetadataExt;

        let (uid, gid) = (meta.uid(), meta.gid());
        let owner = self.0.entry((uid, gid)).or_insert_with(|| {
            // Fall back to the numeric id, like `ls -l` does for unknown ids.
            let user = User::from_uid(Uid::from_raw(uid))
                .ok()
                .flatten()
                .map_or_else(|| uid.to_string(), |u| u.name);
            let group = Group::from_gid(Gid::from_raw(gid))
                .ok()
                .flatten()
                .map_or_else(|| gid.to_string(), |g| g.name);
            format!("{user}:{group}").into()
        });
        Some(owner.clone())
    }

    #[cfg(not(unix))]
    fn get(&mut self, _: &fs::Metadata) -> Option<Arc<str>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn root_has_no_parent_entry() {
        let entries = read_entries(Path::new("/")).unwrap();
        assert!(entries.iter().all(|e| e.kind != EntryKind::Parent));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_dir_is_a_dir_and_broken_link_is_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("nope"), tmp.path().join("broken")).unwrap();

        let entries = read_entries(tmp.path()).unwrap();
        let find = |n: &str| entries.iter().find(|e| e.label == n).unwrap();
        assert_eq!(find("link").kind, EntryKind::Dir);
        assert!(find("link").is_symlink);
        assert_eq!(find("broken").kind, EntryKind::File);
        assert!(find("broken").is_symlink);
    }

    #[cfg(unix)]
    #[test]
    fn reads_mode_and_owner_of_the_entry_itself() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        File::create(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&path, tmp.path().join("link")).unwrap();

        let entries = read_entries(tmp.path()).unwrap();
        let find = |n: &str| entries.iter().find(|e| e.label == n).unwrap();
        assert_eq!(find("f").mode.unwrap() & 0o7777, 0o640);
        assert!(find("f").owner.as_deref().unwrap().contains(':'));
        // The link's own mode (a symlink), not its target's.
        assert_eq!(find("link").mode.unwrap() & 0o170000, 0o120000);
    }
}
