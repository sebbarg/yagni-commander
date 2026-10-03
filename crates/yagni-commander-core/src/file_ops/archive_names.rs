//! Archive names: which formats we read, the folder an archive extracts
//! into, and the checks on the names inside an archive.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
}

/// Archive extensions, longest first, so `.tar.gz` is matched before `.tar`.
const EXTENSIONS: [(&str, Format); 10] = [
    (".tar.bz2", Format::TarBz2),
    (".tar.zst", Format::TarZst),
    (".tar.gz", Format::TarGz),
    (".tar.xz", Format::TarXz),
    (".tbz2", Format::TarBz2),
    (".tzst", Format::TarZst),
    (".tgz", Format::TarGz),
    (".txz", Format::TarXz),
    (".tar", Format::Tar),
    (".zip", Format::Zip),
];

/// The archive extension's length in bytes and its format.
fn extension(name: &OsStr) -> Option<(usize, Format)> {
    let lower = name.to_string_lossy().to_lowercase();
    EXTENSIONS
        .iter()
        .find(|(ext, _)| lower.ends_with(ext))
        .map(|(ext, format)| (ext.len(), *format))
}

impl Format {
    pub fn from_name(name: &OsStr) -> Option<Format> {
        extension(name).map(|(_, format)| format)
    }
}

pub fn is_archive(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| Format::from_name(name).is_some())
}

/// The folder an archive extracts into when it has no single top folder:
/// its name without the archive extension (`archive` if nothing is left).
pub(crate) fn stem(name: &OsStr) -> OsString {
    let bytes = name.as_bytes();
    let cut = extension(name).map_or(bytes.len(), |(len, _)| bytes.len() - len);
    // "." or ".." (from "..zip", "...zip") would name the target or its
    // parent.
    match &bytes[..cut] {
        b"" | b"." | b".." => OsString::from("archive"),
        rest => OsString::from_vec(rest.to_vec()),
    }
}

/// Splits an entry name into path components. Refuses names that would
/// leave the extraction folder: absolute, `..`, and in zips (written on
/// Windows too) drive letters and backslashes. `.` and empty parts are
/// dropped; nothing left is an error.
pub(crate) fn components(name: &[u8], zip: bool) -> Result<Vec<OsString>, &'static str> {
    if name.first() == Some(&b'/') {
        return Err("an absolute path in an archive is not allowed");
    }
    if zip {
        if name.contains(&b'\\') {
            return Err("a backslash path in a zip is not allowed");
        }
        if name.len() >= 2 && name[1] == b':' && name[0].is_ascii_alphabetic() {
            return Err("a drive letter in a zip is not allowed");
        }
    }
    let mut parts = Vec::new();
    for part in name.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => return Err("“..” in an archive path is not allowed"),
            part => parts.push(OsString::from_vec(part.to_vec())),
        }
    }
    if parts.is_empty() {
        return Err("an empty name in an archive");
    }
    Ok(parts)
}

/// Whether every entry sits under one top-level folder (smart extraction).
/// `entries` are (components, is a folder).
pub(crate) fn single_top_folder(entries: &[(Vec<OsString>, bool)]) -> bool {
    let Some((first, _)) = entries.first() else {
        return false;
    };
    let top = &first[0];
    entries
        .iter()
        .all(|(parts, is_dir)| &parts[0] == top && (parts.len() > 1 || *is_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    #[test]
    fn formats_come_from_the_name_ignoring_case() {
        for (name, format) in [
            ("a.zip", Format::Zip),
            ("A.ZIP", Format::Zip),
            ("a.tar", Format::Tar),
            ("a.tar.gz", Format::TarGz),
            ("a.tgz", Format::TarGz),
            ("a.tar.bz2", Format::TarBz2),
            ("a.tbz2", Format::TarBz2),
            ("a.tar.xz", Format::TarXz),
            ("a.txz", Format::TarXz),
            ("a.tar.zst", Format::TarZst),
            ("a.TZST", Format::TarZst),
        ] {
            assert_eq!(Format::from_name(os(name)), Some(format), "{name}");
        }
        for name in ["a.gz", "a.7z", "a.rar", "zip", "a.zipx", "a.txt", ""] {
            assert_eq!(Format::from_name(os(name)), None, "{name}");
        }
        assert!(is_archive(Path::new("/x/y.tar.zst")));
        assert!(!is_archive(Path::new("/x/y.txt")));
        assert!(!is_archive(Path::new("/")));
    }

    #[test]
    fn the_stem_drops_the_archive_extension() {
        assert_eq!(stem(os("photos.zip")), "photos");
        assert_eq!(stem(os("src-1.2.tar.gz")), "src-1.2");
        assert_eq!(stem(os("x.TGZ")), "x");
        assert_eq!(stem(os(".zip")), "archive");
        assert_eq!(stem(os("plain")), "plain");
    }

    fn ok(name: &str) -> Vec<String> {
        components(name.as_bytes(), true)
            .unwrap()
            .into_iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn names_split_into_components() {
        assert_eq!(ok("a/b/c.txt"), ["a", "b", "c.txt"]);
        assert_eq!(ok("./a//b/"), ["a", "b"]);
        assert_eq!(ok("a/./b"), ["a", "b"]);
    }

    #[test]
    fn escaping_names_are_refused() {
        let why = |name: &str, zip: bool| components(name.as_bytes(), zip).unwrap_err();
        assert_eq!(
            why("/etc/passwd", true),
            "an absolute path in an archive is not allowed"
        );
        assert_eq!(why("../x", true), "“..” in an archive path is not allowed");
        assert_eq!(
            why("a/../../x", true),
            "“..” in an archive path is not allowed"
        );
        assert_eq!(why("a/..", true), "“..” in an archive path is not allowed");
        assert_eq!(why("", true), "an empty name in an archive");
        assert_eq!(why("./", true), "an empty name in an archive");
        assert_eq!(why("C:/x", true), "a drive letter in a zip is not allowed");
        assert_eq!(why("c:x", true), "a drive letter in a zip is not allowed");
        assert_eq!(
            why("a\\b", true),
            "a backslash path in a zip is not allowed"
        );
        // A backslash and a colon are ordinary in tar (Unix) names.
        assert!(components(b"a\\b", false).is_ok());
        assert!(components(b"c:x", false).is_ok());
        assert!(components(b"../x", false).is_err());
        assert!(components(b"/x", false).is_err());
    }

    #[test]
    fn non_utf8_tar_names_are_kept_as_bytes() {
        let parts = components(b"dir/caf\xe9", false).unwrap();
        assert_eq!(parts[1].as_bytes(), b"caf\xe9");
    }

    fn entries(list: &[(&str, bool)]) -> Vec<(Vec<OsString>, bool)> {
        list.iter()
            .map(|(name, dir)| (components(name.as_bytes(), true).unwrap(), *dir))
            .collect()
    }

    #[test]
    fn one_top_folder_is_detected() {
        assert!(single_top_folder(&entries(&[("p/", true), ("p/a", false)])));
        assert!(single_top_folder(&entries(&[
            ("p/a", false),
            ("p/b/c", false)
        ])));
        assert!(single_top_folder(&entries(&[("p/", true)])));
        assert!(!single_top_folder(&entries(&[
            ("p/a", false),
            ("q", false)
        ])));
        assert!(!single_top_folder(&entries(&[("a.txt", false)])));
        assert!(!single_top_folder(&entries(&[])));
        // A file named like the folder at the top level breaks it.
        assert!(!single_top_folder(&entries(&[
            ("p", false),
            ("p/a", false)
        ])));
    }

    #[test]
    fn a_stem_of_dots_is_never_a_folder_reference() {
        assert_eq!(stem(os("...zip")), "archive");
        assert_eq!(stem(os("..zip")), "archive");
        assert_eq!(stem(os("...tar.gz")), "archive");
    }
}
