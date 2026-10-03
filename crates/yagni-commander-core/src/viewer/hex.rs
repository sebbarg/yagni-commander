//! Hex mode (`H`): 16 bytes per row at multiples of 16, as the offset, the
//! hex codes and the bytes as ASCII characters:
//!
//! `00000000  48 65 6C 6C 6F 20 77 6F  72 6C 64 0A 00 01 FF 41  Hello world....A`

use std::ops::Range;

use super::layout::Row;

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
fn code_at(digits: usize, i: usize) -> usize {
    digits + 2 + i * 3 + usize::from(i >= 8)
}

fn chars_at(digits: usize) -> usize {
    code_at(digits, 15) + 2 + 2
}

/// The row of up to 16 `bytes` at `start` (a multiple of 16, or the start
/// of a short last row). `mark`: file bytes whose codes and characters get
/// [`Row::marks`].
pub(crate) fn hex_row(bytes: &[u8], start: u64, digits: usize, mark: Option<&Range<u64>>) -> Row {
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
    let mut marks = Vec::new();
    if let Some(m) = mark {
        let (from, to) = (m.start.max(start), m.end.min(end));
        if from < to {
            let (first, last) = ((from - start) as usize, (to - start) as usize - 1);
            marks.push(code_at(digits, first)..code_at(digits, last) + 2);
            let chars = chars_at(digits);
            marks.push(chars + first..chars + last + 1);
        }
    }
    Row {
        start,
        end,
        width: text.len() as u32,
        text,
        dim,
        marks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_row() {
        let row = hex_row(b"Hello world\n\x00\x01\xffA", 0, 8, None);
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
        let full = hex_row(&[b'a'; 16], 0, 8, None);
        let short = hex_row(b"ab.", 32, 8, None);
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
        let row = hex_row(b"x", 1 << 32, 9, None);
        assert!(row.text.starts_with("100000000  78 "), "{}", row.text);
    }

    fn marked(row: &Row) -> Vec<&str> {
        row.marks.iter().map(|m| &row.text[m.clone()]).collect()
    }

    #[test]
    fn a_mark_covers_codes_and_characters() {
        let bytes = b"0123456789abcdef";
        let row = hex_row(bytes, 16, 8, Some(&(22..26)));
        assert_eq!(marked(&row), ["36 37  38 39", "6789"]);
        // Starting before the row and ending after it: the whole row.
        let row = hex_row(bytes, 16, 8, Some(&(10..40)));
        assert_eq!(marked(&row)[1], "0123456789abcdef");
        assert!(hex_row(bytes, 16, 8, Some(&(32..40))).marks.is_empty());
        assert!(hex_row(bytes, 16, 8, Some(&(20..20))).marks.is_empty());
    }

    #[test]
    fn visible_clips_both_marks() {
        let row = hex_row(b"0123456789abcdef", 0, 8, Some(&(0..2)));
        let shown = row.visible(10, 4);
        assert_eq!(shown.text, "30 3");
        assert_eq!(shown.marks, std::iter::once(0..4).collect::<Vec<_>>());
        let shown = row.visible(60, 20);
        assert_eq!(shown.marks, std::iter::once(0..2).collect::<Vec<_>>());
    }
}
