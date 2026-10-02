//! File icons: a Nerd Font glyph for each entry, chosen by name and
//! extension like `eza --icons`. The app bundles the font
//! (`assets/fonts/SymbolsNerdFontMono-Regular.ttf` in the app crate).

mod table;

/// The bundled font's family name; the app draws icons in it.
pub const FONT_FAMILY: &str = "Symbols Nerd Font Mono";

/// nf-fa-folder.
const FOLDER: char = '\u{f07b}';
/// nf-fa-arrow_up, for "..".
const PARENT: char = '\u{f062}';
/// nf-fa-file: files the table doesn't know, broken symlinks.
const FILE: char = '\u{f15b}';
/// nf-oct-terminal: executables the table doesn't know.
const EXECUTABLE: char = '\u{f489}';

/// Folders with their own icon, matched by exact name. Sorted by name.
const SPECIAL_FOLDERS: &[(&str, char)] = &[
    (".cargo", '\u{e68b}'),       // seti-rust
    (".config", '\u{e5fc}'),      // custom-folder_config
    (".git", '\u{e5fb}'),         // custom-folder_git_branch
    (".github", '\u{e5fd}'),      // custom-folder_github
    (".ssh", '\u{f08ac}'),        // md-folder_key
    ("Desktop", '\u{f108}'),      // fa-desktop
    ("Documents", '\u{f0c82}'),   // md-folder_text
    ("Downloads", '\u{f024d}'),   // md-folder_download
    ("Library", '\u{f0331}'),     // md-library
    ("Music", '\u{f1359}'),       // md-folder_music
    ("Pictures", '\u{f024f}'),    // md-folder_image
    ("Videos", '\u{f19fa}'),      // md-folder_play
    ("docs", '\u{f0c82}'),        // md-folder_text
    ("node_modules", '\u{e5fa}'), // custom-folder_npm
    ("src", '\u{f08de}'),         // md-folder_edit
    ("target", '\u{f107f}'),      // md-folder_cog
    ("tests", '\u{f0668}'),       // md-test_tube
];

use crate::{Entry, EntryKind};

/// The icon for `entry`, from what it already holds (no file system access).
/// Files: exact name, the name in lowercase, compound extensions longest
/// first ("d.ts" before "ts"), then executable, then a generic file.
pub fn icon(entry: &Entry) -> char {
    match entry.kind {
        EntryKind::Parent => PARENT,
        EntryKind::Dir => find(SPECIAL_FOLDERS, &entry.label).unwrap_or(FOLDER),
        EntryKind::File => file_icon(entry),
    }
}

fn file_icon(entry: &Entry) -> char {
    // Broken symlink: no target to describe.
    if entry.is_symlink && entry.size.is_none() && entry.modified.is_none() {
        return FILE;
    }
    let name = entry.label.as_str();
    let lower = name.to_lowercase();
    find(table::FILE_NAMES, name)
        .or_else(|| find(table::FILE_NAMES, &lower))
        .or_else(|| extensions(&lower).find_map(|ext| find(table::EXTENSIONS, ext)))
        .or_else(|| is_executable(entry).then_some(EXECUTABLE))
        .unwrap_or(FILE)
}

/// "a.d.ts" gives "d.ts", then "ts". A leading dot doesn't start one.
fn extensions(name: &str) -> impl Iterator<Item = &str> {
    let start = usize::from(name.starts_with('.'));
    name[start..]
        .match_indices('.')
        .map(move |(i, _)| &name[start + i + 1..])
        .filter(|ext| !ext.is_empty())
}

/// A symlink's own mode is always rwxrwxrwx, so links never count.
fn is_executable(entry: &Entry) -> bool {
    !entry.is_symlink && entry.mode.is_some_and(|mode| mode & 0o111 != 0)
}

fn find(table: &[(&str, char)], key: &str) -> Option<char> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|i| table[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, EntryKind};
    use std::ffi::OsString;
    use std::time::SystemTime;

    const FONT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../yagni-commander/assets/fonts/SymbolsNerdFontMono-Regular.ttf"
    ));

    fn face() -> ttf_parser::Face<'static> {
        ttf_parser::Face::parse(FONT, 0).unwrap()
    }

    fn sorted_and_unique(table: &[(&str, char)]) -> bool {
        table.windows(2).all(|w| w[0].0 < w[1].0)
    }

    #[test]
    fn tables_are_sorted_with_unique_keys() {
        assert!(sorted_and_unique(table::FILE_NAMES));
        assert!(sorted_and_unique(table::EXTENSIONS));
        assert!(sorted_and_unique(SPECIAL_FOLDERS));
    }

    #[test]
    fn every_glyph_is_in_the_bundled_font() {
        let face = face();
        let fixed = [
            ("FOLDER", FOLDER),
            ("PARENT", PARENT),
            ("FILE", FILE),
            ("EXECUTABLE", EXECUTABLE),
        ];
        let all = table::FILE_NAMES
            .iter()
            .chain(table::EXTENSIONS)
            .chain(SPECIAL_FOLDERS)
            .copied()
            .chain(fixed);
        let missing: Vec<_> = all
            .filter(|(_, c)| face.glyph_index(*c).is_none())
            .map(|(k, c)| format!("{k} U+{:04X}", c as u32))
            .collect();
        assert!(missing.is_empty(), "not in the font: {missing:?}");
    }

    #[test]
    fn the_font_maps_m_so_gpui_keeps_it() {
        // gpui's Linux text system drops any face without a glyph for 'm'
        // (cosmic_text_system.rs, `load_family`). See scripts/patch-icon-font.py.
        assert!(face().glyph_index('m').is_some());
    }

    #[test]
    fn the_font_family_is_the_one_the_app_asks_for() {
        let face = face();
        let family = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
            .and_then(|n| n.to_string());
        assert_eq!(family.as_deref(), Some(FONT_FAMILY));
    }

    fn entry(label: &str, kind: EntryKind) -> Entry {
        Entry {
            name: OsString::from(label),
            label: label.into(),
            sort_name: label.to_lowercase(),
            kind,
            is_symlink: false,
            size: (kind == EntryKind::File).then_some(1),
            modified: Some(SystemTime::UNIX_EPOCH),
            mode: Some(0o100644),
            owner: None,
        }
    }

    fn file(label: &str) -> Entry {
        entry(label, EntryKind::File)
    }

    fn in_table(table: &[(&str, char)], key: &str) -> char {
        table[table.binary_search_by(|(k, _)| k.cmp(&key)).unwrap()].1
    }

    #[test]
    fn parent_gets_the_up_arrow() {
        assert_eq!(icon(&Entry::parent()), PARENT);
    }

    #[test]
    fn folders_are_plain_unless_special_by_exact_name() {
        assert_eq!(icon(&entry("a", EntryKind::Dir)), FOLDER);
        assert_eq!(
            icon(&entry(".git", EntryKind::Dir)),
            in_table(SPECIAL_FOLDERS, ".git")
        );
        assert_eq!(
            icon(&entry("Documents", EntryKind::Dir)),
            in_table(SPECIAL_FOLDERS, "Documents")
        );
        assert_eq!(
            icon(&entry("documents", EntryKind::Dir)),
            FOLDER,
            "case-sensitive"
        );
        // A folder named like a known file is still a folder.
        assert_eq!(icon(&entry("Dockerfile", EntryKind::Dir)), FOLDER);
    }

    #[test]
    fn exact_file_name_beats_extension() {
        assert_eq!(
            icon(&file("PKGBUILD")),
            in_table(table::FILE_NAMES, "PKGBUILD")
        );
        assert_eq!(
            icon(&file(".gitignore")),
            in_table(table::FILE_NAMES, ".gitignore")
        );
        let by_name = in_table(table::FILE_NAMES, "compose.yaml");
        assert_ne!(by_name, in_table(table::EXTENSIONS, "yaml"));
        assert_eq!(icon(&file("compose.yaml")), by_name);
        // A mixed-case key matches exactly.
        let freecad = in_table(table::FILE_NAMES, "FreeCAD.conf");
        assert_ne!(freecad, in_table(table::EXTENSIONS, "conf"));
        assert_eq!(icon(&file("FreeCAD.conf")), freecad);
    }

    #[test]
    fn names_match_lowercase_keys_in_any_case() {
        assert_eq!(
            icon(&file("DOCKERFILE")),
            in_table(table::FILE_NAMES, "dockerfile")
        );
        assert_eq!(icon(&file("Main.RS")), in_table(table::EXTENSIONS, "rs"));
        // Non-ASCII uppercase lowercases too and must not panic.
        assert_eq!(icon(&file("ÄRGER.TXT")), in_table(table::EXTENSIONS, "txt"));
    }

    #[test]
    fn compound_extensions_win_over_the_last_one() {
        let spec = in_table(table::EXTENSIONS, "spec.ts");
        assert_ne!(spec, in_table(table::EXTENSIONS, "ts"));
        assert_eq!(icon(&file("app.spec.ts")), spec);
        // "tar.gz" is not a key, so the last extension decides.
        assert_eq!(icon(&file("x.tar.gz")), in_table(table::EXTENSIONS, "gz"));
        assert_eq!(icon(&file("main.rs")), in_table(table::EXTENSIONS, "rs"));
    }

    #[test]
    fn a_leading_dot_is_not_an_extension() {
        // ".bashrc" is in FILE_NAMES; a dot file not in it has no extension.
        assert_eq!(icon(&file(".unknownrc")), FILE);
        assert_eq!(
            icon(&file(".env.toml")),
            in_table(table::EXTENSIONS, "toml")
        );
        assert_eq!(icon(&file(".tar.gz")), in_table(table::EXTENSIONS, "gz"));
    }

    #[test]
    fn odd_names_fall_back_to_the_generic_file() {
        assert_eq!(icon(&file("foo.")), FILE);
        assert_eq!(icon(&file("no_extension")), FILE);
        assert_eq!(icon(&file("x.unknownext")), FILE);
        assert_eq!(icon(&file("bad\u{fffd}name")), FILE, "lossy label");
        assert_eq!(icon(&file("")), FILE);
    }

    #[test]
    fn executables_without_a_table_match_get_the_terminal() {
        let mut tool = file("tool");
        tool.mode = Some(0o100755);
        assert_eq!(icon(&tool), EXECUTABLE);
        let mut script = file("run.sh");
        script.mode = Some(0o100755);
        assert_eq!(
            icon(&script),
            in_table(table::EXTENSIONS, "sh"),
            "the table wins"
        );
        let mut unknown_mode = file("tool");
        unknown_mode.mode = None;
        assert_eq!(icon(&unknown_mode), FILE);
    }

    #[test]
    fn symlinks_follow_their_target_but_are_never_executable() {
        // A symlink's own mode is always rwxrwxrwx.
        let mut link = file("tool");
        link.is_symlink = true;
        link.mode = Some(0o120777);
        assert_eq!(icon(&link), FILE);
        let mut link_to_rs = file("lib.rs");
        link_to_rs.is_symlink = true;
        link_to_rs.mode = Some(0o120777);
        assert_eq!(icon(&link_to_rs), in_table(table::EXTENSIONS, "rs"));
        let mut link_to_dir = entry("src", EntryKind::Dir);
        link_to_dir.is_symlink = true;
        assert_eq!(icon(&link_to_dir), in_table(SPECIAL_FOLDERS, "src"));
    }

    #[test]
    fn broken_symlinks_get_the_generic_file() {
        let mut broken = file("lib.rs");
        broken.is_symlink = true;
        broken.size = None;
        broken.modified = None;
        assert_eq!(icon(&broken), FILE);
    }
}
