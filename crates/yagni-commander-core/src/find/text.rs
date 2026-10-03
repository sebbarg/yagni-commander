//! Alt-F7's "Containing text": one engine (`regex::bytes`) for plain text
//! and regular expressions. Files are read in blocks cut at line ends, and a
//! match never spans lines, like grep.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::ops::Range;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use nix::fcntl::OFlag;
use regex::bytes::{Regex, RegexBuilder};

/// Bytes read per block.
pub(crate) const BLOCK: usize = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Text {
    pub pattern: String,
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_words: bool,
    /// Results are the files that don't contain it.
    pub not_containing: bool,
}

/// [`Text`] compiled, ready to search files with.
#[derive(Debug, Clone)]
pub struct TextQuery {
    regex: Regex,
    pub not_containing: bool,
}

impl Text {
    pub fn compile(&self) -> Result<TextQuery, regex::Error> {
        let pattern = if self.regex {
            self.pattern.clone()
        } else {
            regex::escape(&self.pattern)
        };
        let pattern = if self.whole_words {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()?;
        Ok(TextQuery {
            regex,
            not_containing: self.not_containing,
        })
    }
}

impl TextQuery {
    /// Whether `path` contains a match. `Interrupted` once `cancel` is set
    /// (checked per block).
    pub fn is_match_in(&self, path: &Path, cancel: &AtomicBool) -> io::Result<bool> {
        let mut file = open(path)?;
        let mut buf: Vec<u8> = Vec::with_capacity(2 * BLOCK);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let start = buf.len();
            buf.resize(start + BLOCK, 0);
            let mut filled = start;
            while filled < buf.len() {
                match file.read(&mut buf[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            let eof = filled < buf.len();
            buf.truncate(filled);
            // Up to the last line end; the rest starts the next block. A
            // block without one is searched as a piece.
            let cut = match buf.iter().rposition(|&b| b == b'\n') {
                Some(i) if !eof => i + 1,
                _ => buf.len(),
            };
            if self.line_match(&buf[..cut]) {
                return Ok(true);
            }
            buf.drain(..cut);
            if eof {
                return Ok(false);
            }
        }
    }

    /// A match in `hay` that stays on one line.
    fn line_match(&self, hay: &[u8]) -> bool {
        self.find_at(hay, 0).is_some()
    }

    /// The leftmost match starting at or after `at` that stays on one line.
    /// The bytes before `at` are context for `^` and `\b`. Shared by Alt-F7
    /// and the viewer, so both agree on what a match is.
    pub fn find_at(&self, hay: &[u8], at: usize) -> Option<Range<usize>> {
        let mut at = at;
        while at <= hay.len() {
            let m = self.regex.find_at(hay, at)?;
            let Some(i) = m.as_bytes().iter().position(|&b| b == b'\n') else {
                return Some(m.range());
            };
            // The leftmost match ran past its line; a shorter one may still
            // fit on it. Cut at the line end, which behaves like the `\n`
            // for `$` and `\b`.
            let line_end = m.start() + i;
            if let Some(m) = self.regex.find_at(&hay[..line_end], m.start()) {
                return Some(m.range());
            }
            at = line_end + 1;
        }
        None
    }
}

/// Opens a regular file. `O_NONBLOCK` keeps a FIFO swapped in after the
/// walk's check from blocking; the opened file is checked again.
fn open(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOCTTY).bits())
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(pattern: &str) -> Text {
        Text {
            pattern: pattern.into(),
            ..Text::default()
        }
    }

    fn found(t: &Text, content: &[u8]) -> bool {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, content).unwrap();
        t.compile()
            .unwrap()
            .is_match_in(&path, &AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn plain_text_is_literal_and_case_insensitive_by_default() {
        assert!(found(&text("a.b"), b"xx A.B yy"));
        assert!(!found(&text("a.b"), b"axb"));
        let cs = Text {
            case_sensitive: true,
            ..text("Hello")
        };
        assert!(!found(&cs, b"hello"));
        assert!(found(&cs, b"Hello"));
    }

    #[test]
    fn regex_and_whole_words() {
        let re = Text {
            regex: true,
            ..text(r"fn\s+main")
        };
        assert!(found(&re, b"pub fn  main() {}"));
        let ww = Text {
            whole_words: true,
            ..text("cat")
        };
        assert!(found(&ww, b"a cat."));
        assert!(!found(&ww, b"concatenate"));
        let both = Text {
            regex: true,
            whole_words: true,
            ..text("ca|do")
        };
        assert!(!found(&both, b"cat dog"));
        assert!(found(&both, b"ca do"));
    }

    #[test]
    fn an_invalid_regex_is_an_error() {
        let bad = Text {
            regex: true,
            ..text("(")
        };
        assert!(bad.compile().is_err());
        assert!(text("(").compile().is_ok(), "plain text is literal");
    }

    #[test]
    fn binary_files_are_searched() {
        assert!(found(&text("needle"), b"\0\xff\xfeneedle\0"));
    }

    #[test]
    fn a_line_crossing_a_block_boundary_is_found() {
        let mut content = vec![b'x'; BLOCK - 3];
        content.extend_from_slice(b"\nabc needle def\n");
        assert!(found(&text("needle"), &content));
        let mut content = vec![b'\n'; BLOCK - 3];
        content.extend_from_slice(b"needle\n");
        assert!(found(&text("needle"), &content));
    }

    #[test]
    fn matches_never_span_lines() {
        let re = Text {
            regex: true,
            ..text(r"a\sb")
        };
        assert!(!found(&re, b"a\nb"));
        assert!(found(&re, b"a\nb\na b"), "a later match on one line counts");
        let anchored = Text {
            regex: true,
            ..text("^b$")
        };
        assert!(found(&anchored, b"a\nb\nc"), "anchors work per line");
    }

    #[test]
    fn a_match_running_into_a_line_end_still_finds_the_line() {
        for (pattern, content) in [
            (r"foo\s*", "foo\nbar\n"),
            ("TODO[^;]*", "x TODO\ny;\n"),
            (r"foo\s*$", "foo  \n"),
        ] {
            let re = Text {
                regex: true,
                ..text(pattern)
            };
            assert!(found(&re, content.as_bytes()), "{pattern}");
        }
    }

    #[test]
    fn a_line_longer_than_a_block_is_searched_in_pieces() {
        let mut content = vec![b'x'; 3 * BLOCK];
        content.extend_from_slice(b"needle");
        assert!(found(&text("needle"), &content));
        assert!(!found(&text("absent"), &content));
    }

    fn at(t: &Text, hay: &str, at: usize) -> Option<Range<usize>> {
        t.compile().unwrap().find_at(hay.as_bytes(), at)
    }

    fn re(pattern: &str) -> Text {
        Text {
            regex: true,
            ..text(pattern)
        }
    }

    #[test]
    fn find_at_gives_the_leftmost_match_from_a_position() {
        assert_eq!(at(&text("ab"), "ab ab", 0), Some(0..2));
        assert_eq!(at(&text("ab"), "ab ab", 1), Some(3..5));
        assert_eq!(at(&text("aa"), "aaa", 1), Some(1..3), "overlapping");
        assert_eq!(at(&text("ab"), "ab ab", 4), None);
        assert_eq!(at(&text("ab"), "ab", 3), None, "past the end");
    }

    #[test]
    fn find_at_sees_the_bytes_before_it() {
        let words = Text {
            whole_words: true,
            ..text("foo")
        };
        assert_eq!(at(&words, "xfoo foo", 1), Some(5..8));
        assert_eq!(at(&re("^b"), "ab\nb", 1), Some(3..4));
    }

    #[test]
    fn find_at_keeps_matches_on_one_line() {
        assert_eq!(
            at(&re(r"a\s*"), "a\nb", 0),
            Some(0..1),
            "shorter on the line"
        );
        assert_eq!(at(&re(r"a\sb"), "a\nb a b", 0), Some(4..7), "next line's");
        assert_eq!(at(&re(r"a\sb"), "a\nb", 0), None);
    }

    #[test]
    fn find_at_can_return_an_empty_match() {
        assert_eq!(at(&re("^"), "ab\ncd", 1), Some(3..3));
    }

    #[test]
    fn an_empty_file_has_no_match() {
        assert!(!found(&text("x"), b""));
    }

    #[test]
    fn cancel_stops_the_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, b"needle").unwrap();
        let q = text("needle").compile().unwrap();
        let e = q.is_match_in(&path, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pipe");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let q = text("x").compile().unwrap();
        assert!(q.is_match_in(&path, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let q = text("x").compile().unwrap();
        assert!(
            q.is_match_in(&tmp.path().join("gone"), &AtomicBool::new(false))
                .is_err()
        );
    }
}
