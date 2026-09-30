//! User settings, stored as a hand-editable TOML file (see [`crate::storage`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::storage::{self, StorageError};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Command that F4 runs with the file or directory as its argument, e.g. `code`.
    pub editor: Option<String>,
    /// Sort names case-sensitively (uppercase before lowercase).
    pub case_sensitive_sort: bool,
}

/// Written on first start so the available settings are discoverable.
const TEMPLATE: &str = r#"# yagni-commander settings

# Command that F4 runs with the file or directory as its argument.
# editor = "code"

# Sort names case-sensitively (uppercase before lowercase).
case_sensitive_sort = false
"#;

impl Config {
    /// Loads the config at `path`. If the file doesn't exist yet, writes a
    /// commented template there and returns the defaults.
    pub fn load_or_create(path: &Path) -> Result<Self, StorageError> {
        if !path.exists() {
            storage::write_atomic(path, TEMPLATE)?;
        }
        storage::load(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn first_load_writes_template_and_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/config.toml");
        assert_eq!(Config::load_or_create(&path).unwrap(), Config::default());
        assert_eq!(fs::read_to_string(&path).unwrap(), TEMPLATE);
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(
            toml::from_str::<Config>(TEMPLATE).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn existing_file_is_read_and_left_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let text = "editor = \"code --wait\"\ncase_sensitive_sort = true\n";
        fs::write(&path, text).unwrap();

        let config = Config::load_or_create(&path).unwrap();
        assert_eq!(config.editor.as_deref(), Some("code --wait"));
        assert!(config.case_sensitive_sort);
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let config: Config = toml::from_str("editor = \"vim\"").unwrap();
        assert!(!config.case_sensitive_sort);
    }

    #[test]
    fn unknown_keys_are_rejected_to_catch_typos() {
        assert!(toml::from_str::<Config>("case_sensitve_sort = true").is_err());
    }

    #[test]
    fn round_trips_through_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let config = Config {
            editor: Some("zed".into()),
            case_sensitive_sort: true,
        };
        storage::save(&path, &config).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), config);
    }
}
