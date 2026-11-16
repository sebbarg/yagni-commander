//! Source mode (`S`, and at open for a file core `viewer::language` knows):
//! the text view with syntax colors. The rows, search, selection and wrap
//! are the text view's own; a parse of the whole file (gpui-component's
//! tree-sitter highlighter) adds colors to the rows on screen.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use gpui_kit::base::input::Rope;
use gpui_kit::base::text::CodeBlock;
use gpui_kit::component::highlighter::{LanguageRegistry, SyntaxHighlighter};
use gpui_kit::{App, Context, HighlightStyle, SharedString, Window};
use yagni_commander_core::format_size;
use yagni_commander_core::viewer::{Role, Row, SOURCE_CAP, display_name, language, role};

use super::ViewerView;
use crate::file_manager::commands::{OPENER_POLL, show_message};
use crate::theme::{Colors, Theme};

/// Highlight queries in place of gpui-component's: C#, Protocol Buffers and
/// Swift have none there (these are each grammar's own crate's), and its
/// Kotlin one doesn't compile against the grammar it ships, so it colored
/// nothing (ours leaves out the nodes the grammar lacks). Sources in
/// THIRD-PARTY-NOTICES.md; `every_language_colors_its_sample` checks them.
const QUERIES: [(&str, &str); 4] = [
    (
        "csharp",
        include_str!("../../assets/queries/csharp-highlights.scm"),
    ),
    (
        "kotlin",
        include_str!("../../assets/queries/kotlin-highlights.scm"),
    ),
    (
        "proto",
        include_str!("../../assets/queries/proto-highlights.scm"),
    ),
    (
        "swift",
        include_str!("../../assets/queries/swift-highlights.scm"),
    ),
];

/// Puts our queries in the registry, once, before the first parse.
fn register_queries() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(register_queries_now);
}

fn register_queries_now() {
    let registry = LanguageRegistry::singleton();
    for (name, query) in QUERIES {
        if let Some(mut config) = registry.language(name) {
            config.highlights = query.into();
            registry.register(name, &config);
        }
    }
}

pub(super) struct SourceMode {
    /// gpui-component's name for the grammar.
    pub(super) language: &'static str,
    /// `None` while the parse runs: the rows show uncolored meanwhile.
    highlighter: Option<SyntaxHighlighter>,
    parsing: Option<Receiver<SyntaxHighlighter>>,
}

impl SourceMode {
    #[cfg(test)]
    pub(super) fn parsed(&self) -> bool {
        self.highlighter.is_some()
    }
}

/// The color of each role in the theme.
struct Palette(Colors);

impl gpui_kit::base::input::HighlightStyleResolver for Palette {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let c = &self.0;
        let color = match role(name)? {
            Role::Keyword => c.syntax_keyword,
            Role::String => c.syntax_string,
            Role::Comment => c.syntax_comment,
            Role::Type => c.syntax_type,
            Role::Function => c.syntax_function,
            Role::Constant => c.syntax_constant,
            Role::Property => c.syntax_property,
            Role::Punctuation => c.syntax_punctuation,
            Role::Added => c.success,
            Role::Removed => c.error,
        };
        Some(HighlightStyle {
            color: Some(color.into()),
            ..Default::default()
        })
    }
}

/// Colors a Markdown code block: its ranges and their styles.
pub(super) type CodeColors =
    dyn Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync;

/// Markdown mode's code block colors for the theme in use, the same as
/// source mode's (gpui-component's own colors are its `highlight_theme`,
/// which we don't set: a light one, unreadable on dark themes). The same
/// `Arc` while the theme stays: gpui-base keeps a block's colors only while
/// it is handed the same highlighter (`Arc::ptr_eq`), and the view is built
/// on every frame, so a new one would parse every block on every frame.
pub(super) fn code_colors(cx: &App) -> Arc<CodeColors> {
    thread_local! {
        static SHARED: RefCell<Option<(String, Arc<CodeColors>)>> = const { RefCell::new(None) };
    }
    let theme = Theme::get(cx);
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        if let Some((name, colors)) = shared.as_ref()
            && *name == theme.name
        {
            return colors.clone();
        }
        register_queries();
        let palette = Palette(theme.colors.clone());
        let colors: Arc<CodeColors> = Arc::new(move |block| color_code(block, &palette));
        *shared = Some((theme.name.clone(), colors.clone()));
        colors
    })
}

/// `block`'s colors by its fence's language (none without one, or for a
/// language without a grammar). One highlighter per language, reused: making
/// one compiles its queries.
fn color_code(block: &CodeBlock, palette: &Palette) -> Vec<(Range<usize>, HighlightStyle)> {
    thread_local! {
        static HIGHLIGHTERS: RefCell<HashMap<SharedString, Option<SyntaxHighlighter>>> =
            RefCell::new(HashMap::new());
    }
    let Some(lang) = block.lang() else {
        return Vec::new();
    };
    let code = block.code();
    HIGHLIGHTERS.with(|highlighters| {
        let mut highlighters = highlighters.borrow_mut();
        let entry = highlighters
            .entry(lang.clone())
            .or_insert_with(|| highlighter_for(&lang));
        let Some(highlighter) = entry else {
            return Vec::new();
        };
        // `None`: the change is unknown, so the new text is parsed as if
        // inserted before the old tree, which leaves nothing to reuse.
        highlighter.update(None, &Rope::from(code.as_ref()), None);
        highlighter.styles(&(0..code.len()), palette)
    })
}

/// A highlighter for a fence's language, if the registry knows it. Not
/// `SyntaxHighlighter::new` alone: for an unknown name (` ```mermaid `) it
/// logs a warning and falls back to plain text, which colors nothing anyway.
fn highlighter_for(lang: &str) -> Option<SyntaxHighlighter> {
    LanguageRegistry::singleton()
        .language(lang)
        .is_some()
        .then(|| SyntaxHighlighter::new(lang))
}

/// Why `bytes` (all of a file of `len` bytes named `name`) can't be shown in
/// source mode, or its grammar. `bytes` is read only when the rest allows.
fn grammar_for(
    name: &str,
    len: u64,
    first_line: &[u8],
    bytes: impl FnOnce() -> Vec<u8>,
) -> Result<(&'static str, String), String> {
    let Some(language) = language(name, first_line) else {
        return Err("Source mode has no grammar for this kind of file.".to_owned());
    };
    if len > SOURCE_CAP {
        return Err(format!(
            "Source mode shows files up to {}; this one is {}.",
            format_size(SOURCE_CAP),
            format_size(len)
        ));
    }
    String::from_utf8(bytes())
        .map(|text| (language, text))
        .map_err(|_| "Source mode needs UTF-8 text; this file isn't.".to_owned())
}

impl ViewerView {
    /// `S`: source mode, out of any other mode (from Markdown mode at the
    /// place it showed). A file it can't show stays in its mode, with a box
    /// saying why.
    pub(super) fn show_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.try_source(window, cx) {
            Ok(()) => {
                self.leave_picture(window, cx);
                self.leave_markdown(cx);
                if self.hex {
                    self.hex = false;
                    self.relayout(cx);
                }
                cx.notify();
            }
            Err(message) => {
                let back = Some(self.focus.clone());
                show_message("Source", message, "OK", back, window, cx);
            }
        }
    }

    /// Starts source mode (the parse runs on a thread), or says why not.
    pub(super) fn try_source(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.source.is_some() {
            return Ok(());
        }
        self.start_source(window, cx)
    }

    /// A parse of the file as it is now, replacing any source mode.
    pub(super) fn start_source(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let len = self.doc.len();
        let first_line = self.doc.bytes(0, len.min(256) as usize);
        let doc = &mut self.doc;
        let (language, text) = grammar_for(&name, len, &first_line, || doc.bytes(0, len as usize))?;
        register_queries();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let rope = gpui_kit::base::input::Rope::from(text.as_str());
            let mut highlighter = SyntaxHighlighter::new(language);
            highlighter.update(None, &rope, None);
            let _ = tx.send(highlighter);
        });
        self.source = Some(SourceMode {
            language,
            highlighter: None,
            parsing: Some(rx),
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                let going = this
                    .update(cx, |this, cx| this.poll_source(cx))
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    /// Takes in a finished parse. Returns whether one is still running.
    fn poll_source(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mode) = self.source.as_mut() else {
            return false;
        };
        let Some(rx) = &mode.parsing else {
            return false;
        };
        match rx.try_recv() {
            Ok(highlighter) => {
                mode.highlighter = Some(highlighter);
                mode.parsing = None;
                cx.notify();
                false
            }
            Err(TryRecvError::Empty) => true,
            // A newer parse replaced this one's mode.
            Err(TryRecvError::Disconnected) => {
                mode.parsing = None;
                false
            }
        }
    }

    /// Colors the rows about to be drawn (text rows from `top`), and
    /// returns the style of each class they now carry.
    pub(super) fn color_rows(&self, rows: &mut [Row], cx: &Context<Self>) -> Vec<HighlightStyle> {
        let Some(highlighter) = self
            .source
            .as_ref()
            .filter(|_| !self.hex)
            .and_then(|mode| mode.highlighter.as_ref())
        else {
            return Vec::new();
        };
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return Vec::new();
        };
        let range = first.start as usize..last.end as usize;
        let palette = Palette(Theme::get(cx).colors.clone());
        let (styles, spans) = intern(highlighter.styles(&range, &palette));
        for row in rows {
            row.set_classes(&spans);
        }
        styles
    }

    /// The status line's word for the mode: the language's name.
    pub(super) fn source_name(&self) -> Option<&'static str> {
        self.source
            .as_ref()
            .filter(|_| !self.hex)
            .map(|mode| display_name(mode.language))
    }
}

/// The distinct styles of `styled` (uncolored parts left out), and its
/// ranges with each one's index among them.
fn intern(
    styled: Vec<(Range<usize>, HighlightStyle)>,
) -> (Vec<HighlightStyle>, Vec<(Range<u64>, u16)>) {
    let mut styles: Vec<HighlightStyle> = Vec::new();
    let mut spans = Vec::new();
    for (range, style) in styled {
        if style == HighlightStyle::default() || range.is_empty() {
            continue;
        }
        let class = match styles.iter().position(|s| *s == style) {
            Some(ix) => ix,
            None => {
                styles.push(style);
                styles.len() - 1
            }
        };
        spans.push((range.start as u64..range.end as u64, class as u16));
    }
    (styles, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grammar the app is built with is registered and colors a
    /// sample (a broken highlight query colors nothing, silently).
    #[test]
    fn every_language_colors_its_sample() {
        register_queries();
        let registered = LanguageRegistry::singleton().languages();
        let samples = [
            ("bash", "echo \"hi\" # note\n"),
            ("c", "int main(void) { return 0; }\n"),
            ("cpp", "class A { public: int x = 1; };\n"),
            ("csharp", "class A { public int X = 1; }\n"),
            ("css", "a { color: red; }\n"),
            ("diff", "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n"),
            ("ejs", "<% if (x) { %>hi<% } %>\n"),
            ("erb", "<% if x %>hi<% end %>\n"),
            ("go", "package main\nfunc main() {}\n"),
            ("html", "<p class=\"a\">hi</p>\n"),
            ("java", "class A { int x = 1; }\n"),
            ("javascript", "const x = 1; // note\n"),
            ("json", "{\"a\": 1}\n"),
            ("kotlin", "fun main() { val x = 1 }\n"),
            ("lua", "local x = 1 -- note\n"),
            ("make", "all:\n\techo hi\n"),
            ("markdown", "# Title\n\nText\n"),
            (
                "proto",
                "syntax = \"proto3\";\nmessage A { int32 x = 1; }\n",
            ),
            ("python", "def f():\n    return 1\n"),
            ("ruby", "def f\n  1\nend\n"),
            ("rust", "fn main() {}\n"),
            ("sql", "SELECT a FROM t;\n"),
            ("swift", "func f() -> Int { return 1 }\n"),
            ("toml", "[a]\nb = 1\n"),
            ("tsx", "const x: number = <div />;\n"),
            ("typescript", "const x: number = 1;\n"),
            ("yaml", "a: 1\n"),
        ];
        assert_eq!(samples.len(), yagni_commander_core::viewer::LANGUAGES.len());
        let palette = Palette(Theme::default().colors);
        let mut colorless = Vec::new();
        for ((name, _), (sample_name, sample)) in
            yagni_commander_core::viewer::LANGUAGES.iter().zip(samples)
        {
            assert_eq!(*name, sample_name);
            assert!(
                registered.iter().any(|r| r == name),
                "{name} isn't registered"
            );
            let rope = gpui_kit::base::input::Rope::from(sample);
            let mut highlighter = SyntaxHighlighter::new(name);
            highlighter.update(None, &rope, None);
            let (styles, _) = intern(highlighter.styles(&(0..sample.len()), &palette));
            if styles.is_empty() {
                colorless.push(*name);
            }
        }
        assert!(colorless.is_empty(), "these color nothing: {colorless:?}");
    }

    /// The style `colors` gives the first `word` in `code`.
    fn style_of(
        colors: &[(Range<usize>, HighlightStyle)],
        code: &str,
        word: &str,
    ) -> Option<gpui_kit::Hsla> {
        let at = code.find(word).unwrap();
        colors
            .iter()
            .find(|(range, _)| range.start <= at && at < range.end)
            .and_then(|(_, style)| style.color)
    }

    #[test]
    fn code_blocks_take_the_themes_syntax_colors() {
        let theme = Theme::builtin("gruvbox-dark").unwrap();
        let palette = Palette(theme.colors.clone());
        let keyword = Some(theme.colors.syntax_keyword.into());
        let first = "fn main() {}\n";
        let block = CodeBlock::from_code(first, Some("rust"));
        assert_eq!(
            style_of(&color_code(&block, &palette), first, "fn"),
            keyword
        );

        // The language's highlighter is reused: a second block is parsed on
        // its own, not as an edit of the first.
        let second = "// note\nstruct A;\nfn b() {}\n";
        let colors = color_code(&CodeBlock::from_code(second, Some("rust")), &palette);
        assert_eq!(style_of(&colors, second, "fn"), keyword);
        assert_eq!(
            style_of(&colors, second, "note"),
            Some(theme.colors.syntax_comment.into())
        );
        assert!(colors.iter().all(|(range, _)| range.end <= second.len()));

        // A language without a grammar: plain, and no highlighter is made
        // (making one for an unknown name logs a warning).
        assert!(highlighter_for("mermaid").is_none());
        let diagram = "graph TD; A-->B\n";
        let block = CodeBlock::from_code(diagram, Some("mermaid"));
        assert!(color_code(&block, &palette).is_empty());

        // An alias as fences write it.
        let shell = "echo hi # note\n";
        let colors = color_code(&CodeBlock::from_code(shell, Some("sh")), &palette);
        assert_eq!(
            style_of(&colors, shell, "note"),
            Some(theme.colors.syntax_comment.into())
        );
    }

    #[test]
    fn code_blocks_without_a_known_language_stay_plain() {
        let palette = Palette(Theme::default().colors);
        let code = "fn main() {}\n";
        let none: Option<&str> = None;
        assert!(color_code(&CodeBlock::from_code(code, none), &palette).is_empty());
        let colors = color_code(&CodeBlock::from_code(code, Some("no-such")), &palette);
        assert!(colors.iter().all(|(_, style)| style.color.is_none()));
    }

    /// One highlighter per theme: gpui-base keeps a block's colors only
    /// while it gets the same `Arc`.
    #[gpui_kit::test]
    fn code_colors_stay_the_same_until_the_theme_changes(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::builtin("tokyo-night").unwrap());
            let first = code_colors(cx);
            assert!(Arc::ptr_eq(&first, &code_colors(cx)));

            let theme = Theme::builtin("gruvbox-dark").unwrap();
            let keyword = Some(theme.colors.syntax_keyword.into());
            cx.set_global(theme);
            let second = code_colors(cx);
            assert!(!Arc::ptr_eq(&first, &second));
            let code = "fn main() {}\n";
            let colors = second(&CodeBlock::from_code(code, Some("rust")));
            assert_eq!(style_of(&colors, code, "fn"), keyword);
        });
    }

    #[test]
    fn a_file_needs_a_grammar_the_size_and_utf8() {
        let read = || b"fn main() {}".to_vec();
        assert_eq!(
            grammar_for("main.rs", 12, b"", read),
            Ok(("rust", "fn main() {}".to_owned()))
        );
        let never = || -> Vec<u8> { panic!("read") };
        assert!(
            grammar_for("notes.txt", 5, b"", never)
                .unwrap_err()
                .contains("no grammar")
        );
        assert!(
            grammar_for("big.rs", SOURCE_CAP + 1, b"", never)
                .unwrap_err()
                .contains("up to 4.0 MiB")
        );
        assert!(
            grammar_for("bad.rs", 2, b"", || vec![0xff, 0xfe])
                .unwrap_err()
                .contains("UTF-8")
        );
    }

    #[test]
    fn styles_are_interned_and_plain_parts_dropped() {
        let red = HighlightStyle {
            color: Some(gpui_kit::Hsla {
                h: 0.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
            ..Default::default()
        };
        let blue = HighlightStyle {
            color: Some(gpui_kit::Hsla {
                h: 0.6,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
            ..Default::default()
        };
        let (styles, spans) = intern(vec![
            (0..2, red),
            (2..3, HighlightStyle::default()),
            (3..5, blue),
            (5..5, blue),
            (6..9, red),
        ]);
        assert_eq!(styles, [red, blue]);
        assert_eq!(spans, [(0..2, 0), (3..5, 1), (6..9, 0)]);
    }
}
