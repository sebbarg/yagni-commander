//! Mount points, so walks that delete (or search) stay on one filesystem,
//! and the mounts dropdown's entries (Alt-F1/Alt-F2).

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
    text.lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(unescape)
        .collect()
}

/// A mountinfo field with its octal escapes (`\040` for a space) decoded.
fn unescape(field: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
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
}

/// One entry of the mounts dropdown (Alt-F1/Alt-F2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
}

/// The mounts dropdown's entries: Root, Home when it is on a filesystem of
/// its own, then the mounted filesystems a user would want to go to, by
/// mount point. Built from the
/// mount table's text alone (Linux) or one read of `/Volumes` (macOS):
/// nothing on a mount is touched, so a dead network mount can't hang it.
pub fn places(home: &Path) -> Vec<Place> {
    // macOS: home is on the boot volume's data volume, which users see as
    // the same disk as `/`.
    let (mounts, separate_home) = if cfg!(target_os = "macos") {
        (volumes(Path::new("/Volumes")), false)
    } else {
        let user = nix::unistd::User::from_uid(nix::unistd::getuid())
            .ok()
            .flatten()
            .map(|user| user.name)
            .unwrap_or_default();
        let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        (
            user_mounts(&text, home, &user),
            home_is_separate(&text, home),
        )
    };
    with_root_and_home(mounts, home, separate_home)
}

fn with_root_and_home(mounts: Vec<PathBuf>, home: &Path, separate_home: bool) -> Vec<Place> {
    let mut places = vec![Place {
        label: "Root".into(),
        path: PathBuf::from("/"),
    }];
    if separate_home {
        places.push(Place {
            label: "Home".into(),
            path: home.to_path_buf(),
        });
    }
    places.extend(
        mounts
            .into_iter()
            .filter(|path| path != home && path != Path::new("/"))
            .map(|path| Place {
                label: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                path,
            }),
    );
    places
}

/// Filesystem types that are the system's, not the user's: GLib's list
/// (`g_unix_is_system_fs_type`) plus a few it leaves out.
const SYSTEM_FS_TYPES: &[&str] = &[
    "adfs",
    "afs",
    "auto",
    "autofs",
    "autofs4",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "cxfs",
    "debugfs",
    "devfs",
    "devpts",
    "devtmpfs",
    "ecryptfs",
    "efivarfs",
    "fdescfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "fuse.snapfuse",
    "fusectl",
    "gfs",
    "gfs2",
    "gpfs",
    "hugetlbfs",
    "kernfs",
    "linprocfs",
    "linsysfs",
    "lustre",
    "lustre_lite",
    "mfs",
    "mqueue",
    "ncpfs",
    "nfsd",
    "nsfs",
    "nullfs",
    "ocfs2",
    "overlay",
    "proc",
    "procfs",
    "pstore",
    "ptyfs",
    "ramfs",
    "rootfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "squashfs",
    "sysfs",
    "tmpfs",
    "tracefs",
    "usbfs",
];

/// The mount points in `mountinfo` worth offering, sorted, each once (GLib's
/// `g_unix_mount_entry_guess_should_display`, plus `/mnt/`): not a system
/// filesystem type, not a loop device, the filesystem's own root (not a
/// bind mount or a btrfs subvolume, which repeat another entry), no hidden
/// component, and under `/media/`, `/run/media/<user>/`, `/mnt/` or `home`.
fn user_mounts(mountinfo: &str, home: &Path, user: &str) -> Vec<PathBuf> {
    let run_media = Path::new("/run/media").join(user);
    let mut points: Vec<PathBuf> = mountinfo
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let root = *fields.get(3)?;
            let point = unescape(fields.get(4)?);
            // After the optional fields: " - fstype source superoptions".
            let dash = fields.iter().position(|f| *f == "-")?;
            let fs_type = *fields.get(dash + 1)?;
            let source = fields.get(dash + 2).copied().unwrap_or("");
            let under = |dir: &Path| point.starts_with(dir) && point != dir;
            let wanted = root == "/"
                && !SYSTEM_FS_TYPES.contains(&fs_type)
                && !source.starts_with("/dev/loop")
                && !point
                    .components()
                    .any(|c| c.as_os_str().as_encoded_bytes().starts_with(b"."))
                && (under(Path::new("/media"))
                    || (!user.is_empty() && under(&run_media))
                    || under(Path::new("/mnt"))
                    || under(home));
            wanted.then_some(point)
        })
        .collect();
    points.sort();
    points.dedup();
    points
}

/// Whether `home` lies on another filesystem than `/`: the mount holding it
/// (the longest mount point containing it; the last of stacked ones) has
/// another device than the root mount. Device numbers in mountinfo are the
/// filesystem's, so a btrfs subvolume mounted on `/home` counts as the same
/// filesystem as a `/` on the same btrfs.
fn home_is_separate(mountinfo: &str, home: &Path) -> bool {
    let mut root_dev = None;
    let mut home_mount: Option<(usize, &str)> = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let (Some(dev), Some(point)) = (fields.get(2), fields.get(4)) else {
            continue;
        };
        let point = unescape(point);
        if point == Path::new("/") {
            root_dev = Some(*dev);
        }
        let depth = point.components().count();
        if home.starts_with(&point) && home_mount.is_none_or(|(d, _)| depth >= d) {
            home_mount = Some((depth, *dev));
        }
    }
    match (root_dev, home_mount) {
        (Some(root), Some((_, home))) => root != home,
        _ => false,
    }
}

/// The volumes in `dir` (macOS's `/Volumes`), sorted; symlinks left out
/// (the boot volume's entry points to `/`). The entries' types come from
/// the directory itself, so no volume is stat'ed.
fn volumes(dir: &Path) -> Vec<PathBuf> {
    let mut points: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| !t.is_symlink()))
        .filter(|entry| !entry.file_name().as_encoded_bytes().starts_with(b"."))
        .map(|entry| entry.path())
        .collect();
    points.sort();
    points
}

/// The entry holding `dir`: the one with the longest path that contains
/// it (Root at the least).
pub fn containing(places: &[Place], dir: &Path) -> usize {
    places
        .iter()
        .enumerate()
        .filter(|(_, place)| dir.starts_with(&place.path))
        .max_by_key(|(_, place)| place.path.components().count())
        .map_or(0, |(ix, _)| ix)
}

/// A letter typed in the dropdown: the next entry after `from` whose label
/// starts with it (ignoring case, wrapping), and whether it is the only one,
/// so the dropdown can go there at once, like a menu mnemonic.
pub fn next_with_letter(places: &[Place], from: usize, letter: char) -> Option<(usize, bool)> {
    let letter: String = letter.to_lowercase().collect();
    let starts = |place: &Place| {
        let first: String = place
            .label
            .chars()
            .take(1)
            .flat_map(char::to_lowercase)
            .collect();
        !first.is_empty() && first == letter
    };
    let count = places.len();
    let next = (1..=count)
        .map(|step| (from + step) % count)
        .find(|&ix| starts(&places[ix]))?;
    let unique = places.iter().filter(|place| starts(place)).count() == 1;
    Some((next, unique))
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

    const HOME: &str = "/home/seb";

    fn line(root: &str, point: &str, fs_type: &str, source: &str) -> String {
        format!("36 35 8:1 {root} {point} rw,relatime shared:1 - {fs_type} {source} rw\n")
    }

    fn mounts_of(lines: &[String]) -> Vec<PathBuf> {
        user_mounts(&lines.concat(), Path::new(HOME), "seb")
    }

    #[test]
    fn user_mounts_keep_media_mnt_and_home() {
        let found = mounts_of(&[
            line("/", "/", "ext4", "/dev/nvme0n1p2"),
            line("/", "/run/media/seb/USB\\040STICK", "vfat", "/dev/sdb1"),
            line("/", "/media/backup", "ext4", "/dev/sdc1"),
            line("/", "/mnt/nas", "nfs4", "nas:/export"),
            line("/", "/home/seb/mnt", "fuse.sshfs", "seb@host:/"),
            line("/", "/boot/efi", "vfat", "/dev/nvme0n1p1"),
            line("/", "/srv/data", "ext4", "/dev/sdd1"),
        ]);
        assert_eq!(
            found,
            [
                "/home/seb/mnt",
                "/media/backup",
                "/mnt/nas",
                "/run/media/seb/USB STICK"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn user_mounts_leave_out_the_system_and_repeats() {
        let found = mounts_of(&[
            line("/", "/mnt/proc", "proc", "proc"),
            line("/", "/mnt/tmp", "tmpfs", "tmpfs"),
            line("/", "/mnt/snap", "squashfs", "/dev/loop3"),
            line("/", "/mnt/image", "iso9660", "/dev/loop0"),
            // A bind mount and a btrfs subvolume: not the filesystem's root.
            line("/data", "/mnt/bind", "ext4", "/dev/sda1"),
            line("/@home", "/home/seb/sub", "btrfs", "/dev/sda2"),
            line("/", "/home/seb/.cache/doc", "fuse.portal", "portal"),
            line("/", "/home/seb/.hidden", "fuse.sshfs", "host:/"),
            line("/", "/run/media/other/USB", "vfat", "/dev/sdb1"),
            // The folders themselves, and a mount stacked twice.
            line("/", "/mnt", "ext4", "/dev/sda3"),
            line("/", "/media/disk", "ext4", "/dev/sdc1"),
            line("/", "/media/disk", "ext4", "/dev/sdc2"),
            "malformed line\n".into(),
        ]);
        assert_eq!(found, [PathBuf::from("/media/disk")]);
    }

    #[test]
    fn without_a_user_run_media_is_not_offered() {
        let text = line("/", "/run/media/USB", "vfat", "/dev/sdb1");
        assert!(user_mounts(&text, Path::new(HOME), "").is_empty());
    }

    #[test]
    fn root_and_a_separate_home_come_first_and_are_not_repeated() {
        let mounts = ["/", HOME, "/media/USB", "/"].map(PathBuf::from).to_vec();
        let labels =
            |places: Vec<Place>| -> Vec<String> { places.into_iter().map(|p| p.label).collect() };
        let places = with_root_and_home(mounts.clone(), Path::new(HOME), true);
        assert_eq!(places[1].path, Path::new(HOME));
        assert_eq!(labels(places), ["Root", "Home", "USB"]);
        let places = with_root_and_home(mounts, Path::new(HOME), false);
        assert_eq!(labels(places), ["Root", "USB"]);
    }

    fn dev_line(dev: &str, root: &str, point: &str) -> String {
        format!("36 35 {dev} {root} {point} rw - ext4 /dev/x rw\n")
    }

    #[test]
    fn home_is_separate_on_another_device() {
        let home = Path::new(HOME);
        let root = dev_line("259:2", "/", "/");
        assert!(!home_is_separate(&root, home), "all on /");
        let part = root.clone() + &dev_line("259:3", "/", "/home");
        assert!(home_is_separate(&part, home), "a /home partition");
        let own = root.clone() + &dev_line("0:52", "/", HOME);
        assert!(home_is_separate(&own, home), "a network home");
        // A btrfs subvolume on /home: the same filesystem as /.
        let subvol = dev_line("0:31", "/@", "/") + &dev_line("0:31", "/@home", "/home");
        assert!(!home_is_separate(&subvol, home), "a btrfs subvolume");
        // A mount below home doesn't count, a stacked one does (the last).
        let below = root.clone() + &dev_line("0:60", "/", "/home/seb/mnt");
        assert!(!home_is_separate(&below, home));
        let stacked = part + &dev_line("259:2", "/", "/home");
        assert!(!home_is_separate(&stacked, home));
        assert!(!home_is_separate("", home), "no table");
        assert!(!home_is_separate("short line\n", home));
    }

    #[test]
    fn volumes_leave_out_symlinks_and_hidden_entries() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("Backup")).unwrap();
        fs::create_dir(tmp.path().join("Archive")).unwrap();
        fs::create_dir(tmp.path().join(".timemachine")).unwrap();
        std::os::unix::fs::symlink("/", tmp.path().join("Macintosh HD")).unwrap();
        assert_eq!(
            volumes(tmp.path()),
            [tmp.path().join("Archive"), tmp.path().join("Backup")]
        );
        assert!(volumes(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn the_system_places_start_with_root() {
        let places = places(Path::new(HOME));
        assert_eq!(places[0].path, Path::new("/"));
    }

    fn sample() -> Vec<Place> {
        with_root_and_home(
            [
                "/home/seb/mnt",
                "/media/hdd",
                "/media/home-backup",
                "/mnt/nas",
            ]
            .map(PathBuf::from)
            .to_vec(),
            Path::new(HOME),
            true,
        )
    }

    #[test]
    fn the_containing_entry_is_the_longest_match() {
        let places = sample();
        assert_eq!(containing(&places, Path::new("/etc")), 0);
        assert_eq!(containing(&places, Path::new("/home/seb/src")), 1);
        assert_eq!(containing(&places, Path::new("/home/seb/mnt/x")), 2);
        // By component: /mnt/nasty is not on /mnt/nas.
        assert_eq!(containing(&places, Path::new("/mnt/nasty")), 0);
        assert_eq!(containing(&[], Path::new("/x")), 0);
    }

    #[test]
    fn a_letter_cycles_or_goes_at_once_when_unique() {
        let places = sample();
        // Labels: Root, Home, mnt, hdd, home-backup, nas.
        assert_eq!(next_with_letter(&places, 0, 'h'), Some((1, false)));
        assert_eq!(next_with_letter(&places, 1, 'H'), Some((3, false)));
        assert_eq!(next_with_letter(&places, 4, 'h'), Some((1, false)));
        assert_eq!(next_with_letter(&places, 0, 'n'), Some((5, true)));
        assert_eq!(next_with_letter(&places, 5, 'r'), Some((0, true)));
        assert_eq!(next_with_letter(&places, 0, 'z'), None);
        assert_eq!(next_with_letter(&[], 0, 'a'), None);
    }
}
