//! User settings, stored as a hand-editable TOML file (see [`crate::storage`]).

use std::fs;
use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sort::SortKey;
use crate::storage::{self, StorageError};

pub use crate::hotlist::HotlistEntry;
pub use crate::user_commands::UserCommand;

/// Unknown keys are ignored, and left alone when the file is saved, so
/// another version of the app can read a file this one wrote and the
/// reverse. Missing keys get their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
    /// The Git branch in the path header (see [`crate::git`]).
    pub git: bool,
    /// Ask GitHub for a newer release at startup (see [`crate::update`]).
    pub check_updates: bool,
    /// The theme's id (`tokyo-night`, `gruvbox-dark`, `everforest-dark`,
    /// `catppuccin-latte`, `classic`); none
    /// means the default. Not checked here: the app knows the themes.
    pub theme: Option<String>,
    /// Bookmarked folders (Ctrl-D), edited in the app.
    pub hotlist: Vec<HotlistEntry>,
    /// Programs to open the cursor entry with (Ctrl-S), edited in the app.
    pub commands: Vec<UserCommand>,
    /// The file's top-level keys this version doesn't know, as loaded by
    /// [`Config::load_or_create`]. Not part of the file.
    #[serde(skip)]
    pub ignored: Vec<String>,
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
            git: true,
            check_updates: false,
            theme: None,
            hotlist: Vec::new(),
            commands: Vec::new(),
            ignored: Vec::new(),
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

# The Git branch in the path header, with commits ahead and behind its
# upstream and a dot when something changed.
git = true

# Ask GitHub for a newer release at startup; a button in the status bar
# then offers to install it.
check_updates = false

# Color theme: "tokyo-night" (the default), "gruvbox-dark", "everforest-dark",
# "catppuccin-latte" or "classic".
# theme = "tokyo-night"
"#;

impl Config {
    /// Loads the config at `path`. If the file doesn't exist yet, writes a
    /// commented template there and returns the defaults.
    pub fn load_or_create(path: &Path) -> Result<Self, StorageError> {
        if !path.exists() {
            storage::write_atomic(path, TEMPLATE)?;
        }
        let mut config: Self = storage::load(path)?;
        if let Ok(text) = fs::read_to_string(path) {
            config.ignored = unknown_keys(&text);
        }
        Ok(config)
    }

    /// The status line's note about [`Config::ignored`], if any.
    pub fn ignored_notice(&self) -> Option<String> {
        match self.ignored.as_slice() {
            [] => None,
            [key] => Some(format!("Ignored unknown setting: {key}")),
            keys => Some(format!("Ignored unknown settings: {}", keys.join(", "))),
        }
    }
}

/// The top-level keys of `text` that [`Config`] doesn't have, sorted.
fn unknown_keys(text: &str) -> Vec<String> {
    // Every field, so the list can't fall behind the struct.
    let filled = Config {
        editor: Some(String::new()),
        theme: Some(String::new()),
        ..Config::default()
    };
    let (Ok(table), Ok(known)) = (text.parse::<toml::Table>(), toml::Table::try_from(filled))
    else {
        return Vec::new();
    };
    let mut unknown: Vec<String> = table
        .keys()
        .filter(|key| !known.contains_key(*key))
        .cloned()
        .collect();
    unknown.sort();
    unknown
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
    Git(bool),
    CheckUpdates(bool),
    Theme(String),
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
            Setting::Git(on) => self.git = *on,
            Setting::CheckUpdates(on) => self.check_updates = *on,
            Setting::Theme(id) => self.theme = Some(id.clone()),
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
        Setting::Git(on) => doc["git"] = toml_edit::value(*on),
        Setting::CheckUpdates(on) => doc["check_updates"] = toml_edit::value(*on),
        Setting::Theme(id) => doc["theme"] = toml_edit::value(id.as_str()),
    }
    let text = doc.to_string();
    let config: Config =
        toml::from_str(&text).map_err(|e| StorageError::Parse(path.to_owned(), e))?;
    storage::write_atomic(path, &text)?;
    Ok(config)
}

/// Comment above the first `[[hotlist]]` table.
const HOTLIST_COMMENT: &str = "\n# Directory hotlist (Ctrl-D), edited in the app. Keep it at the end of\n# the file: a setting written below it would belong to its last entry.\n";

/// Saves the whole hotlist, like [`save_setting`]: only the `hotlist`
/// tables change (comments inside the old ones are lost), a broken file is
/// left alone, and the config as now on disk is returned.
pub fn save_hotlist(path: &Path, hotlist: &[HotlistEntry]) -> Result<Config, StorageError> {
    let tables = hotlist
        .iter()
        .map(|entry| [("name", entry.name.as_str()), ("path", entry.path.as_str())]);
    save_tables(path, "hotlist", HOTLIST_COMMENT, tables)
}

/// Comment above the first `[[commands]]` table.
const COMMANDS_COMMENT: &str = "\n# User commands (Ctrl-S), edited in the app. Keep them at the end of the\n# file: a setting written below would belong to the last entry.\n";

/// Saves the whole list of user commands, like [`save_hotlist`].
pub fn save_commands(path: &Path, commands: &[UserCommand]) -> Result<Config, StorageError> {
    let tables = commands.iter().map(|entry| {
        [
            ("name", entry.name.as_str()),
            ("command", entry.command.as_str()),
        ]
    });
    save_tables(path, "commands", COMMANDS_COMMENT, tables)
}

/// Replaces the `[[key]]` tables, each given as its fields.
fn save_tables<'a, const N: usize>(
    path: &Path,
    key: &str,
    comment: &str,
    entries: impl ExactSizeIterator<Item = [(&'a str, &'a str); N]>,
) -> Result<Config, StorageError> {
    Config::load_or_create(path)?;
    let text = fs::read_to_string(path).map_err(|e| StorageError::Io(path.to_owned(), e))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| StorageError::Edit(path.to_owned(), e.to_string()))?;
    doc.remove(key);
    if entries.len() > 0 {
        let mut tables = toml_edit::ArrayOfTables::new();
        for (ix, fields) in entries.enumerate() {
            let mut table = toml_edit::Table::new();
            for (name, value) in fields {
                table[name] = toml_edit::value(value);
            }
            table
                .decor_mut()
                .set_prefix(if ix == 0 { comment } else { "\n" });
            tables.push(table);
        }
        doc.insert(key, toml_edit::Item::ArrayOfTables(tables));
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

    fn entry(name: &str, path: &str) -> HotlistEntry {
        HotlistEntry {
            name: name.into(),
            path: path.into(),
        }
    }

    #[test]
    fn user_commands_are_read_from_the_file() {
        let text = "[[commands]]\nname = \"&SQLite\"\ncommand = \"sqlitebrowser {active_path}\"\n\n[[commands]]\ncommand = \"x\"\n";
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.commands.len(), 2);
        assert_eq!(config.commands[0].name, "&SQLite");
        assert_eq!(config.commands[1].name, "");
        assert!(Config::default().commands.is_empty());
    }

    #[test]
    fn the_hotlist_and_the_commands_are_saved_without_touching_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "# mine\nicons = false\n").unwrap();
        let commands = [UserCommand {
            name: "&SQLite".into(),
            command: "x \"a b\" {active_path}".into(),
        }];
        save_hotlist(&path, &[entry("&Src", "~/src")]).unwrap();
        let config = save_commands(&path, &commands).unwrap();
        assert_eq!(config.commands, commands);
        assert_eq!(config.hotlist, [entry("&Src", "~/src")]);
        let config = save_hotlist(&path, &[entry("&New", "/new")]).unwrap();
        assert_eq!(config.commands, commands);
        let config = save_setting(&path, &Setting::Git(false)).unwrap();
        assert_eq!(config.commands, commands);
        assert!(!config.git, "a setting doesn't land in a table");
        let config = save_commands(&path, &[]).unwrap();
        assert!(config.commands.is_empty());
        assert_eq!(config.hotlist, [entry("&New", "/new")]);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("# mine") && !saved.contains("[[commands]]"),
            "{saved}"
        );
    }

    #[test]
    fn save_commands_never_writes_a_broken_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "not toml [").unwrap();
        assert!(save_commands(&path, &[]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not toml [");
    }

    #[test]
    fn a_hotlist_is_read_from_the_file() {
        let text = "icons = false\n\n[[hotlist]]\nname = \"&Src\"\npath = \"~/src\"\n\n[[hotlist]]\nname = \"\"\npath = \"/etc\"\n";
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.hotlist, [entry("&Src", "~/src"), entry("", "/etc")]);
        assert!(!config.icons);
        assert!(Config::default().hotlist.is_empty());
    }

    #[test]
    fn a_hotlist_entry_without_a_name_has_an_empty_one() {
        let config: Config = toml::from_str("[[hotlist]]\npath = \"/x\"\n").unwrap();
        assert_eq!(config.hotlist, [entry("", "/x")]);
    }

    #[test]
    fn a_hotlist_entry_missing_its_path_is_a_parse_error() {
        let text = "[[hotlist]]\nname = \"x\"\npth = \"/etc\"\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }

    #[test]
    fn an_unknown_key_in_an_entry_is_ignored() {
        let text =
            "[[hotlist]]\npath = \"/x\"\nfuture = 1\n[[commands]]\ncommand = \"c\"\nfuture = 1\n";
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.hotlist, [entry("", "/x")]);
        assert_eq!(config.commands[0].command, "c");
    }

    #[test]
    fn save_hotlist_replaces_only_the_hotlist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let text =
            "# mine\neditor = \"vim\" # inline\n\n[[hotlist]]\nname = \"old\"\npath = \"/old\"\n";
        fs::write(&path, text).unwrap();

        let list = [entry("&Src", "~/src"), entry("R&&D", "/rd")];
        let config = save_hotlist(&path, &list).unwrap();
        assert_eq!(config.hotlist, list);
        assert_eq!(config.editor.as_deref(), Some("vim"));
        let saved = fs::read_to_string(&path).unwrap();
        assert!(
            saved.starts_with("# mine\neditor = \"vim\" # inline\n"),
            "{saved}"
        );
        assert!(!saved.contains("/old"), "{saved}");
        assert!(saved.contains("# Directory hotlist (Ctrl-D), edited in the app."));
        assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
    }

    #[test]
    fn a_setting_saved_after_the_hotlist_goes_above_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "").unwrap();
        save_hotlist(&path, &[entry("&Src", "~/src")]).unwrap();
        let config = save_setting(&path, &Setting::editor("vim")).unwrap();
        assert_eq!(config.editor.as_deref(), Some("vim"));
        assert_eq!(config.hotlist, [entry("&Src", "~/src")]);
    }

    #[test]
    fn save_hotlist_keeps_hand_edits_made_since_startup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        Config::load_or_create(&path).unwrap();
        // Edited by hand while the app runs.
        let edited = fs::read_to_string(&path)
            .unwrap()
            .replace("icons = true", "icons = false");
        fs::write(&path, edited).unwrap();

        let config = save_hotlist(&path, &[entry("a", "/a")]).unwrap();
        assert!(!config.icons);
        assert!(fs::read_to_string(&path).unwrap().contains("icons = false"));
    }

    #[test]
    fn an_empty_hotlist_removes_the_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(
            &path,
            "log = true\n\n[[hotlist]]\nname = \"a\"\npath = \"/a\"\n",
        )
        .unwrap();
        let config = save_hotlist(&path, &[]).unwrap();
        assert!(config.hotlist.is_empty());
        let saved = fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("hotlist"), "{saved}");
        assert!(saved.contains("log = true"));
    }

    #[test]
    fn save_hotlist_creates_the_template_first() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new/config.toml");
        let config = save_hotlist(&path, &[entry("a", "/a")]).unwrap();
        assert_eq!(config.hotlist.len(), 1);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .starts_with("# yagni-commander settings")
        );
    }

    #[test]
    fn save_hotlist_never_writes_a_broken_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "log = maybe\n").unwrap();
        assert!(save_hotlist(&path, &[entry("a", "/a")]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "log = maybe\n");
    }

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
    fn unknown_keys_and_tables_are_ignored_for_other_versions() {
        let text = "log = true\nfuture = 1\n[[future_list]]\na = 1\n[future_table]\nb = 2\n";
        let config: Config = toml::from_str(text).unwrap();
        assert!(config.log);
    }

    #[test]
    fn unknown_keys_are_listed_for_the_status_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(
            &path,
            "case_sensitve_sort = true\nlog = true\n[[zed]]\na = 1\n",
        )
        .unwrap();
        let config = Config::load_or_create(&path).unwrap();
        assert!(config.log);
        assert_eq!(config.ignored, ["case_sensitve_sort", "zed"]);
        assert_eq!(
            config.ignored_notice().unwrap(),
            "Ignored unknown settings: case_sensitve_sort, zed"
        );
        fs::write(&path, "future = 1\n").unwrap();
        assert_eq!(
            Config::load_or_create(&path)
                .unwrap()
                .ignored_notice()
                .unwrap(),
            "Ignored unknown setting: future"
        );
    }

    #[test]
    fn every_known_key_and_the_template_count_as_known() {
        let text = "editor = \"e\"\ncase_sensitive_sort = true\nlog = true\nlog_keep_days = 1\nshow_modified = true\nshow_owner = true\nshow_permissions = true\nicons = true\ngit = true\ncheck_updates = true\ntheme = \"t\"\n[[hotlist]]\npath = \"/x\"\n[[commands]]\ncommand = \"c\"\n";
        assert_eq!(unknown_keys(text), Vec::<String>::new());
        assert_eq!(unknown_keys(TEMPLATE), Vec::<String>::new());
    }

    #[test]
    fn saving_keeps_what_another_version_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "future = 1\n\n[[future_list]]\na = 1\n").unwrap();
        save_setting(&path, &Setting::Log(true)).unwrap();
        save_hotlist(&path, &[entry("n", "/x")]).unwrap();
        save_commands(
            &path,
            &[UserCommand {
                name: "c".into(),
                command: "x".into(),
            }],
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("future = 1"), "{text}");
        assert!(text.contains("[[future_list]]"), "{text}");
        let config: Config = toml::from_str(&text).unwrap();
        assert!(config.log);
        assert_eq!(config.hotlist.len(), 1);
        assert_eq!(config.commands.len(), 1);
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
            git: false,
            check_updates: true,
            theme: Some("classic".into()),
            hotlist: vec![entry("&Src", "~/src")],
            commands: Vec::new(),
            ignored: Vec::new(),
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
    fn the_git_setting_is_on_by_default_saved_and_set() {
        let config: Config = toml::from_str("editor = \"vim\"").unwrap();
        assert!(config.git);
        assert!(TEMPLATE.lines().any(|l| l == "git = true"));
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let config = save_setting(&path, &Setting::Git(false)).unwrap();
        assert!(!config.git);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.lines().any(|l| l == "git = false"), "{text}");
        assert!(text.starts_with("# my settings"));
    }

    #[test]
    fn checking_for_updates_is_off_by_default_saved_and_set() {
        let config: Config = toml::from_str("editor = \"vim\"").unwrap();
        assert!(!config.check_updates);
        assert!(TEMPLATE.lines().any(|l| l == "check_updates = false"));
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, HAND_WRITTEN).unwrap();
        let config = save_setting(&path, &Setting::CheckUpdates(true)).unwrap();
        assert!(config.check_updates);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.lines().any(|l| l == "check_updates = true"), "{text}");
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

    #[test]
    fn theme_round_trips_and_defaults_to_none() {
        assert_eq!(toml::from_str::<Config>("").unwrap().theme, None);
        let config: Config = toml::from_str("theme = \"classic\"\n").unwrap();
        assert_eq!(config.theme.as_deref(), Some("classic"));
    }

    #[test]
    fn saving_the_theme_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# mine\neditor = \"vim\"\n").unwrap();
        let config = save_setting(&path, &Setting::Theme("classic".into())).unwrap();
        assert_eq!(config.theme.as_deref(), Some("classic"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("theme = \"classic\""), "{text}");
    }

    #[test]
    fn a_non_string_theme_is_a_config_error() {
        assert!(toml::from_str::<Config>("theme = 3\n").is_err());
    }

    #[test]
    fn the_template_mentions_the_theme_and_still_means_the_default() {
        assert!(TEMPLATE.contains("# theme = \"tokyo-night\""));
        assert_eq!(toml::from_str::<Config>(TEMPLATE).unwrap().theme, None);
    }
}
