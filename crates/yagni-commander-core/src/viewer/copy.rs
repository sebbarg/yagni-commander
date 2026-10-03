//! Ctrl-C in the viewer: the selected bytes as text, or as hex codes.

use std::fmt::Write;
use std::ops::Range;

use super::document::Document;
use super::source::Source;

/// The most one copy takes: 64 MiB of selected bytes for text, of codes
/// for hex.
pub const COPY_CAP: u64 = 64 << 20;
/// Bytes read at a time.
const PIECE: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copy {
    /// The bytes as UTF-8 (invalid bytes become U+FFFD).
    Text,
    /// The bytes as hex codes, `4F 6B`, a line per 16-byte row.
    Hex,
}

/// The selection would copy this many bytes, over [`COPY_CAP`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLarge(pub u64);

/// What copying `n` bytes `how` would exceed the cap with.
fn too_large(n: u64, how: Copy) -> Option<TooLarge> {
    let size = match how {
        Copy::Text => n,
        Copy::Hex => n.saturating_mul(3),
    };
    (size > COPY_CAP).then_some(TooLarge(size))
}

impl<S: Source> Document<S> {
    /// The bytes of `range` as `how` says. Past the end (the file shrank):
    /// what is there.
    pub fn copy_text(&mut self, range: Range<u64>, how: Copy) -> Result<String, TooLarge> {
        let n = range.end.saturating_sub(range.start);
        if let Some(e) = too_large(n, how) {
            return Err(e);
        }
        let mut bytes = Vec::with_capacity(n.min(self.len()) as usize);
        let mut pos = range.start;
        while pos < range.end {
            let piece = self.bytes(pos, (range.end - pos).min(PIECE) as usize);
            if piece.is_empty() {
                break;
            }
            pos += piece.len() as u64;
            bytes.extend_from_slice(&piece);
        }
        Ok(match how {
            Copy::Text => String::from_utf8_lossy(&bytes).into_owned(),
            Copy::Hex => {
                let mut out = String::with_capacity(bytes.len() * 3);
                for (i, b) in bytes.iter().enumerate() {
                    if i > 0 {
                        let at = range.start + i as u64;
                        out.push(if at.is_multiple_of(16) { '\n' } else { ' ' });
                    }
                    let _ = write!(out, "{b:02X}");
                }
                out
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bytes: &[u8]) -> Document<Vec<u8>> {
        Document::new(bytes.to_vec())
    }

    #[test]
    fn text_keeps_tabs_and_crlf() {
        let mut d = doc(b"a\tb\r\nc");
        assert_eq!(d.copy_text(0..6, Copy::Text).unwrap(), "a\tb\r\nc");
        assert_eq!(d.copy_text(2..4, Copy::Text).unwrap(), "b\r");
    }

    #[test]
    fn invalid_utf8_becomes_the_replacement_character() {
        let mut d = doc(b"a\xffb");
        assert_eq!(d.copy_text(0..3, Copy::Text).unwrap(), "a\u{fffd}b");
    }

    #[test]
    fn hex_breaks_lines_at_multiples_of_16() {
        let bytes: Vec<u8> = (0u8..40).collect();
        let mut d = doc(&bytes);
        assert_eq!(
            d.copy_text(14..34, Copy::Hex).unwrap(),
            "0E 0F\n10 11 12 13 14 15 16 17 18 19 1A 1B 1C 1D 1E 1F\n20 21"
        );
        assert_eq!(d.copy_text(0..1, Copy::Hex).unwrap(), "00");
        assert_eq!(d.copy_text(5..5, Copy::Hex).unwrap(), "");
    }

    #[test]
    fn the_cap_refuses_before_reading() {
        let mut d = doc(&[b'a'; 100]);
        assert_eq!(d.copy_text(0..100, Copy::Text).unwrap().len(), 100);
        assert_eq!(
            d.copy_text(0..COPY_CAP + 1, Copy::Text),
            Err(TooLarge(COPY_CAP + 1)),
            "refused without reading"
        );
        assert_eq!(too_large(COPY_CAP, Copy::Text), None);
        assert_eq!(
            too_large(COPY_CAP / 3 + 1, Copy::Hex),
            Some(TooLarge((COPY_CAP / 3 + 1) * 3))
        );
    }

    #[test]
    fn copy_text_after_the_file_shrank() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, b"hello world").unwrap();
        let mut d = Document::new(crate::viewer::FileSource::open(&path).unwrap());
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(d.copy_text(0..11, Copy::Text).unwrap(), "hello");
    }
}
