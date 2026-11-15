//! Source mode (`S`, and at open for a file core `viewer::language` knows):
//! the text view with syntax colors. The rows, search, selection and wrap
//! are the text view's own; a parse of the whole file (gpui-component's
//! tree-sitter highlighter) adds colors to the rows on screen.

use std::ops::Range;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use gpui_kit::component::highlighter::{LanguageRegistry, SyntaxHighlighter};
use gpui_kit::{Context, HighlightStyle, Window};
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
    /// `S`: source mode, out of any other mode. A file it can't show stays
    /// in its mode, with a box saying why.
    pub(super) fn show_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.try_source(window, cx) {
            Ok(()) => {
                self.leave_picture(window, cx);
                self.markdown = None;
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
