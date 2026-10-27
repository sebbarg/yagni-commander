//! Linux: a file's MIME type and its parents, from shared-mime-info.
//! xdg-mime guesses the type (globs and magic); the parents come from the
//! `subclasses` and `aliases` files, read here, since xdg-mime 0.4's own
//! lookup only works for aliases.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};

use xdg_mime::SharedMimeInfo;

use super::desktop::Env;
use crate::viewer::{FileSource, Source};

/// How much of the file the magic rules see.
const HEAD: u64 = 64 * 1024;

const TEXT: &str = "text/plain";
const BINARY: &str = "application/octet-stream";
const ZERO_SIZE: &str = "application/x-zerosize";

/// The type of the file open in `source`, then its parents, nearest first.
pub fn types_of(source: &FileSource, path: &Path, env: &Env) -> io::Result<Vec<String>> {
    let mut data = vec![0; source.len().min(HEAD) as usize];
    let mut read = 0;
    while read < data.len() {
        match source.read_at(&mut data[read..], read as u64)? {
            0 => break,
            n => read += n,
        }
    }
    data.truncate(read);
    // xdg-mime panics without a data folder (no home).
    if dirs::data_dir().is_none() {
        return Err(io::Error::other("no data folder to find file types in"));
    }
    let db = SharedMimeInfo::new();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let dirs: Vec<PathBuf> = env.data().map(|dir| dir.join("mime")).collect();
    Ok(Tree::read(&dirs).lineage(&guess(&db, name, &data)))
}

/// The type by name and content. No metadata goes in: with it xdg-mime calls
/// every empty file `application/x-zerosize`, whatever its name.
fn guess(db: &SharedMimeInfo, name: &str, data: &[u8]) -> String {
    let guessed = db
        .guess_mime_type()
        .file_name(name)
        .data(data)
        .guess()
        .mime_type()
        .essence_str()
        .to_owned();
    // shared-mime-info's rule, which xdg-mime skips for unknown names. Its
    // `application/x-zerosize` (for data no rule matched, too) is never
    // what an application registers for.
    match guessed.as_str() {
        BINARY | ZERO_SIZE if looks_like_text(data) => TEXT.to_owned(),
        ZERO_SIZE => BINARY.to_owned(),
        _ => guessed,
    }
}

/// No control characters but whitespace in the first 128 bytes (an empty
/// file too).
fn looks_like_text(data: &[u8]) -> bool {
    !data
        .iter()
        .take(128)
        .any(|b| b.is_ascii_control() && !b.is_ascii_whitespace())
}

/// The `aliases` and `subclasses` of every `mime` folder.
#[derive(Debug, Default)]
struct Tree {
    aliases: HashMap<String, String>,
    parents: HashMap<String, Vec<String>>,
}

impl Tree {
    fn read(dirs: &[PathBuf]) -> Self {
        let mut tree = Tree::default();
        for dir in dirs {
            for (alias, kind) in pairs(&dir.join("aliases")) {
                tree.aliases.entry(alias).or_insert(kind);
            }
            for (kind, parent) in pairs(&dir.join("subclasses")) {
                let parents = tree.parents.entry(kind).or_default();
                if !parents.contains(&parent) {
                    parents.push(parent);
                }
            }
        }
        tree
    }

    fn canonical(&self, kind: &str) -> String {
        self.aliases
            .get(kind)
            .cloned()
            .unwrap_or_else(|| kind.to_owned())
    }

    /// `kind`, then its parents breadth first; `text/plain` for any
    /// `text/*`, and `application/octet-stream` last for any file.
    fn lineage(&self, kind: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut queue = VecDeque::from([self.canonical(kind)]);
        while let Some(kind) = queue.pop_front() {
            if out.contains(&kind) {
                continue;
            }
            for parent in self.parents.get(&kind).into_iter().flatten() {
                queue.push_back(self.canonical(parent));
            }
            out.push(kind);
        }
        if out.iter().any(|k| k.starts_with("text/")) && !out.iter().any(|k| k == TEXT) {
            out.push(TEXT.to_owned());
        }
        if !out.iter().any(|k| k == BINARY) {
            out.push(BINARY.to_owned());
        }
        out
    }
}

/// The two-word lines of a shared-mime-info file; nothing if unreadable.
fn pairs(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            match (words.next(), words.next()) {
                (Some(a), Some(b)) if !a.starts_with('#') => Some((a.to_owned(), b.to_owned())),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A data folder with a small `mime` database.
    fn database() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let mime = tmp.path().join("mime");
        fs::create_dir_all(&mime).unwrap();
        fs::write(
            mime.join("globs2"),
            "# comment\n50:text/x-csrc:*.c\n50:application/pdf:*.pdf\n50:text/x-old:*.old\n",
        )
        .unwrap();
        fs::write(
            mime.join("subclasses"),
            "text/x-csrc text/plain\napplication/x-sub application/zip\n",
        )
        .unwrap();
        fs::write(mime.join("aliases"), "text/x-old text/x-csrc\n").unwrap();
        tmp
    }

    fn types(name: &str, data: &[u8]) -> Vec<String> {
        let tmp = database();
        let db = SharedMimeInfo::new_for_directory(tmp.path());
        Tree::read(&[tmp.path().join("mime")]).lineage(&guess(&db, name, data))
    }

    #[test]
    fn a_type_by_name_then_its_parents() {
        assert_eq!(
            types("main.c", b"int main;"),
            ["text/x-csrc", "text/plain", "application/octet-stream"]
        );
        assert_eq!(
            types("a.pdf", b"%PDF-1.7"),
            ["application/pdf", "application/octet-stream"]
        );
    }

    #[test]
    fn an_alias_counts_as_its_type() {
        assert_eq!(
            types("x.old", b"x"),
            ["text/x-csrc", "text/plain", "application/octet-stream"]
        );
    }

    #[test]
    fn unknown_text_and_empty_files_are_plain_text() {
        let plain = ["text/plain", "application/octet-stream"];
        assert_eq!(types("README", b"just words\n"), plain);
        assert_eq!(types("empty.c", b""), types("main.c", b""));
        assert_eq!(types("empty", b""), plain);
        assert_eq!(types("blob", b"\x00\x01\x02"), ["application/octet-stream"]);
    }

    #[test]
    fn lineage_goes_breadth_first_without_repeats() {
        let tree = Tree {
            aliases: HashMap::new(),
            parents: HashMap::from([
                ("a/x".into(), vec!["a/y".into(), "text/z".into()]),
                ("a/y".into(), vec!["text/z".into()]),
            ]),
        };
        assert_eq!(
            tree.lineage("a/x"),
            [
                "a/x",
                "a/y",
                "text/z",
                "text/plain",
                "application/octet-stream"
            ]
        );
    }

    #[test]
    fn the_first_folders_alias_wins_and_parents_add_up() {
        let a = database();
        let b = tempfile::tempdir().unwrap();
        fs::write(b.path().join("aliases"), "text/x-old text/other\n").unwrap();
        fs::write(b.path().join("subclasses"), "text/x-csrc text/x-base\n").unwrap();
        let tree = Tree::read(&[a.path().join("mime"), b.path().to_path_buf()]);
        assert_eq!(tree.canonical("text/x-old"), "text/x-csrc");
        assert_eq!(tree.parents["text/x-csrc"], ["text/plain", "text/x-base"]);
    }

    #[test]
    fn types_of_reads_the_head_of_a_file() {
        let tmp = database();
        let file = tmp.path().join("notes");
        fs::write(&file, "plain words\n").unwrap();
        let env = Env {
            data_home: Some(tmp.path().to_path_buf()),
            ..Env::default()
        };
        let source = FileSource::open(&file).unwrap();
        let types = types_of(&source, &file, &env).unwrap();
        assert_eq!(types.first().map(String::as_str), Some("text/plain"));
    }
}
