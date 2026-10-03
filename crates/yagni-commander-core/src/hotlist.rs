//! The directory hotlist (Ctrl-D): bookmarked folders. Names mark their
//! hotkey letter with `&`, like Total Commander (`&Projects`: P; `&&` is a
//! literal `&`). Paths are stored as typed, with `~` for home.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One hotlist entry, as stored in `config.toml` (`[[hotlist]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotlistEntry {
    /// May be left out by hand: the popup then shows the folder's name.
    #[serde(default)]
    pub name: String,
    pub path: String,
}

/// A name ready to show: the text without its `&` marks, the hotkey letter
/// (lowercase) and the byte range of `text` to underline.
#[derive(Debug, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    pub letter: Option<char>,
    pub underline: Option<Range<usize>>,
}

pub fn label(name: &str) -> Label {
    let mut text = String::with_capacity(name.len());
    let mut letter = None;
    let mut underline = None;
    let mut chars = name.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            text.push(ch);
            continue;
        }
        match chars.next() {
            Some('&') => text.push('&'),
            Some(next) => {
                if letter.is_none() {
                    letter = next.to_lowercase().next();
                    underline = Some(text.len()..text.len() + next.len_utf8());
                }
                text.push(next);
            }
            None => text.push('&'), // a trailing lone `&` is shown as is
        }
    }
    Label {
        text,
        letter,
        underline,
    }
}

/// What the popup and the dialog show for `entry`: its name, or the
/// folder's name (no letter) when the name is empty.
pub fn display_label(entry: &HotlistEntry) -> Label {
    if entry.name.is_empty() {
        return Label {
            text: default_name(&entry.path),
            letter: None,
            underline: None,
        };
    }
    label(&entry.name)
}

/// The first entry whose letter is `ch`, ignoring case.
pub fn find_letter(entries: &[HotlistEntry], ch: char) -> Option<usize> {
    let ch = ch.to_lowercase().next()?;
    entries
        .iter()
        .position(|e| label(&e.name).letter == Some(ch))
}

/// The folder an entry's path names: `~` and `~/...` under `home`, an
/// absolute path as is, anything else relative to `home`.
pub fn expand(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    }
}

/// How "Add current folder" stores `path`: inside `home` as `~` or
/// `~/...`, so a config copied between macOS and Linux still works.
pub fn contract(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The Configure dialog's check of a typed path.
pub fn validate(path: &str) -> Result<(), &'static str> {
    if path == "~" || path.starts_with("~/") || Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err("A folder must be an absolute path or start with ~/")
    }
}

/// The last component of `path` (`/` for the root, `~` for home).
pub fn default_name(path: &str) -> String {
    match Path::new(path).components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        Some(Component::RootDir) | None if path.starts_with('/') => "/".to_owned(),
        _ => path.to_owned(),
    }
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
    fn an_ampersand_marks_the_next_character() {
        let l = label("&Projects");
        assert_eq!(l.text, "Projects");
        assert_eq!(l.letter, Some('p'));
        assert_eq!(l.underline, Some(0..1));
        let l = label("My &docs");
        assert_eq!(l.text, "My docs");
        assert_eq!(l.letter, Some('d'));
        assert_eq!(l.underline, Some(3..4));
    }

    #[test]
    fn a_double_ampersand_is_a_literal_one() {
        let l = label("R&&D");
        assert_eq!(l.text, "R&D");
        assert_eq!(l.letter, None);
        assert_eq!(l.underline, None);
        let l = label("R&&&D");
        assert_eq!(l.text, "R&D");
        assert_eq!(l.letter, Some('d'));
        assert_eq!(l.underline, Some(2..3));
    }

    #[test]
    fn odd_ampersands_never_panic_or_mark_anything() {
        for (name, text) in [("&", "&"), ("tail&", "tail&"), ("&&", "&"), ("", "")] {
            let l = label(name);
            assert_eq!(l.text, text, "{name:?}");
            assert_eq!(l.letter, None, "{name:?}");
            assert_eq!(l.underline, None, "{name:?}");
        }
    }

    #[test]
    fn only_the_first_marked_letter_counts() {
        let l = label("&A&B");
        assert_eq!(l.text, "AB");
        assert_eq!(l.letter, Some('a'));
        assert_eq!(l.underline, Some(0..1));
    }

    #[test]
    fn non_ascii_letters_are_lowercased_and_underlined_by_bytes() {
        let l = label("B&ü");
        assert_eq!(l.text, "Bü");
        assert_eq!(l.letter, Some('ü'));
        assert_eq!(l.underline, Some(1..3)); // 'ü' is two bytes
        let l = label("&Øvelse");
        assert_eq!(l.letter, Some('ø'));
        assert_eq!(l.underline, Some(0..2));
    }

    #[test]
    fn find_letter_ignores_case_and_takes_the_first() {
        let entries = [
            entry("Plain", "/a"),
            entry("&Beta", "/b"),
            entry("&bin", "/c"),
            entry("&Øst", "/d"),
        ];
        assert_eq!(find_letter(&entries, 'b'), Some(1));
        assert_eq!(find_letter(&entries, 'B'), Some(1));
        assert_eq!(find_letter(&entries, 'Ø'), Some(3));
        assert_eq!(find_letter(&entries, 'p'), None, "no & in Plain");
        assert_eq!(find_letter(&entries, 'x'), None);
    }

    #[test]
    fn an_empty_name_shows_the_folder_name() {
        assert_eq!(display_label(&entry("", "~/src/app")).text, "app");
        assert_eq!(display_label(&entry("", "/")).text, "/");
        assert_eq!(display_label(&entry("", "~")).text, "~");
        assert_eq!(display_label(&entry("&Src", "~/src")).text, "Src");
        assert_eq!(display_label(&entry("", "~/src")).letter, None);
    }

    #[test]
    fn expand_resolves_home_and_relative_paths() {
        let home = Path::new("/home/me");
        assert_eq!(expand("~", home), PathBuf::from("/home/me"));
        assert_eq!(expand("~/src", home), PathBuf::from("/home/me/src"));
        assert_eq!(expand("/etc", home), PathBuf::from("/etc"));
        // A hand-edited relative path is relative to home, not to the
        // app's working directory.
        assert_eq!(expand("src", home), PathBuf::from("/home/me/src"));
        assert_eq!(expand("~other", home), PathBuf::from("/home/me/~other"));
    }

    #[test]
    fn contract_writes_home_as_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(contract(Path::new("/home/me"), home), "~");
        assert_eq!(contract(Path::new("/home/me/src/app"), home), "~/src/app");
        assert_eq!(contract(Path::new("/home/meow"), home), "/home/meow");
        assert_eq!(contract(Path::new("/etc"), home), "/etc");
    }

    #[test]
    fn validate_takes_absolute_and_home_paths_only() {
        for ok in ["/", "/etc", "~", "~/src"] {
            assert_eq!(validate(ok), Ok(()), "{ok}");
        }
        for bad in ["", "  ", "src", "./src", "~other"] {
            assert!(validate(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn default_name_is_the_last_component() {
        assert_eq!(default_name("/home/me/src"), "src");
        assert_eq!(default_name("~/src/"), "src");
        assert_eq!(default_name("/"), "/");
        assert_eq!(default_name("~"), "~");
    }
}
