//! Linux: the applications for a file's MIME types, by the freedesktop
//! Desktop Entry and MIME Applications Associations specs, so the list is
//! the one GNOME, KDE and the others build from the same files.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::App;
use crate::terminal::find_in_path;

/// What the lookup depends on, read from the process by [`Env::current`];
/// tests build their own.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `$XDG_CONFIG_HOME`.
    pub config_home: Option<PathBuf>,
    /// `$XDG_CONFIG_DIRS`.
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_DATA_HOME`.
    pub data_home: Option<PathBuf>,
    /// `$XDG_DATA_DIRS`.
    pub data_dirs: Vec<PathBuf>,
    /// `$XDG_CURRENT_DESKTOP`'s names (`KDE`, `GNOME`, `Hyprland`).
    pub desktops: Vec<String>,
    /// `$PATH`, for `TryExec` and `xdg-terminal-exec`.
    pub path: Option<OsString>,
    /// `$LC_ALL`, else `$LC_MESSAGES`, else `$LANG`.
    pub locale: Option<String>,
}

impl Env {
    /// This process's environment, with the base directory spec's defaults
    /// for unset (or relative) folders.
    pub fn current() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let home = dirs::home_dir();
        let one = |name: &str, fallback: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| home.as_ref().map(|home| home.join(fallback)))
        };
        let list = |name: &str, fallback: &str| {
            let dirs: Vec<PathBuf> = var(name)
                .map(|v| {
                    std::env::split_paths(&v)
                        .filter(|p| p.is_absolute())
                        .collect()
                })
                .unwrap_or_default();
            if dirs.is_empty() {
                std::env::split_paths(fallback).collect()
            } else {
                dirs
            }
        };
        Self {
            config_home: one("XDG_CONFIG_HOME", ".config"),
            config_dirs: list("XDG_CONFIG_DIRS", "/etc/xdg"),
            data_home: one("XDG_DATA_HOME", ".local/share"),
            data_dirs: list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            desktops: std::env::var("XDG_CURRENT_DESKTOP")
                .map(|d| {
                    d.split(':')
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            path: var("PATH"),
            locale: ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty())),
        }
    }

    /// `$XDG_DATA_HOME`, then `$XDG_DATA_DIRS`: where `applications` and
    /// `mime` are looked up, the first one winning.
    pub fn data(&self) -> impl Iterator<Item = &PathBuf> {
        self.data_home.iter().chain(&self.data_dirs)
    }

    /// The `mimeapps.list` files, most important first: the config folders,
    /// then `applications` in the data folders; in each, one per desktop
    /// name (`kde-mimeapps.list`), then the plain one.
    fn mimeapps_lists(&self) -> Vec<PathBuf> {
        let names: Vec<String> = self
            .desktops
            .iter()
            .map(|d| format!("{}-mimeapps.list", d.to_lowercase()))
            .chain(["mimeapps.list".to_owned()])
            .collect();
        let config = self.config_home.iter().chain(&self.config_dirs).cloned();
        let data = self.data().map(|dir| dir.join("applications"));
        config
            .chain(data)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .collect()
    }
}

/// The applications for a file of `types` (its MIME type, then each parent),
/// unordered except that the default is marked. The file goes into each
/// command line.
pub fn apps(env: &Env, types: &[String], file: &Path) -> Vec<App> {
    let entries = entries(env);
    let lists: Vec<MimeApps> = env
        .mimeapps_lists()
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| MimeApps::parse(&text))
        .collect();
    let terminal = find_in_path("xdg-terminal-exec", env.path.as_deref());
    let usable = |id: &str| {
        entries
            .get(id)
            .filter(|entry| entry.usable(env, terminal.is_some()))
    };
    let default = types.iter().find_map(|kind| {
        lists
            .iter()
            .flat_map(|list| list.defaults.get(kind).into_iter().flatten())
            .find(|id| usable(id).is_some())
            .cloned()
    });
    // The spec's algorithm, with one blacklist for the whole lookup, as GIO
    // does: an application removed for a type stays out for its parents.
    let mut ids: Vec<String> = Vec::new();
    let mut removed: HashSet<&String> = HashSet::new();
    let add = |ids: &mut Vec<String>, removed: &HashSet<&String>, id: &String| {
        if !removed.contains(id) && !ids.contains(id) {
            ids.push(id.clone());
        }
    };
    for kind in types {
        for list in &lists {
            for id in list.added.get(kind).into_iter().flatten() {
                add(&mut ids, &removed, id);
            }
            removed.extend(list.removed.get(kind).into_iter().flatten());
        }
        let mut supporting: Vec<&String> = entries
            .iter()
            .filter(|(_, entry)| entry.mime_types.contains(kind))
            .map(|(id, _)| id)
            .collect();
        supporting.sort();
        for id in supporting {
            add(&mut ids, &removed, id);
        }
    }
    if let Some(default) = &default
        && !ids.contains(default)
    {
        ids.push(default.clone());
    }
    ids.iter()
        .filter_map(|id| {
            let entry = usable(id)?;
            let is_default = default.as_ref() == Some(id);
            if entry.no_display && !is_default {
                return None;
            }
            entry.app(id, env, file, is_default, terminal.as_deref())
        })
        .collect()
}

/// Every desktop file under `applications` in the data folders, by id (its
/// path below `applications`, `/` turned into `-`). The first folder
/// holding an id wins, so a user's copy hides the system's.
fn entries(env: &Env) -> HashMap<String, Entry> {
    let mut found = HashMap::new();
    for dir in env.data() {
        scan(&dir.join("applications"), "", &mut found);
    }
    found
}

fn scan(dir: &Path, prefix: &str, found: &mut HashMap<String, Entry>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let id = format!("{prefix}{name}");
        // Links to folders are not followed (no loops); links to files are
        // (Flatpak exports are links).
        if item.file_type().is_ok_and(|t| t.is_dir()) {
            scan(&item.path(), &format!("{id}-"), found);
        } else if name.ends_with(".desktop")
            && !found.contains_key(&id)
            && let Ok(text) = std::fs::read_to_string(item.path())
        {
            found.insert(id, Entry::parse(&text, item.path()));
        }
    }
}

/// The `[Desktop Entry]` keys we use.
#[derive(Debug, Default)]
struct Entry {
    path: PathBuf,
    /// `Name` under "", `Name[de_DE]` under "de_DE".
    names: HashMap<String, String>,
    exec: Option<String>,
    try_exec: Option<String>,
    working_dir: Option<String>,
    icon: Option<String>,
    mime_types: Vec<String>,
    only_show_in: Vec<String>,
    not_show_in: Vec<String>,
    application: bool,
    hidden: bool,
    no_display: bool,
    terminal: bool,
}

impl Entry {
    fn parse(text: &str, path: PathBuf) -> Self {
        let mut entry = Entry {
            path,
            ..Entry::default()
        };
        let mut in_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_group = line == "[Desktop Entry]";
                continue;
            }
            let Some((key, value)) = line.split_once('=').filter(|_| in_group) else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if let Some(locale) = key.strip_prefix("Name[").and_then(|k| k.strip_suffix(']')) {
                entry.names.insert(locale.to_owned(), unescape(value));
                continue;
            }
            let text = || Some(unescape(value)).filter(|v| !v.is_empty());
            match key {
                "Name" => {
                    entry.names.insert(String::new(), unescape(value));
                }
                "Type" => entry.application = value == "Application",
                "Exec" => entry.exec = text(),
                "TryExec" => entry.try_exec = text(),
                "Path" => entry.working_dir = text(),
                "Icon" => entry.icon = text(),
                "MimeType" => entry.mime_types = list(value),
                "OnlyShowIn" => entry.only_show_in = list(value),
                "NotShowIn" => entry.not_show_in = list(value),
                "Hidden" => entry.hidden = value == "true",
                "NoDisplay" => entry.no_display = value == "true",
                "Terminal" => entry.terminal = value == "true",
                _ => {}
            }
        }
        entry
    }

    /// Whether it may be offered (`NoDisplay` aside, which only the
    /// default overrides).
    fn usable(&self, env: &Env, has_terminal: bool) -> bool {
        let named = |list: &[String]| {
            list.iter()
                .any(|name| env.desktops.iter().any(|d| d.eq_ignore_ascii_case(name)))
        };
        self.application
            && !self.hidden
            && self.exec.is_some()
            && (!self.terminal || has_terminal)
            && (self.only_show_in.is_empty() || named(&self.only_show_in))
            && !named(&self.not_show_in)
            && self
                .try_exec
                .as_ref()
                .is_none_or(|t| find_in_path(t, env.path.as_deref()).is_some())
    }

    /// `Name` in the user's language: `lang_COUNTRY@MODIFIER`,
    /// `lang_COUNTRY`, `lang@MODIFIER`, `lang`, then the plain one.
    fn name(&self, locale: Option<&str>) -> Option<&str> {
        let localized = locale.and_then(|locale| {
            let locale = locale.split('.').next().unwrap_or(locale);
            let (base, modifier) = match locale.split_once('@') {
                Some((base, modifier)) => (base, Some(modifier)),
                None => (locale, None),
            };
            let (lang, country) = match base.split_once('_') {
                Some((lang, country)) => (lang, Some(country)),
                None => (base, None),
            };
            let with = |a: Option<&str>, b: Option<&str>| {
                let mut key = lang.to_owned();
                if let Some(country) = a {
                    key = format!("{key}_{country}");
                }
                if let Some(modifier) = b {
                    key = format!("{key}@{modifier}");
                }
                key
            };
            let mut keys = Vec::new();
            if country.is_some() && modifier.is_some() {
                keys.push(with(country, modifier));
            }
            if country.is_some() {
                keys.push(with(country, None));
            }
            if modifier.is_some() {
                keys.push(with(None, modifier));
            }
            keys.push(lang.to_owned());
            keys.into_iter()
                .find_map(|key| self.names.get(&key).filter(|name| !name.is_empty()))
        });
        localized
            .or_else(|| self.names.get(""))
            .map(String::as_str)
            .filter(|name| !name.is_empty())
    }

    /// The application with its command line for `file`; `None` when `Exec`
    /// doesn't parse.
    fn app(
        &self,
        id: &str,
        env: &Env,
        file: &Path,
        default: bool,
        terminal: Option<&Path>,
    ) -> Option<App> {
        let name = self
            .name(env.locale.as_deref())
            .unwrap_or_else(|| id.strip_suffix(".desktop").unwrap_or(id))
            .to_owned();
        let mut argv = expand(self.exec.as_deref()?, file, &name, self)?;
        if let Some(terminal) = terminal.filter(|_| self.terminal) {
            argv.insert(0, terminal.as_os_str().to_owned());
        }
        let dir = self
            .working_dir
            .as_ref()
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| file.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut argv = argv.into_iter();
        Some(App {
            name,
            default,
            program: argv.next()?,
            args: argv.collect(),
            dir,
        })
    }
}

/// The string escapes of the Desktop Entry spec (`\s`, `\n`, `\t`, `\r`,
/// `\\`). Any other backslash stays, for `Exec`'s own quoting.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A `;`-separated list, empty items dropped.
fn list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `Exec` split into words: double quotes group, and inside them a
/// backslash escapes the next character. Each word says whether any of it
/// was quoted (field codes only count outside quotes). `None` when a quote
/// is left open.
fn split_exec(exec: &str) -> Option<Vec<(String, bool)>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
                '"' => in_quotes = false,
                '\\' => word.push(chars.next()?),
                _ => word.push(ch),
            }
        } else if ch == '"' {
            (in_quotes, quoted, started) = (true, true, true);
        } else if ch.is_whitespace() {
            if started {
                words.push((std::mem::take(&mut word), quoted));
            }
            (started, quoted) = (false, false);
        } else {
            word.push(ch);
            started = true;
        }
    }
    if in_quotes {
        return None;
    }
    if started {
        words.push((word, quoted));
    }
    Some(words)
}

/// `Exec` as a command line for `file`: field codes replaced (`%f %F %u %U`
/// the path, `%i` the icon, `%c` the name, `%k` the desktop file, `%%`),
/// the deprecated ones dropped, and the path added last when no code took
/// it. `None` when it doesn't parse or is empty.
fn expand(exec: &str, file: &Path, name: &str, entry: &Entry) -> Option<Vec<OsString>> {
    let words = split_exec(exec)?;
    if words.is_empty() {
        return None;
    }
    let mut argv = Vec::new();
    let mut took_file = false;
    for (word, quoted) in words {
        if quoted {
            argv.push(OsString::from(word));
            continue;
        }
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                argv.push(file.as_os_str().to_owned());
                took_file = true;
                continue;
            }
            "%i" => {
                if let Some(icon) = &entry.icon {
                    argv.extend(["--icon".into(), OsString::from(icon)]);
                }
                continue;
            }
            _ => {}
        }
        let mut arg = OsString::new();
        let mut chars = word.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                arg.push(ch.encode_utf8(&mut [0; 4]));
                continue;
            }
            match chars.next() {
                Some('%') => arg.push("%"),
                Some('f' | 'F' | 'u' | 'U') => {
                    arg.push(file.as_os_str());
                    took_file = true;
                }
                Some('c') => arg.push(name),
                Some('k') => arg.push(entry.path.as_os_str()),
                // %i inside a word, the deprecated codes, unknown ones.
                _ => {}
            }
        }
        if !arg.is_empty() {
            argv.push(arg);
        }
    }
    if !took_file {
        argv.push(file.as_os_str().to_owned());
    }
    Some(argv)
}

/// The groups of one `mimeapps.list`: application ids by MIME type.
#[derive(Debug, Default)]
struct MimeApps {
    defaults: HashMap<String, Vec<String>>,
    added: HashMap<String, Vec<String>>,
    removed: HashMap<String, Vec<String>>,
}

impl MimeApps {
    fn parse(text: &str) -> Self {
        let mut apps = MimeApps::default();
        let mut group = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                group = match line {
                    "[Default Applications]" => Some(0),
                    "[Added Associations]" => Some(1),
                    "[Removed Associations]" => Some(2),
                    _ => None,
                };
                continue;
            }
            let (Some(group), Some((kind, ids))) = (group, line.split_once('=')) else {
                continue;
            };
            let map = match group {
                0 => &mut apps.defaults,
                1 => &mut apps.added,
                _ => &mut apps.removed,
            };
            map.entry(kind.trim().to_owned())
                .or_default()
                .extend(list(ids));
        }
        apps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// A home with config and data folders, a system data folder and a
    /// `bin` folder for `PATH`.
    struct Fixture {
        tmp: tempfile::TempDir,
        env: Env,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            for dir in [
                "config",
                "etc",
                "home/applications",
                "usr/applications",
                "bin",
            ] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            let env = Env {
                config_home: Some(root.join("config")),
                config_dirs: vec![root.join("etc")],
                data_home: Some(root.join("home")),
                data_dirs: vec![root.join("usr")],
                desktops: vec!["KDE".into()],
                path: Some(root.join("bin").into_os_string()),
                locale: None,
            };
            Fixture { tmp, env }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.tmp.path().join(rel)
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// A system application for `types`, plus `extra` lines.
        fn app(&self, id: &str, name: &str, types: &str, extra: &str) {
            self.write(
                &format!("usr/applications/{id}"),
                &format!(
                    "[Desktop Entry]\nType=Application\nName={name}\nExec={name} %f\nMimeType={types}\n{extra}\n"
                ),
            );
        }

        fn program(&self, name: &str) {
            let path = self.path(&format!("bin/{name}"));
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        fn names(&self, types: &[&str]) -> Vec<String> {
            let mut names: Vec<String> = self
                .apps(types)
                .into_iter()
                .map(|app| {
                    if app.default {
                        format!("{}*", app.name)
                    } else {
                        app.name
                    }
                })
                .collect();
            names.sort();
            names
        }

        fn apps(&self, types: &[&str]) -> Vec<App> {
            let types: Vec<String> = types.iter().map(|t| (*t).to_owned()).collect();
            apps(&self.env, &types, Path::new("/docs/a file.c"))
        }

        fn one(&self, types: &[&str]) -> App {
            let mut apps = self.apps(types);
            assert_eq!(apps.len(), 1, "{apps:?}");
            apps.remove(0)
        }
    }

    const C: &[&str] = &["text/x-csrc", "text/plain", "application/octet-stream"];

    #[test]
    fn apps_for_the_type_and_its_parents() {
        let f = Fixture::new();
        f.app("kate.desktop", "Kate", "text/plain;", "");
        f.app("cdev.desktop", "CDev", "text/x-csrc;text/x-chdr;", "");
        f.app("okular.desktop", "Okular", "application/pdf;", "");
        f.app("hex.desktop", "Hex", "application/octet-stream", "");
        assert_eq!(f.names(C), ["CDev", "Hex", "Kate"]);
        assert_eq!(f.names(&["application/pdf"]), ["Okular"]);
        assert!(f.names(&["image/png"]).is_empty());
    }

    #[test]
    fn the_default_comes_from_default_applications_in_file_order() {
        let f = Fixture::new();
        f.app("kate.desktop", "Kate", "text/plain;", "");
        f.app("gedit.desktop", "Gedit", "text/plain;", "");
        f.app("vim.desktop", "Vim", "text/plain;", "");
        f.write(
            "etc/mimeapps.list",
            "[Default Applications]\ntext/plain=vim.desktop\n",
        );
        assert_eq!(f.names(C), ["Gedit", "Kate", "Vim*"]);
        // The user's file wins; an uninstalled id is passed over.
        f.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=gone.desktop;gedit.desktop\n",
        );
        assert_eq!(f.names(C), ["Gedit*", "Kate", "Vim"]);
        // The desktop's own file wins over the plain one.
        f.write(
            "config/kde-mimeapps.list",
            "[Default Applications]\ntext/plain=kate.desktop\n",
        );
        assert_eq!(f.names(C), ["Gedit", "Kate*", "Vim"]);
        // A default for the type itself wins over its parent's.
        f.write(
            "home/applications/mimeapps.list",
            "[Default Applications]\ntext/x-csrc=vim.desktop\n",
        );
        assert_eq!(f.names(C), ["Gedit", "Kate", "Vim*"]);
    }

    #[test]
    fn added_and_removed_associations() {
        let f = Fixture::new();
        f.app("kate.desktop", "Kate", "text/plain;", "");
        f.app("gimp.desktop", "Gimp", "image/png;", "");
        f.app("vim.desktop", "Vim", "text/plain;", "");
        f.write(
            "config/mimeapps.list",
            "[Added Associations]\ntext/x-csrc=gimp.desktop;\n\
             [Removed Associations]\ntext/x-csrc=vim.desktop;\n",
        );
        // Removed for the type, so not back through its parent either.
        assert_eq!(f.names(C), ["Gimp", "Kate"]);
        // A less important file can't add back what was removed.
        f.write(
            "etc/mimeapps.list",
            "[Added Associations]\ntext/x-csrc=vim.desktop;\n",
        );
        assert_eq!(f.names(C), ["Gimp", "Kate"]);
        // Nor remove what a more important one added.
        f.write(
            "etc/mimeapps.list",
            "[Removed Associations]\ntext/x-csrc=gimp.desktop;\n",
        );
        assert_eq!(f.names(C), ["Gimp", "Kate"]);
    }

    #[test]
    fn the_first_data_folder_holding_an_id_wins() {
        let f = Fixture::new();
        f.app("kate.desktop", "Kate", "text/plain;", "");
        f.app("vim.desktop", "Vim", "text/plain;", "");
        f.write(
            "home/applications/kate.desktop",
            "[Desktop Entry]\nType=Application\nName=My Kate\nExec=kate\nMimeType=text/plain;\n",
        );
        f.write(
            "home/applications/vim.desktop",
            "[Desktop Entry]\nHidden=true\n",
        );
        assert_eq!(f.names(C), ["My Kate"]);
    }

    #[test]
    fn ids_of_files_in_subfolders_join_with_dashes() {
        let f = Fixture::new();
        f.write(
            "usr/applications/kde/kate.desktop",
            "[Desktop Entry]\nType=Application\nName=Kate\nExec=kate\nMimeType=text/plain;\n",
        );
        f.write(
            "usr/applications/kde/vim.desktop",
            "[Desktop Entry]\nType=Application\nName=Vim\nExec=vim\nNoDisplay=true\n",
        );
        f.write(
            "config/mimeapps.list",
            "[Default Applications]\ntext/plain=kde-vim.desktop\n",
        );
        assert_eq!(f.names(C), ["Kate", "Vim*"]);
    }

    #[test]
    fn entries_that_must_not_be_offered_are_left_out() {
        let f = Fixture::new();
        f.app("hidden.desktop", "Hidden", "text/plain;", "NoDisplay=true");
        f.app("link.desktop", "Link", "text/plain;", "Type=Link");
        f.app("gnome.desktop", "Gnome", "text/plain;", "OnlyShowIn=GNOME;");
        f.app(
            "kdeonly.desktop",
            "KdeOnly",
            "text/plain;",
            "OnlyShowIn=GNOME;KDE;",
        );
        f.app("notkde.desktop", "NotKde", "text/plain;", "NotShowIn=KDE;");
        f.app(
            "try.desktop",
            "Try",
            "text/plain;",
            "TryExec=missing-program",
        );
        f.app(
            "found.desktop",
            "Found",
            "text/plain;",
            "TryExec=found-program",
        );
        f.program("found-program");
        f.write(
            "usr/applications/dbus.desktop",
            "[Desktop Entry]\nType=Application\nName=DBus\nDBusActivatable=true\nMimeType=text/plain;\n",
        );
        f.write(
            "usr/applications/other.desktop",
            "[Other Group]\nType=Application\nName=Other\nExec=other\nMimeType=text/plain;\n",
        );
        assert_eq!(f.names(C), ["Found", "KdeOnly"]);
    }

    #[test]
    fn terminal_apps_need_xdg_terminal_exec_and_start_through_it() {
        let f = Fixture::new();
        f.app("nvim.desktop", "nvim", "text/plain;", "Terminal=true");
        assert!(f.apps(C).is_empty());
        f.program("xdg-terminal-exec");
        let app = f.one(C);
        assert_eq!(app.program, f.path("bin/xdg-terminal-exec"));
        assert_eq!(app.args, ["nvim", "/docs/a file.c"]);
    }

    #[test]
    fn names_follow_the_language() {
        let f = Fixture::new();
        f.app(
            "kate.desktop",
            "Kate",
            "text/plain;",
            "Name[de]=Kate DE\nName[de_AT]=Kate AT\nName[sr@latin]=Kate SR\nName[x]=",
        );
        let name = |locale: Option<&str>| {
            let mut f2 = Fixture {
                tmp: tempfile::tempdir().unwrap(),
                env: f.env.clone(),
            };
            f2.env.locale = locale.map(str::to_owned);
            f2.one(C).name
        };
        assert_eq!(name(None), "Kate");
        assert_eq!(name(Some("C")), "Kate");
        assert_eq!(name(Some("de_DE.UTF-8")), "Kate DE");
        assert_eq!(name(Some("de_AT.UTF-8@euro")), "Kate AT");
        assert_eq!(name(Some("sr_RS@latin")), "Kate SR");
        assert_eq!(name(Some("x")), "Kate", "an empty translation is skipped");
        // No Name at all: the id.
        let f = Fixture::new();
        f.write(
            "usr/applications/nameless.desktop",
            "[Desktop Entry]\nType=Application\nExec=x\nMimeType=text/plain;\n",
        );
        assert_eq!(f.one(C).name, "nameless");
    }

    fn command(exec: &str) -> Option<Vec<String>> {
        let entry = Entry {
            path: PathBuf::from("/apps/x.desktop"),
            icon: Some("x-icon".into()),
            ..Entry::default()
        };
        expand(&unescape(exec), Path::new("/d/a b.txt"), "X App", &entry)
            .map(|argv| argv.into_iter().map(|a| a.into_string().unwrap()).collect())
    }

    #[test]
    fn exec_field_codes() {
        assert_eq!(command("x %f").unwrap(), ["x", "/d/a b.txt"]);
        assert_eq!(command("x %U --new").unwrap(), ["x", "/d/a b.txt", "--new"]);
        assert_eq!(command("x --file=%u").unwrap(), ["x", "--file=/d/a b.txt"]);
        assert_eq!(
            command("x %i %c %k %%d %f").unwrap(),
            [
                "x",
                "--icon",
                "x-icon",
                "X App",
                "/apps/x.desktop",
                "%d",
                "/d/a b.txt"
            ]
        );
        // Deprecated and unknown codes are dropped, with a word they empty.
        assert_eq!(
            command("x %d %D %n %N %v %m %z %F").unwrap(),
            ["x", "/d/a b.txt"]
        );
        // No file code: the path goes last.
        assert_eq!(
            command("x --new-window").unwrap(),
            ["x", "--new-window", "/d/a b.txt"]
        );
    }

    #[test]
    fn exec_quoting_and_escapes() {
        // In the file, a backslash is itself escaped (`\\`), so `\\"` is an
        // escaped quote inside a quoted word.
        assert_eq!(
            command(r#""/opt/My App/run" "say \\"hi\\"" "%f" a\sb"#).unwrap(),
            [
                "/opt/My App/run",
                "say \"hi\"",
                "%f",
                "a",
                "b",
                "/d/a b.txt"
            ]
        );
        assert_eq!(
            command(r#"x "a\\\\b""#).unwrap(),
            ["x", r"a\b", "/d/a b.txt"]
        );
        assert_eq!(command("  x   %f  ").unwrap(), ["x", "/d/a b.txt"]);
        assert_eq!(command(r#"x """#).unwrap(), ["x", "", "/d/a b.txt"]);
        assert_eq!(command(r#"x "open"#), None);
        assert_eq!(command("   "), None);
    }

    #[test]
    fn an_entry_whose_exec_does_not_parse_is_left_out() {
        let f = Fixture::new();
        f.write(
            "usr/applications/bad.desktop",
            "[Desktop Entry]\nType=Application\nName=Bad\nExec=\"bad %f\nMimeType=text/plain;\n",
        );
        f.app("kate.desktop", "Kate", "text/plain;", "");
        assert_eq!(f.names(C), ["Kate"]);
    }

    #[test]
    fn apps_start_in_the_files_folder_or_their_path() {
        let f = Fixture::new();
        f.app("kate.desktop", "Kate", "text/plain;", "");
        assert_eq!(f.one(C).dir, Path::new("/docs"));
        f.app("kate.desktop", "Kate", "text/plain;", "Path=/opt/kate");
        assert_eq!(f.one(C).dir, Path::new("/opt/kate"));
        f.app("kate.desktop", "Kate", "text/plain;", "Path=relative");
        assert_eq!(f.one(C).dir, Path::new("/docs"));
    }

    #[test]
    fn mimeapps_lists_in_the_specs_order() {
        let f = Fixture::new();
        let mut env = f.env.clone();
        env.desktops = vec!["ubuntu".into(), "GNOME".into()];
        let rel: Vec<String> = env
            .mimeapps_lists()
            .iter()
            .map(|p| p.strip_prefix(f.tmp.path()).unwrap().display().to_string())
            .collect();
        let per = |dir: &str| {
            [
                "ubuntu-mimeapps.list",
                "gnome-mimeapps.list",
                "mimeapps.list",
            ]
            .map(|name| format!("{dir}/{name}"))
        };
        let expected: Vec<String> = [
            per("config"),
            per("etc"),
            per("home/applications"),
            per("usr/applications"),
        ]
        .concat();
        assert_eq!(rel, expected);
    }

    #[test]
    fn mimeapps_groups_and_comments() {
        let apps = MimeApps::parse(
            "# comment\n[Default Applications]\ntext/plain = a.desktop;b.desktop\n\
             [Other]\nimage/png=x.desktop\n[Added Associations]\nimage/png=c.desktop;\n\
             garbage\n[Removed Associations]\nimage/png=d.desktop\n",
        );
        assert_eq!(apps.defaults["text/plain"], ["a.desktop", "b.desktop"]);
        assert!(!apps.defaults.contains_key("image/png"));
        assert_eq!(apps.added["image/png"], ["c.desktop"]);
        assert_eq!(apps.removed["image/png"], ["d.desktop"]);
    }

    #[test]
    fn string_escapes() {
        assert_eq!(unescape(r"a\sb\nc\td\re\\f\;g\"), "a b\nc\td\re\\f\\;g\\");
    }
}
