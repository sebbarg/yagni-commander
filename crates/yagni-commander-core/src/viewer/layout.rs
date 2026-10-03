//! Splits a window of file bytes into display rows: tabs expanded, control
//! characters and invalid UTF-8 replaced, wrapped to a column count.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

use super::hex::{chars_at, code_at};

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
    /// Hex mode: 16 bytes per row (see `hex`).
    Hex,
}

/// What rows highlight: the current search match and the selection, both
/// file byte ranges.
#[derive(Clone, Copy, Debug, Default)]
pub struct Highlight<'a> {
    pub mark: Option<&'a Range<u64>>,
    pub selection: Option<&'a Range<u64>>,
}

impl<'a> Highlight<'a> {
    pub fn mark(range: &'a Range<u64>) -> Self {
        Self {
            mark: Some(range),
            selection: None,
        }
    }

    pub fn selection(range: &'a Range<u64>) -> Self {
        Self {
            mark: None,
            selection: Some(range),
        }
    }
}

/// Which half of a hex row a click is in. Ignored by text rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HexColumn {
    Codes,
    Chars,
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
    /// The ranges of `text` that show the marked bytes (the current match):
    /// at most one in text mode, two in hex (the codes and the characters).
    pub marks: Vec<Range<usize>>,
    /// The ranges of `text` that show selected bytes: at most one in text
    /// mode, two in hex.
    pub selection: Vec<Range<usize>>,
    /// Text rows: for each character of `text`, the offset from `start` of
    /// the file byte it shows (a tab's spaces all point at the tab), plus a
    /// last entry where the shown bytes end (before a line ending). Hex
    /// rows: empty.
    pub sources: Vec<u32>,
}

impl Row {
    /// The part of the row from column `from_col`, at most `cols` wide, with
    /// its dim ranges, marks and selection. A wide character cut by either edge becomes a
    /// space.
    pub fn visible(&self, from_col: u32, cols: u32) -> Visible {
        let to_col = from_col.saturating_add(cols);
        let mut text = String::new();
        let mut dim = Vec::new();
        // One slot per mark of the row, filled as its characters show.
        let mut marks: Vec<Option<Range<usize>>> = vec![None; self.marks.len()];
        let mut selection: Vec<Option<Range<usize>>> = vec![None; self.selection.len()];
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
            if let Some(k) = self.marks.iter().position(|m| m.contains(&ix)) {
                marks[k].get_or_insert(at..at).end = text.len();
            }
            if let Some(k) = self.selection.iter().position(|s| s.contains(&ix)) {
                selection[k].get_or_insert(at..at).end = text.len();
            }
        }
        let marks = marks.into_iter().flatten().collect();
        let selection = selection.into_iter().flatten().collect();
        Visible {
            text,
            dim,
            marks,
            selection,
        }
    }
}

impl Row {
    /// The columns the first of [`Self::marks`] takes.
    pub fn mark_columns(&self) -> Option<Range<u32>> {
        let mark = self.marks.first()?;
        let width = |s: &str| {
            s.chars()
                .map(|c| c.width().unwrap_or(1) as u32)
                .sum::<u32>()
        };
        let start = width(&self.text[..mark.start]);
        Some(start..start + width(&self.text[mark.clone()]))
    }
}

impl Row {
    /// Where the row's shown bytes end: before its line ending.
    pub fn content_end(&self) -> u64 {
        match self.sources.last() {
            Some(&end) => self.start + u64::from(end),
            None => self.end, // hex
        }
    }

    /// The row's cells: (first byte, start column, end column). A text cell
    /// is one character (a tab's spaces are one cell); a hex cell is one
    /// byte's code or character.
    fn cells(&self, column: HexColumn) -> Vec<(u64, f32, f32)> {
        if self.sources.is_empty() {
            return hex_cells(self, column);
        }
        let mut cells: Vec<(u64, f32, f32)> = Vec::new();
        let mut col = 0.0;
        for (ch, &src) in self.text.chars().zip(&self.sources) {
            let w = ch.width().unwrap_or(1) as f32;
            let byte = self.start + u64::from(src);
            match cells.last_mut() {
                Some(last) if last.0 == byte => last.2 += w,
                _ => cells.push((byte, col, col + w)),
            }
            col += w;
        }
        cells
    }

    /// The position between bytes nearest column `x` (fractional, from the
    /// row's first column).
    pub fn boundary_at(&self, x: f32, column: HexColumn) -> u64 {
        self.cells(column)
            .iter()
            .find(|(_, start, end)| (start + end) / 2.0 > x)
            .map_or_else(|| self.content_end(), |c| c.0)
    }

    /// The first byte of the cell under column `x`: in a gap or past the
    /// end, the cell before it; before the first cell, the first one. An
    /// empty row answers its start.
    pub fn char_at(&self, x: f32, column: HexColumn) -> u64 {
        let cells = self.cells(column);
        cells
            .iter()
            .rev()
            .find(|(_, start, _)| *start <= x)
            .or(cells.first())
            .map_or(self.start, |c| c.0)
    }

    /// Hex rows: the characters from one column before they start.
    pub fn hex_column(&self, x: f32) -> HexColumn {
        let digits = self.text.find(' ').unwrap_or(8);
        if x >= chars_at(digits) as f32 - 1.0 {
            HexColumn::Chars
        } else {
            HexColumn::Codes
        }
    }
}

fn hex_cells(row: &Row, column: HexColumn) -> Vec<(u64, f32, f32)> {
    let digits = row.text.find(' ').unwrap_or(8);
    (0..(row.end - row.start) as usize)
        .map(|i| {
            let (at, w) = match column {
                HexColumn::Codes => (code_at(digits, i), 2),
                HexColumn::Chars => (chars_at(digits) + i, 1),
            };
            (row.start + i as u64, at as f32, (at + w) as f32)
        })
        .collect()
}

/// What [`Row::visible`] shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    pub text: String,
    pub dim: Vec<Range<usize>>,
    pub marks: Vec<Range<usize>>,
    pub selection: Vec<Range<usize>>,
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
/// the file) and, wrapping, never past `cols` columns. `hl`: file bytes
/// whose characters get [`Row::marks`] and [`Row::selection`].
pub(crate) fn layout_row(bytes: &[u8], start: u64, limit: usize, wrap: Wrap, hl: Highlight) -> Row {
    let limit = limit.min(bytes.len());
    let mut text = String::new();
    let mut dim = Vec::new();
    let mut sources = Vec::new();
    // Where the shown bytes end, if a line ending ends the row.
    let mut shown_end = None;
    let mut marked: Option<Range<usize>> = None;
    let mut selected: Option<Range<usize>> = None;
    let mut col = 0u32;
    let mut chars = 0;
    let mut i = 0;
    // Where to break for word wrap: after the last space or tab
    // (byte offset, text length, dim count, column, sources count).
    let mut soft: Option<(usize, usize, usize, u32, usize)> = None;
    while i < limit && chars < MAX_ROW_CHARS {
        let (ch, len) = decode(&bytes[i..]);
        if ch == Some('\n') {
            shown_end = Some(i);
            i += 1;
            break;
        }
        if ch == Some('\r') && i + 1 < limit && bytes[i + 1] == b'\n' {
            shown_end = Some(i);
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
            if let Some((si, st, sd, sc, ss)) = soft {
                i = si;
                text.truncate(st);
                dim.truncate(sd);
                col = sc;
                sources.truncate(ss);
            }
            break;
        }
        let at = text.len();
        if ch == Some('\t') {
            text.extend(std::iter::repeat_n(' ', w as usize));
            sources.extend(std::iter::repeat_n(i as u32, w as usize));
        } else {
            text.push(shown);
            sources.push(i as u32);
        }
        if is_dim {
            dim.push(at..text.len());
        }
        let pos = start + i as u64;
        let covers = |r: &Range<u64>| pos < r.end && pos + len as u64 > r.start;
        if hl.mark.is_some_and(covers) {
            marked.get_or_insert(at..at).end = text.len();
        }
        if hl.selection.is_some_and(covers) {
            selected.get_or_insert(at..at).end = text.len();
        }
        col += w;
        chars += 1;
        i += len;
        if matches!(ch, Some(' ' | '\t')) {
            soft = Some((i, text.len(), dim.len(), col, sources.len()));
        }
    }
    // A word-wrap break may have cut the highlighted text off.
    let trim = |r: Option<Range<usize>>| -> Vec<Range<usize>> {
        r.map(|m| m.start..m.end.min(text.len()))
            .filter(|m| m.start < m.end)
            .into_iter()
            .collect()
    };
    let (marks, selection) = (trim(marked), trim(selected));
    sources.push(shown_end.unwrap_or(i) as u32);
    Row {
        start,
        end: start + i as u64,
        text,
        dim,
        width: col,
        marks,
        selection,
        sources,
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
            let row = layout_row(
                &bytes[pos..],
                pos as u64,
                bytes.len() - pos,
                wrap,
                Highlight::default(),
            );
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
        let row = layout_row(b"abcdef", 10, 3, Wrap::Off, Highlight::default());
        assert_eq!((row.start, row.end, row.text.as_str()), (10, 13, "abc"));
    }

    #[test]
    fn crlf_split_by_the_limit_shows_the_cr() {
        // A forced break between \r and \n: the \r stays a placeholder so the
        // row ends exactly at the break.
        let row = layout_row(b"a\r\n", 0, 2, Wrap::Off, Highlight::default());
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
                Highlight::mark(&mark),
            );
            pos = row.end as usize;
            rows.push(row.marks.first().map(|m| row.text[m.clone()].to_owned()));
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
        let row = layout_row(
            "日\tab".as_bytes(),
            0,
            6,
            Wrap::Off,
            Highlight::mark(&(4..6)),
        );
        assert_eq!(row.mark_columns(), Some(8..10));
        assert_eq!(
            layout_row(b"ab", 0, 2, Wrap::Off, Highlight::default()).mark_columns(),
            None
        );
    }

    #[test]
    fn visible_clips_the_mark() {
        let row = layout_row(b"abcdef", 0, 6, Wrap::Off, Highlight::mark(&(1..5)));
        let shown = row.visible(2, 2);
        assert_eq!(shown.text, "cd");
        assert_eq!(shown.marks, std::iter::once(0..2).collect::<Vec<_>>());
        assert!(row.visible(5, 3).marks.is_empty());
        let wide = layout_row("a日b".as_bytes(), 0, 5, Wrap::Off, Highlight::mark(&(1..4)));
        let shown = wide.visible(2, 3);
        assert_eq!(shown.text, " b");
        assert_eq!(shown.marks, std::iter::once(0..1).collect::<Vec<_>>());
    }

    fn row_of(bytes: &[u8]) -> Row {
        layout_row(bytes, 100, bytes.len(), Wrap::Off, Highlight::default())
    }

    const C: HexColumn = HexColumn::Codes;

    #[test]
    fn sources_point_at_the_bytes_shown() {
        let row = row_of("a\té\x01".as_bytes());
        // a, 7 spaces for the tab, é (2 bytes), the placeholder, then the end.
        assert_eq!(row.sources, [0, 1, 1, 1, 1, 1, 1, 1, 2, 4, 5]);
        let row = row_of(b"ab\r\n");
        assert_eq!(row.sources, [0, 1, 2], "the line ending is not shown");
        assert_eq!(row.content_end(), 102);
    }

    #[test]
    fn a_wrap_break_trims_the_sources() {
        let row = layout_row(
            b"hello big world",
            0,
            15,
            Wrap::Columns(10),
            Highlight::default(),
        );
        assert_eq!(row.text, "hello big ");
        assert_eq!(row.sources.len(), row.text.chars().count() + 1);
        assert_eq!(*row.sources.last().unwrap(), 10);
    }

    #[test]
    fn boundary_at_takes_the_nearer_edge_of_a_cell() {
        let row = row_of(b"abc");
        assert_eq!(row.boundary_at(0.2, C), 100);
        assert_eq!(row.boundary_at(0.7, C), 101);
        assert_eq!(row.boundary_at(2.6, C), 103);
        assert_eq!(row.boundary_at(50.0, C), 103, "past the end");
        assert_eq!(row.boundary_at(-3.0, C), 100, "before the start");
    }

    #[test]
    fn boundary_at_treats_tabs_and_wide_characters_as_one_cell() {
        let row = row_of("a\tb".as_bytes()); // the tab spans columns 1..8
        assert_eq!(row.boundary_at(3.0, C), 101);
        assert_eq!(row.boundary_at(6.0, C), 102);
        let row = row_of("日本".as_bytes()); // 2 columns each, 3 bytes each
        assert_eq!(row.boundary_at(0.9, C), 100);
        assert_eq!(row.boundary_at(1.1, C), 103);
        assert_eq!(row.boundary_at(3.5, C), 106);
    }

    #[test]
    fn char_at_is_the_cell_under_the_mouse() {
        let row = row_of("ab日c\n".as_bytes());
        assert_eq!(row.char_at(1.9, C), 101);
        assert_eq!(row.char_at(2.1, C), 102);
        assert_eq!(row.char_at(3.9, C), 102, "the wide character's right half");
        assert_eq!(row.char_at(4.0, C), 105);
        assert_eq!(
            row.char_at(40.0, C),
            105,
            "past the end: the last character"
        );
        assert_eq!(row.char_at(-1.0, C), 100);
    }

    #[test]
    fn an_empty_row_answers_its_start() {
        let row = row_of(b"\n");
        assert_eq!(row.sources, [0]);
        assert_eq!(row.boundary_at(5.0, C), 100);
        assert_eq!(row.char_at(5.0, C), 100);
    }

    #[test]
    fn char_at_past_a_wrapped_row_stays_in_the_row() {
        let row = layout_row(
            b"hello big world",
            0,
            15,
            Wrap::Columns(10),
            Highlight::default(),
        );
        assert_eq!(row.text, "hello big ");
        assert_eq!(
            row.char_at(30.0, C),
            9,
            "the row's last character, not the next row's"
        );
        let row = row_of(b"ab\r\n");
        assert_eq!(row.char_at(30.0, C), 101, "never the \\r of a line ending");
    }

    fn selected(bytes: &[u8], wrap: Wrap, sel: Range<u64>) -> Vec<Option<String>> {
        let mut rows = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            let hl = Highlight::selection(&sel);
            let row = layout_row(&bytes[pos..], pos as u64, bytes.len() - pos, wrap, hl);
            pos = row.end as usize;
            rows.push(
                row.selection
                    .first()
                    .map(|s| row.text[s.clone()].to_owned()),
            );
        }
        rows
    }

    #[test]
    fn the_selection_covers_its_characters_on_every_row() {
        assert_eq!(
            selected(b"ab\ncd\nef", Wrap::Off, 1..7),
            [Some("b".into()), Some("cd".into()), Some("e".into())]
        );
        // Only the line ending selected: nothing to draw.
        assert_eq!(selected(b"ab\ncd", Wrap::Off, 2..3), [None, None]);
        assert_eq!(
            selected(b"hello big world", Wrap::Columns(10), 6..13),
            [Some("big ".into()), Some("wor".into())]
        );
    }

    #[test]
    fn mark_and_selection_are_independent() {
        let (mark, sel) = (1..2, 0..3);
        let hl = Highlight {
            mark: Some(&mark),
            selection: Some(&sel),
        };
        let row = layout_row(b"abc", 0, 3, Wrap::Off, hl);
        assert_eq!(row.marks, vec![1..2]);
        assert_eq!(row.selection, vec![0..3]);
    }

    #[test]
    fn visible_clips_the_selection() {
        let row = layout_row(b"abcdef", 0, 6, Wrap::Off, Highlight::selection(&(1..5)));
        let shown = row.visible(2, 2);
        assert_eq!(shown.selection, vec![0..2]);
        assert!(row.visible(5, 3).selection.is_empty());
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
