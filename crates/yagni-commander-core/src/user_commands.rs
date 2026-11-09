//! User commands (Ctrl-S): programs the user lists to open the entry under
//! the cursor with, for file types nothing is registered for. A command is
//! split into words here and started directly, never through a shell, so a
//! file name can't change what runs.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apps::App;

/// What a parameter asks for, after its `active_` or `inactive_`.
const PARTS: [(&str, Part, &str); 5] = [
    (
        "path",
        Part::Path,
        "full path of the entry under the cursor",
    ),
    ("name", Part::Name, "its name, with the extension"),
    ("stem", Part::Stem, "its name, without the extension"),
    ("ext", Part::Ext, "its extension, without the dot"),
    ("dir", Part::Dir, "full path of the panel's folder"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Path,
    Name,
    Stem,
    Ext,
    Dir,
}

/// Every parameter with what it stands for, for the help box.
pub fn parameters() -> Vec<(String, String)> {
    ["active", "inactive"]
        .iter()
        .flat_map(|side| {
            PARTS
                .iter()
                .map(move |(part, _, text)| (format!("{{{side}_{part}}}"), (*text).to_owned()))
        })
        .collect()
}

/// A panel as a command sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The entry under the cursor; on ".." the panel's folder.
    pub entry: PathBuf,
    /// The panel's folder.
    pub dir: PathBuf,
}

impl Place {
    fn part(&self, part: Part) -> &OsStr {
        let empty = OsStr::new("");
        match part {
            Part::Path => self.entry.as_os_str(),
            Part::Name => self.entry.file_name().unwrap_or(empty),
            Part::Stem => self.entry.file_stem().unwrap_or(empty),
            Part::Ext => self.entry.extension().unwrap_or(empty),
            Part::Dir => self.dir.as_os_str(),
        }
    }
}

/// What a command runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panels {
    pub active: Place,
    /// `None` while the other panel is not a local folder.
    pub inactive: Option<Place>,
    /// Where the program starts.
    pub cwd: PathBuf,
}

/// One piece of a word.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Parameter { active: bool, part: Part },
}

const UNKNOWN: &str = "Unknown parameter: see Help for the list";
const NOT_LOCAL: &str = "The other panel is not a local folder";

/// `word` cut at its parameters. `{word}` in lowercase letters and
/// underscores must be a parameter, so a misspelled one is refused instead
/// of reaching the program as text; so must anything starting `{active_` or
/// `{inactive_`. Other braces are plain text: `{}` for find, `{print $1}`
/// for awk.
fn pieces(word: &str) -> Result<Vec<Piece<'_>>, &'static str> {
    let mut out = Vec::new();
    let mut rest = word;
    while let Some(start) = rest.find('{') {
        let inner = &rest[start + 1..];
        let end = inner.find('}');
        let plain_word = end.is_some_and(|end| {
            end > 0
                && inner[..end]
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
        });
        if !plain_word && !inner.starts_with("active_") && !inner.starts_with("inactive_") {
            out.push(Piece::Text(&rest[..=start]));
            rest = inner;
            continue;
        }
        let end = end.ok_or(UNKNOWN)?;
        let (active, name) = match inner[..end].strip_prefix("active_") {
            Some(name) => (true, name),
            None => (
                false,
                inner[..end].strip_prefix("inactive_").ok_or(UNKNOWN)?,
            ),
        };
        let (_, part, _) = PARTS.iter().find(|(n, ..)| *n == name).ok_or(UNKNOWN)?;
        out.push(Piece::Text(&rest[..start]));
        out.push(Piece::Parameter {
            active,
            part: *part,
        });
        rest = &inner[end + 1..];
    }
    out.push(Piece::Text(rest));
    Ok(out)
}

fn has_parameter(pieces: &[Piece<'_>]) -> bool {
    pieces.iter().any(|p| matches!(p, Piece::Parameter { .. }))
}

/// One command, as stored in `config.toml` (`[[commands]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserCommand {
    /// `&` marks the letter, as in the hotlist. May be left out: the popup
    /// then shows the command.
    #[serde(default)]
    pub name: String,
    pub command: String,
}

/// The command's words. Spaces separate them; `"..."` and `'...'` keep
/// spaces in a word; a backslash outside single quotes takes the next
/// character as it is. `None` when a quote is left open or a backslash ends
/// the text.
fn split(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), _) if ch == q => quote = None,
            (Some('\''), _) => word.push(ch),
            (_, '\\') => {
                word.push(chars.next()?);
                started = true;
            }
            (Some(_), _) => word.push(ch),
            (None, '"' | '\'') => {
                quote = Some(ch);
                started = true;
            }
            (None, _) if ch.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, _) => {
                word.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}

/// The words of a usable command, or why it can't run.
fn checked(text: &str) -> Result<Vec<String>, &'static str> {
    let words = split(text).ok_or("A quote is left open, or a backslash ends the command")?;
    for word in &words {
        pieces(word)?;
    }
    match words.first() {
        None => Err("Enter a command"),
        Some(program) if program.is_empty() || has_parameter(&pieces(program)?) => {
            Err("A command must start with a program")
        }
        Some(_) => Ok(words),
    }
}

/// The Configure dialog's check of a typed command.
pub fn validate(text: &str) -> Result<(), &'static str> {
    checked(text).map(|_| ())
}

/// `pieces` joined, the parameters filled in from `panels`. A value need
/// not be UTF-8.
fn filled(pieces: &[Piece<'_>], panels: &Panels) -> Result<OsString, &'static str> {
    let mut out = OsString::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push(text),
            Piece::Parameter { active: true, part } => out.push(panels.active.part(*part)),
            Piece::Parameter { part, .. } => {
                out.push(panels.inactive.as_ref().ok_or(NOT_LOCAL)?.part(*part));
            }
        }
    }
    Ok(out)
}

/// `command` as a program to start on `panels`, with exactly the arguments
/// written: nothing is added. A program written `~/...` is under `home`,
/// since no shell expands it.
pub fn prepare(command: &str, panels: &Panels, home: &Path) -> Result<App, &'static str> {
    let mut words = checked(command)?.into_iter();
    let name = words.next().unwrap_or_default();
    let program = match name.strip_prefix("~/") {
        Some(rest) => home.join(rest).into_os_string(),
        None => OsString::from(&name),
    };
    let mut args = Vec::new();
    for word in words {
        args.push(filled(&pieces(&word)?, panels)?);
    }
    Ok(App {
        name,
        default: false,
        program,
        args,
        dir: panels.cwd.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn place(entry: &str, dir: &str) -> Place {
        Place {
            entry: entry.into(),
            dir: dir.into(),
        }
    }

    fn panels(file: &str) -> Panels {
        Panels {
            active: place(file, "/f"),
            inactive: Some(place("/o/b.tar.gz", "/o")),
            cwd: "/d".into(),
        }
    }

    fn args_for(command: &str, file: &str) -> (OsString, Vec<OsString>) {
        let app = prepare(command, &panels(file), Path::new("/h")).unwrap();
        (app.program, app.args)
    }

    fn os(texts: &[&str]) -> Vec<OsString> {
        texts.iter().map(OsString::from).collect()
    }

    #[test]
    fn words_are_split_on_spaces_and_kept_by_quotes() {
        let words = split(r#"open -a "DB Browser" 'a "b"' c\ d "e\"f" """#).unwrap();
        assert_eq!(
            words,
            ["open", "-a", "DB Browser", "a \"b\"", "c d", "e\"f", ""]
        );
        assert_eq!(split(r"'a\b'").unwrap(), [r"a\b"]);
        assert_eq!(split("  ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn an_open_quote_or_a_last_backslash_is_refused() {
        for text in ["a \"b", "a 'b", "a\\"] {
            assert!(validate(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_command_needs_a_program() {
        assert_eq!(validate("  "), Err("Enter a command"));
        for text in ["\"\" x", "{active_path}", "a{inactive_dir} b"] {
            assert_eq!(validate(text), Err("A command must start with a program"));
        }
        assert_eq!(validate("sqlitebrowser {active_path}"), Ok(()));
    }

    #[test]
    fn every_parameter_is_filled_in() {
        let command = "x {active_path} {active_name} {active_stem} {active_ext} {active_dir} \
                       {inactive_path} {inactive_name} {inactive_stem} {inactive_ext} {inactive_dir}";
        assert_eq!(
            args_for(command, "/f/a.db").1,
            os(&[
                "/f/a.db",
                "a.db",
                "a",
                "db",
                "/f",
                "/o/b.tar.gz",
                "b.tar.gz",
                "b.tar",
                "gz",
                "/o",
            ])
        );
    }

    #[test]
    fn the_help_lists_every_parameter_and_they_all_work() {
        let list = parameters();
        assert_eq!(list.len(), 10);
        for (name, _) in list {
            assert_eq!(validate(&format!("x {name}")), Ok(()), "{name}");
        }
    }

    #[test]
    fn a_name_without_an_extension_has_an_empty_one() {
        assert_eq!(
            args_for("x {active_stem} {active_ext}", "/f/.bashrc").1,
            os(&[".bashrc", ""])
        );
        assert_eq!(args_for("x {active_name}", "/").1, os(&[""]));
    }

    #[test]
    fn parameters_work_inside_a_word_and_more_than_once() {
        let (program, args) = args_for(
            "x --db={active_path} {active_stem}.{active_stem}",
            "/f/a.db",
        );
        assert_eq!(program, "x");
        assert_eq!(args, os(&["--db=/f/a.db", "a.a"]));
    }

    #[test]
    fn nothing_is_added_to_a_command() {
        assert_eq!(args_for("x", "/f/a.db").1, Vec::<OsString>::new());
        assert_eq!(args_for("x -r", "/f/a.db").1, os(&["-r"]));
        assert_eq!(
            args_for("x -C {active_dir}", "/f/a.db").1,
            os(&["-C", "/f"])
        );
    }

    #[test]
    fn other_braces_are_plain_text() {
        assert_eq!(
            args_for("x a{ {} {1..3} {Foo} '{ print $1 }' {active_name}", "/f/a").1,
            os(&["a{", "{}", "{1..3}", "{Foo}", "{ print $1 }", "a"])
        );
    }

    #[test]
    fn a_misspelled_parameter_is_refused() {
        for text in [
            "x {active_pth}",
            "x {inactive_}",
            "x {active_path",
            "x {active_1}",
            "x {file_{active_x}",
            "x {file}",
            "x {acitve_path}",
            "x {active}",
            "x {print}",
        ] {
            assert_eq!(validate(text), Err(UNKNOWN), "{text}");
        }
    }

    #[test]
    fn the_other_panel_must_be_local_only_if_it_is_used() {
        let mut p = panels("/f/a");
        p.inactive = None;
        let home = Path::new("/h");
        assert_eq!(
            prepare("x {inactive_dir}", &p, home).unwrap_err(),
            NOT_LOCAL
        );
        assert_eq!(
            prepare("x {active_name}", &p, home).unwrap().args,
            os(&["a"])
        );
    }

    #[test]
    fn a_file_name_stays_one_argument_whatever_it_holds() {
        let file = "/f/a b; rm -rf \"$HOME\".db";
        assert_eq!(args_for("x {active_path}", file).1, os(&[file]));
    }

    #[test]
    fn a_file_name_need_not_be_utf8() {
        let file = PathBuf::from(OsString::from_vec(b"/f/\xff.db".to_vec()));
        let mut p = panels("/f");
        p.active.entry = file.clone();
        let app = prepare("x {active_path}", &p, Path::new("/h")).unwrap();
        assert_eq!(app.args, [file.into_os_string()]);
    }

    #[test]
    fn a_program_under_home_is_expanded() {
        assert_eq!(args_for("~/bin/x", "/f").0, "/h/bin/x");
        assert_eq!(args_for("~x/y", "/f").0, "~x/y");
    }

    #[test]
    fn it_starts_in_the_folder_given_and_is_named_by_its_program() {
        let app = prepare("x", &panels("/f"), Path::new("/h")).unwrap();
        assert_eq!(app.dir, Path::new("/d"));
        assert_eq!(app.name, "x");
        assert!(!app.default);
    }

    #[test]
    fn a_bad_command_is_not_prepared() {
        assert!(prepare("a \"b", &panels("/f"), Path::new("/h")).is_err());
    }
}
