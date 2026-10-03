//! Splits a window of file bytes into display rows: tabs expanded, control
//! characters and invalid UTF-8 replaced, wrapped to a column count.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

/// Block size of the read cache and spacing of forced row breaks in giant lines.
pub const BLOCK: u64 = 64 * 1024;
/// Longest row in characters, in both modes (no-wrap rows are cut here, and it
/// bounds rows of zero-width characters).
pub const MAX_ROW_CHARS: usize = 10_000;
/// Bytes one row can need: four per character, plus the `\r\n` lookahead.
pub const ROW_WINDOW: usize = MAX_ROW_CHARS * 4 + 4;
const TAB: u32 = 8;
/// Shown for control characters and invalid UTF-8 bytes.
const PLACEHOLDER: char = '·';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// Word wrap at this many columns.
    Columns(u32),
    /// One row per line (cut at [`MAX_ROW_CHARS`]).
    Off,
}

/// One display row: the bytes `start..end` of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: u64,
    /// Exclusive; includes the line ending if the row has one.
    pub end: u64,
    /// Display text: tabs expanded, placeholders substituted, no line ending.
    pub text: String,
    /// Byte ranges of `text` that hold placeholders (drawn dim).
    pub dim: Vec<Range<usize>>,
    /// Width of `text` in columns.
    pub width: u32,
    /// The range of `text` that shows the marked bytes (the current match).
    pub mark: Option<Range<usize>>,
}

impl Row {
    /// The part of the row from column `from_col`, at most `cols` wide, with
    /// its dim ranges and mark. A wide character cut by either edge becomes a
    /// space.
    pub fn visible(&self, from_col: u32, cols: u32) -> Visible {
        let to_col = from_col.saturating_add(cols);
        let mut text = String::new();
        let mut dim = Vec::new();
        let mut mark: Option<Range<usize>> = None;
        let mut col = 0;
        // `self.dim` is sorted, so one pass over it keeps this linear.
        let mut dims = self.dim.iter().map(|r| r.start).peekable();
        for (ix, ch) in self.text.char_indices() {
            while dims.next_if(|&start| start < ix).is_some() {}
            let is_dim = dims.next_if_eq(&ix).is_some();
            let w = ch.width().unwrap_or(1) as u32;
            let (start, end) = (col, col + w);
            col = end;
            if end <= from_col {
                continue;
            }
            if start >= to_col {
                break;
            }
            let at = text.len();
            if start < from_col || end > to_col {
                text.push(' ');
            } else {
                text.push(ch);
                if is_dim {
                    dim.push(at..text.len());
                }
            }
            if self.mark.as_ref().is_some_and(|m| m.contains(&ix)) {
                mark.get_or_insert(at..at).end = text.len();
            }
        }
        Visible { text, dim, mark }
    }
}

impl Row {
    /// The columns [`Self::mark`] takes.
    pub fn mark_columns(&self) -> Option<Range<u32>> {
        let mark = self.mark.as_ref()?;
        let width = |s: &str| {
            s.chars()
                .map(|c| c.width().unwrap_or(1) as u32)
                .sum::<u32>()
        };
        let start = width(&self.text[..mark.start]);
        Some(start..start + width(&self.text[mark.clone()]))
    }
}

/// What [`Row::visible`] shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    pub text: String,
    pub dim: Vec<Range<usize>>,
    pub mark: Option<Range<usize>>,
}

/// Decodes one unit at the start of `bytes`: a valid UTF-8 character and its
/// length, or `None` for one invalid byte.
pub(crate) fn decode(bytes: &[u8]) -> (Option<char>, usize) {
    let len = match bytes[0] {
        b @ 0x00..=0x7f => return (Some(b as char), 1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match bytes.get(..len).and_then(|s| std::str::from_utf8(s).ok()) {
        Some(s) => (s.chars().next(), len),
        None => (None, 1),
    }
}

/// Lays out the row starting at file offset `start`. `bytes` begins there;
/// the row never extends past `limit` bytes (a forced break or the end of
/// the file) and, wrapping, never past `cols` columns. `mark`: file bytes
/// whose characters get [`Row::mark`].
pub(crate) fn layout_row(
    bytes: &[u8],
    start: u64,
    limit: usize,
    wrap: Wrap,
    mark: Option<&Range<u64>>,
) -> Row {
    let limit = limit.min(bytes.len());
    let mut text = String::new();
    let mut dim = Vec::new();
    let mut marked: Option<Range<usize>> = None;
    let mut col = 0u32;
    let mut chars = 0;
    let mut i = 0;
    // Where to break for word wrap: after the last space or tab
    // (byte offset, text length, dim count, column).
    let mut soft: Option<(usize, usize, usize, u32)> = None;
    while i < limit && chars < MAX_ROW_CHARS {
        let (ch, len) = decode(&bytes[i..]);
        if ch == Some('\n') {
            i += 1;
            break;
        }
        if ch == Some('\r') && i + 1 < limit && bytes[i + 1] == b'\n' {
            i += 2;
            break;
        }
        let (shown, w, is_dim) = match ch {
            Some('\t') => (' ', TAB - col % TAB, false),
            Some(c) if c.is_control() => (PLACEHOLDER, 1, true),
            Some(c) => (c, c.width().unwrap_or(1) as u32, false),
            None => (PLACEHOLDER, 1, true),
        };
        if let Wrap::Columns(cols) = wrap
            && chars > 0
            && col + w > cols
        {
            if let Some((si, st, sd, sc)) = soft {
                i = si;
                text.truncate(st);
                dim.truncate(sd);
                col = sc;
            }
            break;
        }
        let at = text.len();
        if ch == Some('\t') {
            text.extend(std::iter::repeat_n(' ', w as usize));
        } else {
            text.push(shown);
        }
        if is_dim {
            dim.push(at..text.len());
        }
        let pos = start + i as u64;
        if mark.is_some_and(|m| pos < m.end && pos + len as u64 > m.start) {
            marked.get_or_insert(at..at).end = text.len();
        }
        col += w;
        chars += 1;
        i += len;
        if matches!(ch, Some(' ' | '\t')) {
            soft = Some((i, text.len(), dim.len(), col));
        }
    }
    // A word-wrap break may have cut the marked text off.
    let mark = marked
        .map(|m| m.start..m.end.min(text.len()))
        .filter(|m| m.start < m.end);
    Row {
        start,
        end: start + i as u64,
        text,
        dim,
        width: col,
        mark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(cols: u32) -> Wrap {
        Wrap::Columns(cols)
    }

    /// Lays out every row of `bytes` from 0, as the document will.
    fn all(bytes: &[u8], wrap: Wrap) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            let row = layout_row(&bytes[pos..], pos as u64, bytes.len() - pos, wrap, None);
            pos = row.end as usize;
            rows.push(row);
        }
        rows
    }

    fn texts(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn lines_end_at_newline_and_crlf_drops_the_cr() {
        let rows = all(b"ab\r\ncd\nef", Wrap::Off);
        assert_eq!(texts(&rows), ["ab", "cd", "ef"]);
        assert_eq!((rows[0].start, rows[0].end), (0, 4));
        assert_eq!((rows[2].start, rows[2].end), (7, 9));
    }

    #[test]
    fn empty_file_has_no_rows() {
        assert!(all(b"", wrap(10)).is_empty());
        assert_eq!(texts(&all(b"\n", wrap(10))), [""]);
    }

    #[test]
    fn tabs_expand_to_multiples_of_eight() {
        let rows = all(b"a\tb\t\tc", Wrap::Off);
        assert_eq!(
            rows[0].text,
            format!("a{}b{}c", " ".repeat(7), " ".repeat(15))
        );
        assert_eq!(rows[0].width, 25);
    }

    #[test]
    fn controls_and_invalid_bytes_are_dim_placeholders() {
        let rows = all(b"a\x01b\xffc\rd", Wrap::Off);
        assert_eq!(rows[0].text, "a·b·c·d");
        let dim: Vec<&str> = rows[0]
            .dim
            .iter()
            .map(|r| &rows[0].text[r.clone()])
            .collect();
        assert_eq!(dim, ["·", "·", "·"]);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let rows = all("日本語".as_bytes(), wrap(5));
        assert_eq!(texts(&rows), ["日本", "語"]);
        assert_eq!(rows[0].width, 4);
    }

    #[test]
    fn word_wrap_breaks_after_the_last_space() {
        let rows = all(b"hello big world", wrap(10));
        assert_eq!(texts(&rows), ["hello big ", "world"]);
    }

    #[test]
    fn word_wrap_breaks_mid_word_without_spaces() {
        let rows = all(b"abcdefghij", wrap(4));
        assert_eq!(texts(&rows), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_character_wider_than_the_row_still_fits_alone() {
        assert_eq!(texts(&all("日日".as_bytes(), wrap(1))), ["日", "日"]);
    }

    #[test]
    fn no_wrap_cuts_rows_at_max_chars() {
        let line = vec![b'x'; MAX_ROW_CHARS * 2 + 5];
        let rows = all(&line, Wrap::Off);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text.len(), MAX_ROW_CHARS);
        assert_eq!(rows[2].text.len(), 5);
    }

    #[test]
    fn zero_width_characters_are_capped_too() {
        // Combining marks are zero columns wide; the char cap still ends the row.
        let line = "\u{301}".repeat(MAX_ROW_CHARS + 1);
        let rows = all(line.as_bytes(), wrap(80));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn limit_forces_a_break() {
        let row = layout_row(b"abcdef", 10, 3, Wrap::Off, None);
        assert_eq!((row.start, row.end, row.text.as_str()), (10, 13, "abc"));
    }

    #[test]
    fn crlf_split_by_the_limit_shows_the_cr() {
        // A forced break between \r and \n: the \r stays a placeholder so the
        // row ends exactly at the break.
        let row = layout_row(b"a\r\n", 0, 2, Wrap::Off, None);
        assert_eq!((row.end, row.text.as_str()), (2, "a·"));
    }

    /// The marked text of each row laid out from 0.
    fn marked(bytes: &[u8], wrap: Wrap, mark: Range<u64>) -> Vec<Option<String>> {
        let mut rows = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            let row = layout_row(
                &bytes[pos..],
                pos as u64,
                bytes.len() - pos,
                wrap,
                Some(&mark),
            );
            pos = row.end as usize;
            rows.push(row.mark.map(|m| row.text[m].to_owned()));
        }
        rows
    }

    #[test]
    fn the_mark_covers_the_characters_of_its_bytes() {
        assert_eq!(
            marked(b"ab cd\nef", Wrap::Off, 3..5),
            [Some("cd".into()), None]
        );
        assert_eq!(
            marked(b"ab cd\nef", Wrap::Off, 6..8),
            [None, Some("ef".into())]
        );
        // A tab shows as spaces, a control byte as a placeholder.
        assert_eq!(marked(b"a\tb", Wrap::Off, 1..2), [Some(" ".repeat(7))]);
        assert_eq!(marked(b"a\x01b", Wrap::Off, 1..3), [Some("·b".into())]);
        // Inside a multi-byte character: the whole character.
        assert_eq!(
            marked("xéy".as_bytes(), Wrap::Off, 2..3),
            [Some("é".into())]
        );
        assert_eq!(marked(b"abc", Wrap::Off, 1..1), [None], "empty");
    }

    #[test]
    fn a_mark_across_wrapped_rows_shows_on_each() {
        assert_eq!(
            marked(b"hello big world", wrap(10), 6..13),
            [Some("big ".into()), Some("wor".into())]
        );
        // The break falls back to the last space, cutting the marked "wor"
        // off the first row.
        assert_eq!(
            marked(b"hello world", wrap(8), 6..9),
            [None, Some("wor".into())]
        );
    }

    #[test]
    fn mark_columns_count_display_columns() {
        let row = layout_row("日\tab".as_bytes(), 0, 6, Wrap::Off, Some(&(4..6)));
        assert_eq!(row.mark_columns(), Some(8..10));
        assert_eq!(
            layout_row(b"ab", 0, 2, Wrap::Off, None).mark_columns(),
            None
        );
    }

    #[test]
    fn visible_clips_the_mark() {
        let row = layout_row(b"abcdef", 0, 6, Wrap::Off, Some(&(1..5)));
        let shown = row.visible(2, 2);
        assert_eq!((shown.text.as_str(), shown.mark), ("cd", Some(0..2)));
        assert_eq!(row.visible(5, 3).mark, None);
        let wide = layout_row("a日b".as_bytes(), 0, 5, Wrap::Off, Some(&(1..4)));
        let shown = wide.visible(2, 3);
        assert_eq!((shown.text.as_str(), shown.mark), (" b", Some(0..1)));
    }

    #[test]
    fn decode_handles_valid_truncated_and_stray_bytes() {
        assert_eq!(decode("é".as_bytes()), (Some('é'), 2));
        assert_eq!(decode(&[0xe2, 0x82]), (None, 1)); // truncated
        assert_eq!(decode(&[0x80]), (None, 1)); // stray continuation
        assert_eq!(decode(&[0xc0, 0x80]), (None, 1)); // overlong
        assert_eq!(decode("😀".as_bytes()), (Some('😀'), 4));
    }

    #[test]
    fn visible_slices_by_columns() {
        let row = &all("ab日cd".as_bytes(), Wrap::Off)[0];
        // The wide char straddles column 3: it becomes a space at either edge.
        assert_eq!(row.visible(0, 3).text, "ab ");
        assert_eq!(row.visible(3, 10).text, " cd");
        assert_eq!(row.visible(2, 2).text, "日");
        assert_eq!(row.visible(50, 10).text, "");
    }

    #[test]
    fn visible_keeps_dim_ranges() {
        let row = &all(b"ab\x01cd", Wrap::Off)[0];
        let Visible { text, dim, .. } = row.visible(1, 3);
        assert_eq!(text, "b·c");
        assert_eq!(&text[dim[0].clone()], "·");
    }

    #[test]
    fn visible_is_linear_in_dim_ranges() {
        // A no-wrap row of binary data: every character a placeholder.
        let row = &all(&[0x01; MAX_ROW_CHARS], Wrap::Off)[0];
        let start = std::time::Instant::now();
        let Visible { text, dim, .. } = row.visible(0, MAX_ROW_CHARS as u32);
        assert_eq!(dim.len(), MAX_ROW_CHARS);
        assert_eq!(text.chars().count(), MAX_ROW_CHARS);
        // Quadratic takes seconds here in a debug build; linear ~1 ms.
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }
}
