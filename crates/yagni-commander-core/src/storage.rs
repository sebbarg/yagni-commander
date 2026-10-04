//! Loading and saving small TOML files (config, window state).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Directory name used under the platform config and state directories.
const APP_DIR: &str = "yagni-commander";

/// `~/.config/yagni-commander/config.toml` on Linux,
/// `~/Library/Application Support/yagni-commander/config.toml` on macOS.
pub fn config_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(APP_DIR).join("config.toml"))
}

/// `~/.local/state/yagni-commander/state.toml` on Linux. macOS has no state
/// directory, so it shares the config's Application Support folder there.
pub fn state_file() -> Option<PathBuf> {
    Some(state_dir()?.join("state.toml"))
}

/// The user's home folder; `/` if there is none.
pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// The operation log's directory: `logs` next to the state file.
pub fn log_dir() -> Option<PathBuf> {
    Some(state_dir()?.join("logs"))
}

/// Where F3 inside an archive puts its private copies: `viewer-tmp` next
/// to the state file. Emptied at startup.
pub fn viewer_temp_dir() -> Option<PathBuf> {
    Some(state_dir()?.join("viewer-tmp"))
}

/// Creates `dir` (and its parents) readable by the owner only, or makes an
/// existing one so.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Startup: removes what earlier runs left in `base` (viewer copies after
/// a crash). Each running instance has its own folder named after its
/// process id (`base/<pid>`); those of instances still running stay.
pub fn clear_stale_temp(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let running = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
            .is_some_and(is_running);
        if running {
            continue;
        }
        let path = entry.path();
        let _ = if path.is_dir() {
            clear_dir(&path)
        } else {
            fs::remove_file(&path)
        };
    }
}

/// Whether a process with this id exists (signal 0 checks without sending).
fn is_running(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    pid > 0 && matches!(kill(Pid::from_raw(pid), None), Ok(()) | Err(Errno::EPERM))
}

/// Removes `dir` with everything in it; a missing one is fine.
pub fn clear_dir(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn state_dir() -> Option<PathBuf> {
    let base = dirs::state_dir().or_else(dirs::data_dir)?;
    Some(base.join(APP_DIR))
}

#[derive(Debug)]
pub enum StorageError {
    Io(PathBuf, io::Error),
    Parse(PathBuf, toml::de::Error),
    Serialize(toml::ser::Error),
    /// A file that `toml` parsed but `toml_edit` could not.
    Edit(PathBuf, String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(path, e) => write!(f, "{e}: {}", path.display()),
            StorageError::Parse(path, e) => write!(f, "{} in {}", e.message(), path.display()),
            StorageError::Serialize(e) => write!(f, "cannot serialize: {e}"),
            StorageError::Edit(path, e) => write!(f, "{e} in {}", path.display()),
        }
    }
}

impl std::error::Error for StorageError {}

/// Reads `path`. A missing file yields the default value; anything else
/// unreadable or malformed is an error, so callers never silently replace a
/// file the user wrote.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StorageError> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| StorageError::Parse(path.to_owned(), e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(StorageError::Io(path.to_owned(), e)),
    }
}

/// Serializes `value` to `path`, creating parent directories.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), StorageError> {
    let text = toml::to_string(value).map_err(StorageError::Serialize)?;
    write_atomic(path, &text)
}

/// Writes `path` via a temporary file and a rename, so a crash mid-write
/// never leaves a truncated file behind. A symlink (a config kept with
/// dotfiles) is written through, never replaced; the file keeps its mode.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), StorageError> {
    let io_err = |e| StorageError::Io(path.to_owned(), e);
    let existing = path.symlink_metadata().ok();
    let path = &match &existing {
        Some(meta) if meta.is_symlink() => fs::canonicalize(path).map_err(io_err)?,
        _ => path.to_path_buf(),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        io::Write::write_all(&mut file, contents.as_bytes())?;
        if let Ok(meta) = fs::metadata(path) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn only_folders_of_running_instances_survive_the_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("viewer-tmp");
        let mine = std::process::id().to_string();
        for name in [mine.as_str(), "999999999", "junk"] {
            fs::create_dir_all(base.join(name).join("view-x")).unwrap();
        }
        fs::write(base.join("file"), b"").unwrap();
        clear_stale_temp(&base);
        let mut left: Vec<_> = fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [mine]);
        clear_stale_temp(&tmp.path().join("missing")); // nothing to do
    }

    #[test]
    fn private_dir_is_owner_only_and_clear_dir_removes_leftovers() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("viewer-tmp");
        private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::create_dir(dir.join("view-1")).unwrap();
        fs::write(dir.join("view-1/left"), b"").unwrap();
        clear_dir(&dir).unwrap();
        assert!(!dir.exists());
        clear_dir(&dir).unwrap(); // already gone
        assert!(viewer_temp_dir().is_none_or(|d| d.ends_with("yagni-commander/viewer-tmp")));
    }

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Sample {
        name: String,
        count: u32,
    }

    #[test]
    fn missing_file_loads_default() {
        let tmp = tempfile::tempdir().unwrap();
        let loaded: Sample = load(&tmp.path().join("none.toml")).unwrap();
        assert_eq!(loaded, Sample::default());
    }

    #[test]
    fn save_then_load_round_trips_and_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/sample.toml");
        let value = Sample {
            name: "x".into(),
            count: 3,
        };
        save(&path, &value).unwrap();
        assert_eq!(load::<Sample>(&path).unwrap(), value);
        assert!(!tmp.path().join("a/b/sample.toml.tmp").exists());
    }

    #[test]
    fn malformed_file_is_a_parse_error_naming_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        fs::write(&path, "count = \"not a number\"").unwrap();
        let err = load::<Sample>(&path).unwrap_err();
        assert!(matches!(err, StorageError::Parse(..)));
        // The problem comes first so it survives truncation in the UI.
        let message = err.to_string();
        assert!(message.contains("invalid type"), "{message}");
        assert!(message.ends_with(&path.display().to_string()), "{message}");
    }

    #[test]
    fn unreadable_path_is_an_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory can't be read as a file.
        let err = load::<Sample>(tmp.path()).unwrap_err();
        assert!(matches!(err, StorageError::Io(..)));
        assert!(err.to_string().contains(&tmp.path().display().to_string()));
    }

    #[test]
    fn a_symlinked_file_is_written_through_and_keeps_its_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("dotfiles/config.toml");
        fs::create_dir(tmp.path().join("dotfiles")).unwrap();
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = tmp.path().join("config.toml");
        symlink("dotfiles/config.toml", &link).unwrap();

        write_atomic(&link, "new").unwrap();
        assert!(
            link.symlink_metadata().unwrap().is_symlink(),
            "still a link"
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        let mode = real.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!tmp.path().join("dotfiles/config.toml.tmp").exists());
    }

    #[test]
    fn a_link_to_nothing_is_an_error_and_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("config.toml");
        std::os::unix::fs::symlink("missing/config.toml", &link).unwrap();
        assert!(matches!(
            write_atomic(&link, "new").unwrap_err(),
            StorageError::Io(..)
        ));
        assert!(link.symlink_metadata().unwrap().is_symlink());
    }

    #[test]
    fn write_fails_when_parent_is_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("file"), "").unwrap();
        let err = write_atomic(&tmp.path().join("file/child.toml"), "").unwrap_err();
        assert!(matches!(err, StorageError::Io(..)));
    }

    #[test]
    fn platform_paths_end_in_app_directory() {
        for path in [config_file(), state_file(), log_dir()]
            .into_iter()
            .flatten()
        {
            assert_eq!(
                path.parent().unwrap().file_name().unwrap(),
                "yagni-commander"
            );
        }
    }
}
