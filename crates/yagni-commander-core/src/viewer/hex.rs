//! Hex mode (`H`): 16 bytes per row at multiples of 16, as the offset, the
//! hex codes and the bytes as ASCII characters:
//!
//! `00000000  48 65 6C 6C 6F 20 77 6F  72 6C 64 0A 00 01 FF 41  Hello world....A`

use std::ops::Range;

use super::layout::{Highlight, Row};

/// Bytes per hex row.
pub const HEX_ROW: u64 = 16;

/// Shown for bytes that aren't printable ASCII.
const PLACEHOLDER: char = '.';

/// Digits of the offset column: 8, more if the largest offset needs them,
/// so every row of the file lines up.
pub fn offset_digits(len: u64) -> usize {
    let max = len.saturating_sub(1);
    let digits = (u64::BITS - max.leading_zeros()).div_ceil(4) as usize;
    digits.max(8)
}

/// Where byte `i` of a row starts in its text, and where the characters
/// start.
pub(crate) fn code_at(digits: usize, i: usize) -> usize {
    digits + 2 + i * 3 + usize::from(i >= 8)
}

pub(crate) fn chars_at(digits: usize) -> usize {
    code_at(digits, 15) + 2 + 2
}

/// The row of up to 16 `bytes` at `start` (a multiple of 16, or the start
/// of a short last row). `hl`: file bytes whose codes and characters get
/// [`Row::marks`] and [`Row::selection`].
pub(crate) fn hex_row(bytes: &[u8], start: u64, digits: usize, hl: Highlight) -> Row {
    let bytes = &bytes[..bytes.len().min(HEX_ROW as usize)];
    let mut text = format!("{start:0digits$X}  ");
    for i in 0..HEX_ROW as usize {
        if i == 8 {
            text.push(' ');
        }
        match bytes.get(i) {
            Some(b) => text.push_str(&format!("{b:02X}")),
            None => text.push_str("  "),
        }
        if i + 1 < HEX_ROW as usize {
            text.push(' ');
        }
    }
    text.push_str("  ");
    let mut dim = Vec::new();
    for &b in bytes {
        if (0x20..=0x7e).contains(&b) {
            text.push(b as char);
        } else {
            dim.push(text.len()..text.len() + 1);
            text.push(PLACEHOLDER);
        }
    }
    let end = start + bytes.len() as u64;
    let marks = ranges(hl.mark, start, end, digits);
    let selection = ranges(hl.selection, start, end, digits);
    Row {
        start,
        end,
        width: text.len() as u32,
        text,
        dim,
        marks,
        selection,
        sources: Vec::new(),
    }
}

/// The codes and characters showing the bytes of `range` in the row
/// `start..end`.
fn ranges(range: Option<&Range<u64>>, start: u64, end: u64, digits: usize) -> Vec<Range<usize>> {
    let Some(m) = range else {
        return Vec::new();
    };
    let (from, to) = (m.start.max(start), m.end.min(end));
    if from >= to {
        return Vec::new();
    }
    let (first, last) = ((from - start) as usize, (to - start) as usize - 1);
    let chars = chars_at(digits);
    vec![
        code_at(digits, first)..code_at(digits, last) + 2,
        chars + first..chars + last + 1,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_row() {
        let row = hex_row(b"Hello world\n\x00\x01\xffA", 0, 8, Highlight::default());
        assert_eq!(
            row.text,
            "00000000  48 65 6C 6C 6F 20 77 6F  72 6C 64 0A 00 01 FF 41  Hello world....A"
        );
        assert_eq!((row.start, row.end), (0, 16));
        assert_eq!(row.width as usize, row.text.len());
        let dim: Vec<&str> = row.dim.iter().map(|r| &row.text[r.clone()]).collect();
        assert_eq!(dim, [".", ".", ".", "."]);
    }

    #[test]
    fn a_short_last_row_keeps_the_characters_aligned() {
        let full = hex_row(&[b'a'; 16], 0, 8, Highlight::default());
        let short = hex_row(b"ab.", 32, 8, Highlight::default());
        assert_eq!(
            short.text,
            format!("00000020  61 62 2E{}  ab.", " ".repeat(3 * 13 + 1))
        );
        assert_eq!(short.text.find("ab."), full.text.find("aaa"));
        assert_eq!(short.end, 35);
        assert!(short.dim.is_empty(), "a real '.' is not dim");
    }

    #[test]
    fn the_offset_grows_past_4_gib() {
        assert_eq!(offset_digits(0), 8);
        assert_eq!(offset_digits(1 << 32), 8);
        assert_eq!(offset_digits((1 << 32) + 1), 9);
        assert_eq!(offset_digits(u64::MAX), 16);
        let row = hex_row(b"x", 1 << 32, 9, Highlight::default());
        assert!(row.text.starts_with("100000000  78 "), "{}", row.text);
    }

    fn marked(row: &Row) -> Vec<&str> {
        row.marks.iter().map(|m| &row.text[m.clone()]).collect()
    }

    #[test]
    fn a_mark_covers_codes_and_characters() {
        let bytes = b"0123456789abcdef";
        let row = hex_row(bytes, 16, 8, Highlight::mark(&(22..26)));
        assert_eq!(marked(&row), ["36 37  38 39", "6789"]);
        // Starting before the row and ending after it: the whole row.
        let row = hex_row(bytes, 16, 8, Highlight::mark(&(10..40)));
        assert_eq!(marked(&row)[1], "0123456789abcdef");
        assert!(
            hex_row(bytes, 16, 8, Highlight::mark(&(32..40)))
                .marks
                .is_empty()
        );
        assert!(
            hex_row(bytes, 16, 8, Highlight::mark(&(20..20)))
                .marks
                .is_empty()
        );
    }

    use crate::viewer::layout::HexColumn;

    #[test]
    fn hit_testing_hex_codes_and_characters() {
        // 00000010  30 31 32 33 34 35 36 37  38 39 61 62 63 64 65 66  0123456789abcdef
        let row = hex_row(b"0123456789abcdef", 16, 8, Highlight::default());
        let codes = |i: usize| code_at(8, i) as f32;
        let chars = chars_at(8) as f32;
        assert_eq!(row.hex_column(codes(3)), HexColumn::Codes);
        assert_eq!(row.hex_column(2.0), HexColumn::Codes, "the offset column");
        assert_eq!(row.hex_column(chars - 1.0), HexColumn::Chars);
        assert_eq!(row.hex_column(chars + 5.0), HexColumn::Chars);
        // Codes: left half of "33" is before byte 3, right half after it.
        assert_eq!(row.boundary_at(codes(3) + 0.5, HexColumn::Codes), 19);
        assert_eq!(row.boundary_at(codes(3) + 1.5, HexColumn::Codes), 20);
        // The gap after the 8th code goes to the nearer byte.
        assert_eq!(row.boundary_at(codes(8) - 1.0, HexColumn::Codes), 24);
        assert_eq!(row.boundary_at(2.0, HexColumn::Codes), 16, "offset column");
        assert_eq!(row.char_at(codes(15) + 1.0, HexColumn::Codes), 31);
        // The gaps between codes, and past the last one, stay in the row.
        assert_eq!(row.char_at(codes(3) + 2.5, HexColumn::Codes), 19);
        assert_eq!(row.char_at(codes(8) - 1.0, HexColumn::Codes), 23);
        assert_eq!(row.char_at(codes(15) + 2.5, HexColumn::Codes), 31);
        // Characters.
        assert_eq!(row.boundary_at(chars + 4.6, HexColumn::Chars), 21);
        assert_eq!(row.char_at(chars + 4.6, HexColumn::Chars), 20);
        assert_eq!(row.boundary_at(chars + 40.0, HexColumn::Chars), 32);
        assert_eq!(row.content_end(), 32);
    }

    #[test]
    fn a_short_hex_row_ends_at_its_last_byte() {
        let row = hex_row(b"ab", 32, 8, Highlight::default());
        assert_eq!(
            row.boundary_at(chars_at(8) as f32 + 9.0, HexColumn::Chars),
            34
        );
        assert_eq!(row.char_at(code_at(8, 10) as f32, HexColumn::Codes), 33);
    }

    #[test]
    fn the_selection_shows_in_both_columns() {
        let row = hex_row(b"0123456789abcdef", 16, 8, Highlight::selection(&(22..26)));
        let parts: Vec<&str> = row.selection.iter().map(|s| &row.text[s.clone()]).collect();
        assert_eq!(parts, ["36 37  38 39", "6789"]);
        assert!(row.marks.is_empty());
    }

    #[test]
    fn visible_clips_both_marks() {
        let row = hex_row(b"0123456789abcdef", 0, 8, Highlight::mark(&(0..2)));
        let shown = row.visible(10, 4);
        assert_eq!(shown.text, "30 3");
        assert_eq!(shown.marks, std::iter::once(0..4).collect::<Vec<_>>());
        let shown = row.visible(60, 20);
        assert_eq!(shown.marks, std::iter::once(0..2).collect::<Vec<_>>());
    }
}
