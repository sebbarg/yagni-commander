//! Mount points, so walks that delete (or search) stay on one filesystem.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// The mount points (Linux's `/proc/self/mountinfo`), or no table where
/// there is none to read (macOS): then a device number of its own decides.
#[derive(Debug, Clone, Default)]
pub struct Mounts(Option<HashSet<PathBuf>>);

impl Mounts {
    /// The system's mount table.
    pub fn read() -> Self {
        Self(
            fs::read_to_string("/proc/self/mountinfo")
                .ok()
                .map(|text| parse_mountinfo(&text)),
        )
    }

    /// A table of these real paths (tests).
    pub fn table(points: impl IntoIterator<Item = PathBuf>) -> Self {
        Self(Some(points.into_iter().collect()))
    }

    /// Whether the folder at `real` (its real path) on device `dev` is a
    /// mount point below a folder on device `parent_dev`. With a table, the
    /// table decides: a bind mount of the same filesystem counts, a btrfs
    /// subvolume (a device number of its own, no mount) does not. Without
    /// one, the device does.
    pub fn is_mount_point(&self, real: &Path, dev: u64, parent_dev: u64) -> bool {
        match &self.0 {
            Some(points) => points.contains(real),
            None => dev != parent_dev,
        }
    }
}

/// The fifth field of each line of `/proc/self/mountinfo`, with its octal
/// escapes (`\040` for a space) decoded.
fn parse_mountinfo(text: &str) -> HashSet<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    text.lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(|field| {
            let bytes = field.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                let octal = bytes.get(i + 1..i + 4).and_then(|d| {
                    let d = std::str::from_utf8(d).ok()?;
                    u8::from_str_radix(d, 8).ok()
                });
                match octal {
                    Some(b) if bytes[i] == b'\\' => {
                        out.push(b);
                        i += 4;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            PathBuf::from(OsString::from_vec(out))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_lists_mount_points_with_escapes() {
        let text = "36 35 98:0 /mnt1 /mnt/a\\040b rw,noatime master:1 - ext3 /dev/root rw\n\
                    37 35 0:5 / /proc rw - proc proc rw\n\
                    \n";
        let points = parse_mountinfo(text);
        assert!(points.contains(Path::new("/mnt/a b")));
        assert!(points.contains(Path::new("/proc")));
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn the_table_decides_where_there_is_one() {
        let mounts = Mounts::table([PathBuf::from("/m")]);
        // A btrfs subvolume: another device number, but no mount of its own.
        assert!(!mounts.is_mount_point(Path::new("/sub"), 2, 1));
        assert!(mounts.is_mount_point(Path::new("/m"), 2, 1));
        // A bind mount of the same filesystem.
        assert!(mounts.is_mount_point(Path::new("/m"), 1, 1));
    }

    #[test]
    fn without_a_table_the_device_decides() {
        let mounts = Mounts::default();
        assert!(mounts.is_mount_point(Path::new("/sub"), 2, 1));
        assert!(!mounts.is_mount_point(Path::new("/m"), 1, 1));
    }

    #[test]
    fn the_system_table_lists_proc_on_linux() {
        if Path::new("/proc/self/mountinfo").exists() {
            assert!(Mounts::read().is_mount_point(Path::new("/proc"), 0, 0));
        }
    }
}
