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
/// never leaves a truncated file behind.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), StorageError> {
    let io_err = |e| StorageError::Io(path.to_owned(), e);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents).map_err(io_err)?;
    fs::rename(&tmp, path).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

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
