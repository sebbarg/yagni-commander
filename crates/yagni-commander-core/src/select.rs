//! Num+ and Num-: selecting entries by a mask, like TC's "Select group".
//!
//! The syntax is Alt-F7's (`*.rs;*.toml | test*`): masks separated by `;`
//! or spaces, a mask without `*` or `?` means `*text*`, and those after a
//! `|` exclude. Unlike Alt-F7 a mask matches files only, unless it ends in
//! `/`: then it matches folders only (`*/` is every folder, `src/` one
//! named like it). Names are compared ignoring case.

use crate::find::masks::wildcard;

/// The text the select prompt starts with the first time: every file.
pub const DEFAULT_MASK: &str = "*";

/// A parsed select mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectMask {
    include: Vec<Mask>,
    exclude: Vec<Mask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mask {
    pattern: Vec<char>,
    /// Ended in `/`: matches folders, else files.
    folders: bool,
}

impl SelectMask {
    /// `None` when nothing is left to include (an empty text, or only
    /// excludes).
    pub fn parse(text: &str) -> Option<Self> {
        let (include, exclude) = text.split_once('|').unwrap_or((text, ""));
        let include: Vec<Mask> = masks(include, true).collect();
        if include.is_empty() {
            return None;
        }
        Some(Self {
            include,
            exclude: masks(exclude, false).collect(),
        })
    }

    /// Whether an entry named `name` (a file name, not a path) is matched.
    pub fn matches(&self, name: &str, is_dir: bool) -> bool {
        let name: Vec<char> = name.to_lowercase().chars().collect();
        let hit = |masks: &[Mask]| {
            masks
                .iter()
                .any(|m| m.folders == is_dir && wildcard(&m.pattern, &name))
        };
        hit(&self.include) && !hit(&self.exclude)
    }
}

/// The masks in `text`; with `implied`, one without a wildcard means
/// `*text*` (excludes are taken as written, like Alt-F7's).
fn masks(text: &str, implied: bool) -> impl Iterator<Item = Mask> + '_ {
    text.split(|c: char| c == ';' || c.is_whitespace())
        .filter_map(move |token| {
            let folders = token.ends_with('/');
            let token = token.trim_end_matches('/').to_lowercase();
            if token.is_empty() {
                // A lone `/` means every folder.
                return folders.then(|| Mask {
                    pattern: vec!['*'],
                    folders,
                });
            }
            let pattern = if implied && !token.contains(['*', '?']) {
                format!("*{token}*")
            } else {
                token
            };
            Some(Mask {
                pattern: pattern.chars().collect(),
                folders,
            })
        })
}

/// The mask Alt-Num+ uses for a file named `name`: `*.ext` for its last
/// extension, or `None` for a name without one (also `.bashrc`).
pub fn extension_mask(name: &str) -> Option<String> {
    let dot = name.rfind('.').filter(|&dot| dot > 0)?;
    let ext = &name[dot + 1..];
    (!ext.is_empty()).then(|| format!("*.{ext}"))
}

/// Whether a file named `name` has no extension, as [`extension_mask`] sees
/// it.
pub fn has_no_extension(name: &str) -> bool {
    extension_mask(name).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(text: &str) -> SelectMask {
        SelectMask::parse(text).unwrap()
    }

    #[test]
    fn masks_match_files_unless_they_end_in_a_slash() {
        let m = mask("*.rs");
        assert!(m.matches("main.rs", false));
        assert!(!m.matches("main.rs", true));
        let m = mask("*/");
        assert!(m.matches("src", true));
        assert!(!m.matches("src", false));
        let m = mask("/");
        assert!(m.matches("src", true));
        assert!(!m.matches("a.txt", false));
    }

    #[test]
    fn a_bare_word_means_anywhere_in_the_name_ignoring_case() {
        let m = mask("Read");
        assert!(m.matches("README.md", false));
        assert!(m.matches("unread", false));
        assert!(!m.matches("readme", true));
        let m = mask("src/");
        assert!(m.matches("old-src", true));
    }

    #[test]
    fn several_masks_and_excludes() {
        let m = mask("*.rs;*.toml target/ | test* old/");
        assert!(m.matches("lib.rs", false));
        assert!(m.matches("Cargo.toml", false));
        assert!(m.matches("target", true));
        assert!(!m.matches("tests.rs", false));
        assert!(!m.matches("notes.md", false));
        // An exclude ending in `/` leaves files alone.
        let m = mask("* */ | old/");
        assert!(m.matches("old", false));
        assert!(!m.matches("old", true));
        assert!(m.matches("new", true));
    }

    #[test]
    fn excludes_are_taken_as_written() {
        let m = mask("* | test");
        assert!(m.matches("tests", false));
        assert!(!m.matches("test", false));
    }

    #[test]
    fn nothing_to_include_is_no_mask() {
        assert_eq!(SelectMask::parse(""), None);
        assert_eq!(SelectMask::parse("  ; "), None);
        assert_eq!(SelectMask::parse("| *.rs"), None);
    }

    #[test]
    fn the_extension_mask_takes_the_last_extension() {
        assert_eq!(extension_mask("a.tar.gz").as_deref(), Some("*.gz"));
        assert_eq!(extension_mask("main.rs").as_deref(), Some("*.rs"));
        assert_eq!(extension_mask("Makefile"), None);
        assert_eq!(extension_mask(".bashrc"), None);
        assert_eq!(extension_mask("trailing."), None);
        assert!(has_no_extension("Makefile"));
        assert!(!has_no_extension("a.txt"));
    }
}
