//! User settings, stored as a hand-editable TOML file (see [`crate::storage`]).

use std::fs;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sort::SortKey;
use crate::storage::{self, StorageError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Command that F4 runs with the file or directory as its argument, e.g. `code`.
    pub editor: Option<String>,
    /// Sort names case-sensitively (uppercase before lowercase).
    pub case_sensitive_sort: bool,
    /// Log every file the app creates, copies, moves, renames, trashes or
    /// deletes (see [`crate::oplog`]).
    pub log: bool,
    /// Log files older than this many days are deleted at startup.
    pub log_keep_days: u32,
    /// Optional panel columns. Name and Size are always shown.
    pub show_modified: bool,
    pub show_owner: bool,
    pub show_permissions: bool,
    /// A Nerd Font icon in front of every name (see [`crate::icons`]).
    pub icons: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: None,
            case_sensitive_sort: false,
            log: false,
            log_keep_days: 7,
            show_modified: true,
            show_owner: true,
            show_permissions: true,
            icons: true,
        }
    }
}

/// Written on first start so the available settings are discoverable.
const TEMPLATE: &str = r#"# yagni-commander settings

# Command that F4 runs with the file or directory as its argument.
# editor = "code"

# Sort names case-sensitively (uppercase before lowercase).
case_sensitive_sort = false

# Log every file created, copied, moved, renamed, trashed or deleted, one file
# per day in the "logs" folder next to the state file.
log = false

# Log files older than this many days are deleted at startup.
log_keep_days = 7

# Optional panel columns. Name and Size are always shown.
show_modified = true
show_owner = true
show_permissions = true

# An icon in front of every name.
icons = true
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

/// One setting, as the settings dialog changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    Editor(Option<String>),
    CaseSensitiveSort(bool),
    Log(bool),
    LogKeepDays(u32),
    ShowModified(bool),
    ShowOwner(bool),
    ShowPermissions(bool),
    Icons(bool),
}

impl Setting {
    /// The editor as typed: surrounding spaces dropped, empty means none.
    pub fn editor(text: &str) -> Self {
        let text = text.trim();
        Self::Editor((!text.is_empty()).then(|| text.to_owned()))
    }
}

/// Allowed values of `log_keep_days`.
pub const LOG_KEEP_DAYS: RangeInclusive<u32> = 1..=3650;

/// Reads a typed `log_keep_days`.
pub fn parse_keep_days(text: &str) -> Result<u32, &'static str> {
    text.trim()
        .parse()
        .ok()
        .filter(|days| LOG_KEEP_DAYS.contains(days))
        .ok_or("Enter a number from 1 to 3650.")
}

impl Config {
    pub fn set(&mut self, setting: &Setting) {
        match setting {
            Setting::Editor(editor) => self.editor = editor.clone(),
            Setting::CaseSensitiveSort(on) => self.case_sensitive_sort = *on,
            Setting::Log(on) => self.log = *on,
            Setting::LogKeepDays(days) => self.log_keep_days = *days,
            Setting::ShowModified(on) => self.show_modified = *on,
            Setting::ShowOwner(on) => self.show_owner = *on,
            Setting::ShowPermissions(on) => self.show_permissions = *on,
            Setting::Icons(on) => self.icons = *on,
        }
    }

    /// Whether the panels show the column for `key`. Name and Size always.
    pub fn shows_column(&self, key: SortKey) -> bool {
        match key {
            SortKey::Name | SortKey::Size => true,
            SortKey::Modified => self.show_modified,
            SortKey::Owner => self.show_owner,
            SortKey::Permissions => self.show_permissions,
        }
    }

    /// The optional columns turned off, in display order.
    pub fn hidden_columns(&self) -> Vec<SortKey> {
        [SortKey::Modified, SortKey::Owner, SortKey::Permissions]
            .into_iter()
            .filter(|&key| !self.shows_column(key))
            .collect()
    }
}

/// Writes one setting into the file at `path` (created from the template if
/// missing) and returns the whole config as the file now has it. Only that
/// key changes: comments, layout and other keys stay as they are on disk,
/// including hand edits made since startup. A file that does not parse is
/// left alone (`StorageError::Parse`).
pub fn save_setting(path: &Path, setting: &Setting) -> Result<Config, StorageError> {
    // Creates the template if needed; refuses a broken file.
    Config::load_or_create(path)?;
    let text = fs::read_to_string(path).map_err(|e| StorageError::Io(path.to_owned(), e))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| StorageError::Edit(path.to_owned(), e.to_string()))?;
    match setting {
        Setting::Editor(None) => remove_keeping_comments(&mut doc, "editor"),
        Setting::Editor(Some(editor)) => doc["editor"] = toml_edit::value(editor.as_str()),
        Setting::CaseSensitiveSort(on) => doc["case_sensitive_sort"] = toml_edit::value(*on),
        Setting::Log(on) => doc["log"] = toml_edit::value(*on),
        Setting::LogKeepDays(days) => doc["log_keep_days"] = toml_edit::value(i64::from(*days)),
        Setting::ShowModified(on) => doc["show_modified"] = toml_edit::value(*on),
        Setting::ShowOwner(on) => doc["show_owner"] = toml_edit::value(*on),
        Setting::ShowPermissions(on) => doc["show_permissions"] = toml_edit::value(*on),
        Setting::Icons(on) => doc["icons"] = toml_edit::value(*on),
    }
    let text = doc.to_string();
    let config: Config =
        toml::from_str(&text).map_err(|e| StorageError::Parse(path.to_owned(), e))?;
    storage::write_atomic(path, &text)?;
    Ok(config)
}

/// Removes `key`. toml_edit keeps the comments and blank lines above a key
/// as part of it; they move to the next key, or to the end of the file.
fn remove_keeping_comments(doc: &mut toml_edit::DocumentMut, key: &str) {
    let table = doc.as_table_mut();
    let Some(index) = table.iter().position(|(k, _)| k == key) else {
        return;
    };
    let above = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_owned();
    table.remove(key);
    if above.trim().is_empty() {
        return;
    }
    if let Some((mut next, _)) = table.iter_mut().nth(index) {
        let decor = next.leaf_decor_mut();
        let own = decor
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_owned();
        decor.set_prefix(above + &own);
        return;
    }
    let trailing = doc.trailing().as_str().unwrap_or_default().to_owned();
    doc.set_trailing(above + &trailing);
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!config.log, "logging is off by default");
        assert_eq!(config.log_keep_days, 7);
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
            log: true,
            log_keep_days: 30,
            show_modified: false,
            show_owner: true,
            show_permissions: false,
            icons: false,
        };
        storage::save(&path, &config).unwrap();
        assert_eq!(Config::load_or_create(&path).unwrap(), config);
    }

    const HAND_WRITTEN: &str = "\
# my settings

# the editor
editor = \"vim\"

log_keep_days = 30 # a month
case_sensitive_sort = false
";

    #[test]
    fn saving_keeps_comments_layout_and_other_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let config = save_setting(&path, &Setting::CaseSensitiveSort(true)).unwrap();
        assert!(config.case_sensitive_sort);
        assert_eq!(config.editor.as_deref(), Some("vim"));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            HAND_WRITTEN.replace("case_sensitive_sort = false", "case_sensitive_sort = true")
        );
    }

    #[test]
    fn a_commented_out_key_gets_a_real_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        Config::load_or_create(&path).unwrap(); // the template: `# editor = "code"`
        let config = save_setting(&path, &Setting::editor("code --wait")).unwrap();
        assert_eq!(config.editor.as_deref(), Some("code --wait"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# yagni-commander settings"), "{text}");
        assert!(text.contains("# editor = \"code\""), "comment kept: {text}");
        assert!(
            text.lines().any(|l| l == "editor = \"code --wait\""),
            "{text}"
        );
    }

    #[test]
    fn emptying_the_editor_removes_its_key() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let config = save_setting(&path, &Setting::editor("  ")).unwrap();
        assert_eq!(config.editor, None);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("editor = "), "{text}");
        assert!(text.contains("log_keep_days = 30 # a month"), "{text}");
        // The comments above the removed line stay.
        assert!(
            text.starts_with("# my settings\n\n# the editor\n"),
            "{text}"
        );
    }

    #[test]
    fn emptying_the_last_key_keeps_its_comments() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "log = true\n\n# the editor\neditor = \"vim\"\n").unwrap();
        save_setting(&path, &Setting::Editor(None)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# the editor"), "{text}");
        assert!(!text.contains("vim"), "{text}");
    }

    #[test]
    fn saving_keeps_hand_edits_made_since_startup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let _at_startup = Config::load_or_create(&path).unwrap();
        fs::write(&path, HAND_WRITTEN.replace("vim", "helix")).unwrap();
        let config = save_setting(&path, &Setting::Log(true)).unwrap();
        assert_eq!(config.editor.as_deref(), Some("helix"));
        assert!(config.log);
    }

    #[test]
    fn a_file_broken_since_startup_is_not_written() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "log = maybe\n").unwrap();
        let err = save_setting(&path, &Setting::Log(true)).unwrap_err();
        assert!(matches!(err, StorageError::Parse(..)), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "log = maybe\n");
    }

    #[test]
    fn a_missing_file_is_created_from_the_template() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/config.toml");
        let config = save_setting(&path, &Setting::LogKeepDays(14)).unwrap();
        assert_eq!(config.log_keep_days, 14);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("log_keep_days = 14"), "{text}");
        assert!(text.contains("# Log files older than"), "{text}");
    }

    #[test]
    fn set_changes_one_field() {
        let mut config = Config::default();
        config.set(&Setting::editor(" zed "));
        config.set(&Setting::Log(true));
        assert_eq!(config.editor.as_deref(), Some("zed"));
        assert!(config.log);
        assert!(!config.case_sensitive_sort);
        config.set(&Setting::CaseSensitiveSort(true));
        config.set(&Setting::LogKeepDays(3));
        assert!(config.case_sensitive_sort);
        assert_eq!(config.log_keep_days, 3);
    }

    #[test]
    fn keep_days_must_be_1_to_3650() {
        assert_eq!(parse_keep_days(" 30 "), Ok(30));
        assert_eq!(parse_keep_days("1"), Ok(1));
        assert_eq!(parse_keep_days("3650"), Ok(3650));
        for bad in ["0", "3651", "-1", "", "x", "1.5"] {
            assert_eq!(
                parse_keep_days(bad),
                Err("Enter a number from 1 to 3650."),
                "{bad}"
            );
        }
    }

    #[test]
    fn optional_columns_are_shown_by_default() {
        let config: Config = toml::from_str("editor = \"vim\"").unwrap();
        assert!(config.show_modified && config.show_owner && config.show_permissions);
        let template = TEMPLATE;
        for key in ["show_modified", "show_owner", "show_permissions"] {
            assert!(
                template.lines().any(|l| l == format!("{key} = true")),
                "{key} in the template"
            );
        }
    }

    #[test]
    fn name_and_size_are_always_shown() {
        let config = Config {
            show_modified: false,
            show_owner: false,
            show_permissions: false,
            ..Config::default()
        };
        assert!(config.shows_column(SortKey::Name));
        assert!(config.shows_column(SortKey::Size));
        assert!(!config.shows_column(SortKey::Modified));
        assert!(!config.shows_column(SortKey::Owner));
        assert!(!config.shows_column(SortKey::Permissions));
        assert_eq!(
            config.hidden_columns(),
            [SortKey::Modified, SortKey::Owner, SortKey::Permissions]
        );
        assert!(Config::default().hidden_columns().is_empty());
    }

    #[test]
    fn icons_are_on_by_default_and_in_the_template() {
        let config: Config = toml::from_str("editor = \"vim\"").unwrap();
        assert!(config.icons);
        assert!(TEMPLATE.lines().any(|l| l == "icons = true"));
    }

    #[test]
    fn the_icons_setting_is_saved_and_set() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let mut expected = Config::load_or_create(&path).unwrap();
        let config = save_setting(&path, &Setting::Icons(false)).unwrap();
        expected.set(&Setting::Icons(false));
        assert_eq!(config, expected);
        assert!(!config.icons);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.lines().any(|l| l == "icons = false"), "{text}");
        assert!(text.starts_with("# my settings"));
    }

    #[test]
    fn column_settings_are_saved_and_set() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let mut expected = Config::load_or_create(&path).unwrap();
        for (setting, key) in [
            (Setting::ShowModified(false), "show_modified"),
            (Setting::ShowOwner(false), "show_owner"),
            (Setting::ShowPermissions(false), "show_permissions"),
        ] {
            let config = save_setting(&path, &setting).unwrap();
            expected.set(&setting);
            assert_eq!(config, expected);
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.lines().any(|l| l == format!("{key} = false")),
                "{text}"
            );
        }
        assert!(!expected.show_modified && !expected.show_owner && !expected.show_permissions);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .starts_with("# my settings")
        );
    }
}
