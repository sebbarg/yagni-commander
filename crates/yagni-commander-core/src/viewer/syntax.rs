//! Source mode (F3, `S`): which grammar a file gets, and which theme role
//! each highlight capture is drawn in. Parsing and drawing are the app's
//! (gpui-component's tree-sitter highlighter); this decides by name only.

use std::ops::Range;
use std::path::Path;

/// Files up to this size are parsed; larger ones stay plain text. The parser
/// holds the whole file in memory.
pub const SOURCE_CAP: u64 = 4 * 1024 * 1024;

/// The grammars the app is built with: gpui-component's registry name and
/// the name the status line shows. `language` returns only these; an app
/// test checks each is registered.
pub const LANGUAGES: [(&str, &str); 27] = [
    ("bash", "Bash"),
    ("c", "C"),
    ("cpp", "C++"),
    ("csharp", "C#"),
    ("css", "CSS"),
    ("diff", "Diff"),
    ("ejs", "EJS"),
    ("erb", "ERB"),
    ("go", "Go"),
    ("html", "HTML"),
    ("java", "Java"),
    ("javascript", "JavaScript"),
    ("json", "JSON"),
    ("kotlin", "Kotlin"),
    ("lua", "Lua"),
    ("make", "Make"),
    ("markdown", "Markdown"),
    ("proto", "Protocol Buffers"),
    ("python", "Python"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("sql", "SQL"),
    ("swift", "Swift"),
    ("toml", "TOML"),
    ("tsx", "TSX"),
    ("typescript", "TypeScript"),
    ("yaml", "YAML"),
];

/// The name the status line shows for grammar `language`.
pub fn display_name(language: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(n, _)| *n == language)
        .map_or(language, |&(_, shown)| shown)
}

/// Whole file names (compared exactly) that have a grammar.
const NAMES: &[(&str, &str)] = &[
    ("Makefile", "make"),
    ("makefile", "make"),
    ("GNUmakefile", "make"),
    (".bashrc", "bash"),
    (".bash_profile", "bash"),
    (".bash_logout", "bash"),
    (".bash_aliases", "bash"),
    (".profile", "bash"),
    (".zshrc", "bash"),
    (".zprofile", "bash"),
    (".zshenv", "bash"),
    ("PKGBUILD", "bash"),
    ("Rakefile", "ruby"),
    ("Gemfile", "ruby"),
    ("Guardfile", "ruby"),
    ("Podfile", "ruby"),
    ("Vagrantfile", "ruby"),
    ("Brewfile", "ruby"),
    ("Cargo.lock", "toml"),
    ("Pipfile", "toml"),
    ("uv.lock", "toml"),
    ("poetry.lock", "toml"),
];

/// Extensions, lowercase, without the dot.
const EXTENSIONS: &[(&str, &str)] = &[
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("c", "c"),
    // C++ headers are more common than C ones, and the C++ grammar reads C.
    ("h", "cpp"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("cxx", "cpp"),
    ("c++", "cpp"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("h++", "cpp"),
    ("ipp", "cpp"),
    ("cs", "csharp"),
    ("csx", "csharp"),
    ("css", "css"),
    ("diff", "diff"),
    ("patch", "diff"),
    ("ejs", "ejs"),
    ("erb", "erb"),
    ("go", "go"),
    ("html", "html"),
    ("htm", "html"),
    ("xhtml", "html"),
    ("java", "java"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "javascript"),
    ("json", "json"),
    ("jsonc", "json"),
    ("geojson", "json"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("lua", "lua"),
    ("mk", "make"),
    ("mak", "make"),
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("proto", "proto"),
    ("py", "python"),
    ("pyi", "python"),
    ("pyw", "python"),
    ("rb", "ruby"),
    ("rake", "ruby"),
    ("gemspec", "ruby"),
    ("ru", "ruby"),
    ("rs", "rust"),
    ("sql", "sql"),
    ("swift", "swift"),
    ("toml", "toml"),
    ("tsx", "tsx"),
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
];

/// Interpreters on a `#!` line, without a version (`python3.12` is
/// `python`).
const INTERPRETERS: &[(&str, &str)] = &[
    ("sh", "bash"),
    ("bash", "bash"),
    ("dash", "bash"),
    ("ksh", "bash"),
    ("zsh", "bash"),
    ("python", "python"),
    ("ruby", "ruby"),
    ("node", "javascript"),
    ("nodejs", "javascript"),
    ("lua", "lua"),
    ("luajit", "lua"),
    ("make", "make"),
    ("swift", "swift"),
];

/// The grammar for a file named `name` whose text starts with
/// `first_line`: by the whole name, then the extension, then a `#!` line.
pub fn language(name: &str, first_line: &[u8]) -> Option<&'static str> {
    if let Some(&(_, lang)) = NAMES.iter().find(|(n, _)| *n == name) {
        return Some(lang);
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|_| !name.starts_with('.') || name[1..].contains('.'))
        .map(str::to_ascii_lowercase);
    if let Some(ext) = ext {
        return EXTENSIONS
            .iter()
            .find(|(e, _)| *e == ext)
            .map(|&(_, lang)| lang);
    }
    interpreter(first_line)
}

/// The grammar named by a `#!` line: its program, or `env`'s first
/// argument that isn't an option.
fn interpreter(first_line: &[u8]) -> Option<&'static str> {
    let line = first_line.strip_prefix(b"#!")?;
    let line = std::str::from_utf8(line.split(|&b| b == b'\n').next()?).ok()?;
    let mut words = line.split_whitespace();
    let mut program = base_name(words.next()?);
    if program == "env" {
        program = base_name(words.find(|w| !w.starts_with('-') && !w.contains('='))?);
    }
    let program = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    INTERPRETERS
        .iter()
        .find(|(p, _)| *p == program)
        .map(|&(_, lang)| lang)
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The theme role a highlight is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Keyword,
    String,
    Comment,
    Type,
    Function,
    /// Numbers, booleans and named constants.
    Constant,
    /// Keys and fields: object properties, attributes, YAML and TOML keys.
    Property,
    /// Punctuation and operators.
    Punctuation,
    /// A diff's added lines.
    Added,
    /// A diff's removed lines.
    Removed,
}

/// The role of a capture name from a highlight query (`keyword.control`,
/// `string.special.symbol`, ...): the name, then each shorter prefix.
/// `None`: drawn as plain text (variables, for one).
pub fn role(capture: &str) -> Option<Role> {
    let mut name = capture;
    loop {
        if let Some(role) = exact_role(name) {
            return role;
        }
        name = name.rsplit_once('.')?.0;
    }
}

/// `Some(None)`: known, and plain on purpose.
fn exact_role(name: &str) -> Option<Option<Role>> {
    use Role::*;
    let role = match name {
        "keyword" | "conditional" | "repeat" | "include" | "exception" | "preproc"
        | "storageclass" | "tag" | "title" | "label" => Keyword,
        "string" | "character" | "link_uri" | "text.literal" | "text.code.span" | "text.uri" => {
            String
        }
        "string.escape" | "string.regex" | "escape" => Constant,
        "comment" => Comment,
        "type" | "constructor" | "enum" | "namespace" | "module" | "variant" => Type,
        "function" | "method" | "link_text" | "attribute.builtin" => Function,
        "number" | "float" | "boolean" | "constant" => Constant,
        "property" | "field" | "attribute" | "parameter" | "variable.parameter" => Property,
        "punctuation" | "operator" | "delimiter" => Punctuation,
        "diff.plus" | "addition" | "text.diff.add" => Added,
        "diff.minus" | "deletion" | "text.diff.delete" => Removed,
        "variable" | "embedded" | "none" | "spell" | "nospell" | "text" | "emphasis"
        | "primary" | "hint" | "predictive" => return Some(None),
        _ => return None,
    };
    Some(Some(role))
}

/// Puts `spans` (file byte ranges, sorted and not overlapping, each with a
/// caller's class) onto a text row: for each character of `text`, the class
/// of the span holding the byte it shows, as ranges of `text`. `sources`
/// and `start` are the row's (see `Row::sources`).
pub fn row_classes(
    text: &str,
    sources: &[u32],
    start: u64,
    spans: &[(Range<u64>, u16)],
) -> Vec<(Range<usize>, u16)> {
    let mut classes: Vec<(Range<usize>, u16)> = Vec::new();
    let mut span = 0;
    for ((ix, ch), &offset) in text.char_indices().zip(sources) {
        let byte = start + u64::from(offset);
        while spans.get(span).is_some_and(|(r, _)| r.end <= byte) {
            span += 1;
        }
        let Some((_, class)) = spans.get(span).filter(|(r, _)| r.start <= byte) else {
            continue;
        };
        let end = ix + ch.len_utf8();
        match classes.last_mut() {
            Some((r, c)) if *c == *class && r.end == ix => r.end = end,
            _ => classes.push((ix..end, *class)),
        }
    }
    classes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_come_from_the_name_the_extension_or_the_first_line() {
        for (name, lang) in [
            ("main.rs", "rust"),
            ("Program.CS", "csharp"),
            ("a.tar.patch", "diff"),
            ("index.html.erb", "erb"),
            ("types.d.ts", "typescript"),
            ("view.tsx", "tsx"),
            ("Makefile", "make"),
            ("Cargo.lock", "toml"),
            (".bashrc", "bash"),
            (".eslintrc.json", "json"),
            ("x.h", "cpp"),
            ("ci.yml", "yaml"),
        ] {
            assert_eq!(language(name, b""), Some(lang), "{name}");
        }
        // Unknown extensions don't fall back to the first line.
        assert_eq!(language("notes.txt", b"#!/bin/sh\n"), None);
        assert_eq!(language("README", b"hello"), None);
        assert_eq!(language(".gitignore", b"target/\n"), None);
        assert_eq!(language("makefile.bak", b""), None);
    }

    #[test]
    fn a_shebang_names_the_interpreter() {
        for (line, lang) in [
            (&b"#!/bin/sh\necho"[..], "bash"),
            (b"#!/usr/bin/env bash", "bash"),
            (b"#!/usr/bin/env -S python3 -u\n", "python"),
            (b"#!/usr/bin/python3.12", "python"),
            (b"#!/usr/bin/env FOO=1 node", "javascript"),
            (b"#! /usr/bin/ruby -w", "ruby"),
        ] {
            assert_eq!(language("script", line), Some(lang), "{line:?}");
        }
        for line in [
            &b"#!/usr/bin/env"[..],
            b"#!/usr/bin/perl",
            b"# a comment",
            b"#!\xff",
            b"",
        ] {
            assert_eq!(language("script", line), None, "{line:?}");
        }
    }

    #[test]
    fn every_language_returned_is_in_the_list() {
        let returned = NAMES
            .iter()
            .chain(EXTENSIONS)
            .chain(INTERPRETERS)
            .map(|&(_, lang)| lang);
        for lang in returned {
            assert!(
                LANGUAGES.iter().any(|(n, _)| *n == lang),
                "{lang} is not in LANGUAGES"
            );
        }
    }

    #[test]
    fn the_status_line_names_the_language() {
        assert_eq!(display_name("csharp"), "C#");
        assert_eq!(display_name("proto"), "Protocol Buffers");
        assert_eq!(display_name("mystery"), "mystery");
    }

    #[test]
    fn captures_find_their_role_by_the_longest_known_prefix() {
        assert_eq!(role("keyword"), Some(Role::Keyword));
        assert_eq!(role("keyword.control.return"), Some(Role::Keyword));
        assert_eq!(role("string.special.symbol"), Some(Role::String));
        assert_eq!(role("string.escape"), Some(Role::Constant));
        assert_eq!(role("comment.doc"), Some(Role::Comment));
        assert_eq!(role("type.builtin"), Some(Role::Type));
        assert_eq!(role("function.method.call"), Some(Role::Function));
        assert_eq!(role("constant.builtin"), Some(Role::Constant));
        assert_eq!(role("number"), Some(Role::Constant));
        assert_eq!(role("property"), Some(Role::Property));
        assert_eq!(role("punctuation.bracket"), Some(Role::Punctuation));
        assert_eq!(role("operator"), Some(Role::Punctuation));
        assert_eq!(role("tag"), Some(Role::Keyword));
        assert_eq!(role("diff.plus"), Some(Role::Added));
        assert_eq!(role("diff.minus"), Some(Role::Removed));
        // Plain on purpose, and unknown.
        assert_eq!(role("variable"), None);
        assert_eq!(role("variable.member"), None);
        assert_eq!(role("variable.parameter"), Some(Role::Property));
        assert_eq!(role("mystery"), None);
        assert_eq!(role(""), None);
    }

    #[test]
    fn classes_follow_the_characters_onto_the_row() {
        // "\tfn é" shown as four spaces, "fn ", "é": the tab's spaces all
        // point at the tab; é is two bytes.
        let text = "    fn é";
        let sources = [0, 0, 0, 0, 1, 2, 3, 4, 6];
        let spans = [(11..13, 1), (14..16, 2)];
        assert_eq!(
            row_classes(text, &sources, 10, &spans),
            [(4..6, 1), (7..9, 2)]
        );
        // A span over the tab colors its spaces; one before the row and one
        // after it don't matter.
        let spans = [(0..5, 9), (10..12, 3), (20..30, 4)];
        assert_eq!(row_classes(text, &sources, 10, &spans), [(0..5, 3)]);
        assert_eq!(row_classes(text, &sources, 10, &[]), []);
    }
}
