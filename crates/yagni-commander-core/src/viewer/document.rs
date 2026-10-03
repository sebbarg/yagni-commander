//! A file opened for viewing: cached positional reads and navigation by
//! byte position. Rows are laid out on demand around the position shown, so
//! nothing depends on the size of the file.
//!
//! Row starts are found going up without scanning back to the start of a
//! giant line: inside a line, every 64 KiB-aligned offset with no `\n` in the
//! 64 KiB before it is a forced row start (snapped to a character boundary).
//! Both directions apply that rule by looking back at most a few blocks, so
//! they always agree.

use std::ops::Range;
use std::rc::Rc;

use super::hex::{HEX_ROW, hex_row, offset_digits};
use super::layout::{BLOCK, Highlight, ROW_WINDOW, Row, Wrap, decode, layout_row};
use super::source::Source;

/// Cached blocks (16 x 64 KiB = 1 MiB).
const CACHE_BLOCKS: usize = 16;
/// How far back `segment_start` looks: far enough to always pass a forced
/// break inside a giant line (see there).
const LOOKBACK: u64 = 3 * BLOCK;
/// How far a double-click's word reaches each way.
const WORD_REACH: u64 = 4096;

pub struct Document<S: Source> {
    source: S,
    len: u64,
    /// Most recently used first.
    cache: Vec<(u64, Rc<Vec<u8>>)>,
    error: Option<String>,
}

impl<S: Source> Document<S> {
    pub fn new(source: S) -> Self {
        let len = source.len();
        Self {
            source,
            len,
            cache: Vec::new(),
            error: None,
        }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// The last read error, if any. Failed reads look like the end of the file.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Notices a file that shrank since the last call: the cached blocks may
    /// hold bytes that are gone. (Growth is ignored; following a growing file
    /// is v2.)
    fn check_shrunk(&mut self) {
        if let Ok(now) = self.source.current_len()
            && now < self.len
        {
            self.len = now;
            self.cache.clear();
        }
    }

    fn block(&mut self, index: u64) -> Rc<Vec<u8>> {
        if let Some(at) = self.cache.iter().position(|(ix, _)| *ix == index) {
            let entry = self.cache.remove(at);
            self.cache.insert(0, entry);
            return self.cache[0].1.clone();
        }
        let start = index * BLOCK;
        let want = (self.len.saturating_sub(start)).min(BLOCK) as usize;
        let mut buf = vec![0; want];
        let mut got = 0;
        while got < want {
            match self.source.read_at(&mut buf[got..], start + got as u64) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.error = Some(e.to_string());
                    break;
                }
            }
        }
        if got < want {
            // Shrunk since opened (or unreadable): the end is here now.
            buf.truncate(got);
            self.len = start + got as u64;
            self.cache.clear();
        }
        let block = Rc::new(buf);
        self.cache.insert(0, (index, block.clone()));
        self.cache.truncate(CACHE_BLOCKS);
        block
    }

    /// Up to `n` bytes at `start`, fewer at the end of the file.
    pub fn bytes(&mut self, start: u64, n: usize) -> Vec<u8> {
        let end = (start + n as u64).min(self.len);
        let mut out = Vec::with_capacity(end.saturating_sub(start) as usize);
        let mut pos = start;
        while pos < end {
            let block = self.block(pos / BLOCK);
            let from = (pos % BLOCK) as usize;
            let to = ((end - pos) as usize + from).min(block.len());
            if from >= to {
                break;
            }
            out.extend_from_slice(&block[from..to]);
            pos += (to - from) as u64;
        }
        out
    }

    /// `\n` bytes in `from..to`, read in 1 MiB chunks past the cache.
    pub fn count_newlines(&mut self, from: u64, to: u64) -> u64 {
        let mut buf = vec![0; 1 << 20];
        let mut count = 0;
        let mut pos = from;
        while pos < to {
            let n = ((to - pos) as usize).min(buf.len());
            match self.source.read_at(&mut buf[..n], pos) {
                Ok(0) => break,
                Ok(got) => {
                    count += buf[..got].iter().filter(|&&b| b == b'\n').count() as u64;
                    pos += got as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.error = Some(e.to_string());
                    break;
                }
            }
        }
        count
    }

    /// Whether checkpoint `c` (a multiple of [`BLOCK`]) is a forced row start.
    fn forced(&mut self, c: u64) -> bool {
        if c < BLOCK || c >= self.len {
            return false;
        }
        // A short read means the file shrank: no break past its new end.
        let before = self.bytes(c - BLOCK, BLOCK as usize);
        before.len() == BLOCK as usize && !before.contains(&b'\n')
    }

    /// The first character boundary at or after `c`: past a valid multi-byte
    /// character that starts before `c` and spans it.
    fn snap(&mut self, c: u64) -> u64 {
        let lo = c.saturating_sub(3);
        let bytes = self.bytes(lo, (c - lo) as usize + 4);
        for back in 1..=(c - lo) as usize {
            let at = (c - lo) as usize - back;
            if at >= bytes.len() {
                continue; // shrank under us
            }
            if let (Some(_), len) = decode(&bytes[at..])
                && len > back
            {
                return c - back as u64 + len as u64;
            }
        }
        c
    }

    /// The forced break after `start`, if one is within reach of its row.
    fn break_after(&mut self, start: u64) -> Option<u64> {
        let c = (start / BLOCK + 1) * BLOCK;
        if c - start > ROW_WINDOW as u64 || !self.forced(c) {
            return None;
        }
        Some(self.snap(c))
    }

    fn row(&mut self, start: u64, wrap: Wrap) -> Row {
        self.marked_row(start, wrap, Highlight::default())
    }

    fn marked_row(&mut self, start: u64, wrap: Wrap, hl: Highlight) -> Row {
        let window = self.bytes(start, ROW_WINDOW);
        let limit = match self.break_after(start) {
            Some(b) => ((b - start) as usize).min(window.len()),
            None => window.len(),
        };
        layout_row(&window, start, limit, wrap, hl)
    }

    /// Up to `n` rows from the row start `top`.
    pub fn rows(&mut self, top: u64, n: usize, wrap: Wrap) -> Vec<Row> {
        self.marked_rows(top, n, wrap, Highlight::default())
    }

    /// [`Self::rows`], with [`Row::marks`] and [`Row::selection`] set where
    /// they show `hl`'s ranges.
    pub fn marked_rows(&mut self, top: u64, n: usize, wrap: Wrap, hl: Highlight) -> Vec<Row> {
        self.check_shrunk();
        if wrap == Wrap::Hex {
            return self.hex_rows(top, n, hl);
        }
        let mut rows = Vec::new();
        let mut pos = top;
        while rows.len() < n && pos < self.len {
            let row = self.marked_row(pos, wrap, hl);
            if row.end == pos {
                break; // the file shrank under us
            }
            pos = row.end;
            rows.push(row);
        }
        rows
    }

    fn hex_rows(&mut self, top: u64, n: usize, hl: Highlight) -> Vec<Row> {
        let digits = offset_digits(self.len);
        let mut rows = Vec::new();
        let mut pos = top;
        while rows.len() < n && pos < self.len {
            let bytes = self.bytes(pos, HEX_ROW as usize);
            if bytes.is_empty() {
                break; // the file shrank under us
            }
            let row = hex_row(&bytes, pos, digits, hl);
            pos = row.end;
            rows.push(row);
        }
        rows
    }

    /// Hex rows in the file.
    fn hex_row_count(&self) -> u64 {
        self.len.div_ceil(HEX_ROW)
    }

    /// Row starts in `from..to`, laying out from the row start `from`.
    fn row_starts(&mut self, from: u64, to: u64, wrap: Wrap) -> Vec<u64> {
        let mut starts = Vec::new();
        let mut pos = from;
        while pos < to && pos < self.len {
            starts.push(pos);
            let end = self.row(pos, wrap).end;
            if end == pos {
                break;
            }
            pos = end;
        }
        starts
    }

    /// The latest row start that no row can cross: the line start or forced
    /// break at or before `pos`.
    ///
    /// Without a `\n` in the 3 blocks before `pos`, a forced break is always
    /// found there: the checkpoint at or below `pos` has a newline-free block
    /// before it, and if it snaps past `pos`, the one below does.
    fn segment_start(&mut self, pos: u64) -> u64 {
        let lo = pos.saturating_sub(LOOKBACK);
        let before = self.bytes(lo, (pos - lo) as usize);
        let line = match before.iter().rposition(|&b| b == b'\n') {
            Some(i) => lo + i as u64 + 1,
            None => lo,
        };
        let mut c = pos / BLOCK * BLOCK;
        while c > line && c >= BLOCK {
            if self.forced(c) {
                let s = self.snap(c);
                if s <= pos {
                    return s;
                }
            }
            c -= BLOCK;
        }
        line
    }

    /// Start of the row containing byte `pos` (clamped to the file).
    pub fn row_start_at(&mut self, pos: u64, wrap: Wrap) -> u64 {
        self.check_shrunk();
        if self.len == 0 {
            return 0;
        }
        let pos = pos.min(self.len - 1);
        if wrap == Wrap::Hex {
            return pos / HEX_ROW * HEX_ROW;
        }
        let seg = self.segment_start(pos);
        *self.row_starts(seg, pos + 1, wrap).last().unwrap_or(&seg)
    }

    /// `by` rows down from `top`, but never so far that the screen of
    /// `screen` rows has empty space below the last row.
    pub fn scroll_down(&mut self, top: u64, by: usize, screen: usize, wrap: Wrap) -> u64 {
        self.check_shrunk();
        if top >= self.len {
            // The file shrank below the top row: show its end.
            return self.last_top(screen, wrap);
        }
        if wrap == Wrap::Hex {
            let last = self.last_top(screen, wrap);
            let target = top.saturating_add((by as u64).saturating_mul(HEX_ROW));
            return target.min(last.max(top));
        }
        let rows = self.rows(top, by.saturating_add(screen), wrap);
        let index = by.min(rows.len().saturating_sub(screen));
        rows.get(index).map_or(top, |r| r.start)
    }

    /// `by` rows up from `top` (which may be the file length), stopping at 0.
    pub fn scroll_up(&mut self, top: u64, by: usize, wrap: Wrap) -> u64 {
        self.check_shrunk();
        let mut top = top.min(self.len);
        if wrap == Wrap::Hex {
            let row = if top >= self.len {
                self.hex_row_count()
            } else {
                top / HEX_ROW
            };
            return row.saturating_sub(by as u64) * HEX_ROW;
        }
        let mut left = by;
        while left > 0 && top > 0 {
            let seg = self.segment_start(top - 1);
            let starts = self.row_starts(seg, top, wrap);
            if starts.len() >= left {
                return starts[starts.len() - left];
            }
            left -= starts.len();
            top = seg;
        }
        top
    }

    /// The word (letters, digits, `_`) holding the character at `pos`,
    /// looking at most [`WORD_REACH`] bytes each way; a non-word character
    /// alone; `len..len` at the end. A slice cut inside a multi-byte
    /// character at the reach's edge reads as non-word bytes.
    pub fn word_at(&mut self, pos: u64) -> Range<u64> {
        self.check_shrunk();
        if pos >= self.len {
            return self.len..self.len;
        }
        let lo = pos.saturating_sub(WORD_REACH);
        let bytes = self.bytes(lo, (pos - lo + WORD_REACH) as usize);
        // (offset, length, is a word character) of every unit from `lo`.
        let mut units = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let (ch, len) = decode(&bytes[i..]);
            let word = ch.is_some_and(|c| c.is_alphanumeric() || c == '_');
            units.push((i, len, word));
            i += len;
        }
        let at = (pos - lo) as usize;
        let Some(k) = units.iter().position(|&(o, l, _)| o <= at && at < o + l) else {
            return pos..pos + 1; // the file shrank under us
        };
        let (o, l, word) = units[k];
        if !word {
            return lo + o as u64..lo + (o + l) as u64;
        }
        let first = units[..k].iter().rposition(|u| !u.2).map_or(0, |j| j + 1);
        let last = units[k..]
            .iter()
            .position(|u| !u.2)
            .map_or(units.len(), |j| k + j);
        let (end_o, end_l, _) = units[last - 1];
        lo + units[first].0 as u64..lo + (end_o + end_l) as u64
    }

    /// The file line holding `pos` with its `\n`, within its segment: a
    /// giant line stops at forced breaks, as rows do.
    pub fn line_at(&mut self, pos: u64) -> Range<u64> {
        self.check_shrunk();
        if pos >= self.len {
            return self.len..self.len;
        }
        let start = self.segment_start(pos);
        let mut end = pos;
        loop {
            let c = (end / BLOCK + 1) * BLOCK;
            let chunk = self.bytes(end, (c.min(self.len) - end) as usize);
            if let Some(i) = chunk.iter().position(|&b| b == b'\n') {
                return start..end + i as u64 + 1;
            }
            end += chunk.len() as u64;
            if chunk.is_empty() || end >= self.len {
                return start..end.min(self.len);
            }
            if self.forced(c) {
                return start..self.snap(c);
            }
        }
    }

    /// The top row that shows the end of the file at the bottom of a screen.
    pub fn last_top(&mut self, screen: usize, wrap: Wrap) -> u64 {
        self.scroll_up(self.len, screen.max(1), wrap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bytes: impl Into<Vec<u8>>) -> Document<Vec<u8>> {
        Document::new(bytes.into())
    }

    fn starts(doc: &mut Document<Vec<u8>>, wrap: Wrap) -> Vec<u64> {
        doc.rows(0, usize::MAX, wrap)
            .iter()
            .map(|r| r.start)
            .collect()
    }

    /// Scrolling up one row at a time from the end visits exactly the row
    /// starts that laying out forward produced, and `row_start_at` maps every
    /// byte to the row containing it.
    fn assert_consistent(bytes: Vec<u8>, wrap: Wrap) {
        let mut d = doc(bytes);
        let forward = starts(&mut d, wrap);
        let mut backward = Vec::new();
        let mut top = d.len();
        while top > 0 {
            top = d.scroll_up(top, 1, wrap);
            backward.push(top);
        }
        backward.reverse();
        assert_eq!(forward, backward, "{wrap:?}");
        let rows = d.rows(0, usize::MAX, wrap);
        for row in &rows {
            for pos in [row.start, (row.start + row.end) / 2, row.end - 1] {
                assert_eq!(d.row_start_at(pos, wrap), row.start, "pos {pos} {wrap:?}");
            }
        }
    }

    /// Deterministic pseudo-random bytes (no rand dependency).
    fn noise(len: usize, seed: u64, newline_every: u64) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                match (x >> 24) as u8 {
                    _ if newline_every > 0 && x.is_multiple_of(newline_every) => b'\n',
                    // newline_every == 0: no newlines at all (giant lines).
                    b'\n' => b'.',
                    b => b,
                }
            })
            .collect()
    }

    #[test]
    fn up_and_down_agree_on_text() {
        let text = "short\n\nhello big world, a longer line\tthat wraps\n日本語のテキスト\nend";
        for wrap in [
            Wrap::Columns(1),
            Wrap::Columns(7),
            Wrap::Columns(80),
            Wrap::Off,
        ] {
            assert_consistent(text.into(), wrap);
        }
    }

    #[test]
    fn up_and_down_agree_on_random_bytes() {
        // Random bytes: invalid UTF-8, stray continuations, controls.
        for seed in 1..6 {
            for wrap in [Wrap::Columns(3), Wrap::Columns(50), Wrap::Off] {
                assert_consistent(noise(20_000, seed, 300), wrap);
            }
        }
    }

    #[test]
    fn up_and_down_agree_on_giant_lines() {
        // No newlines for several blocks: forced breaks at 64 KiB checkpoints.
        // Wide rows keep the row count (and the debug-build run time) low:
        // every one-row step up lays out up to 3 blocks.
        let giant = noise(4 * BLOCK as usize + 123, 7, 0);
        assert_consistent(giant.clone(), Wrap::Off);
        assert_consistent(giant, Wrap::Columns(1999));
        // Multi-byte characters (1 to 4 bytes) straddling the checkpoints.
        let wide = "aé日😀".repeat(25_000).into_bytes();
        assert_consistent(wide.clone(), Wrap::Off);
        assert_consistent(wide, Wrap::Columns(997));
    }

    #[test]
    fn giant_lines_break_at_checkpoints_only() {
        let mut d = doc(vec![b'x'; 3 * BLOCK as usize]);
        let s = starts(&mut d, Wrap::Off);
        assert!(s.contains(&BLOCK) && s.contains(&(2 * BLOCK)));
        // A 100 KiB line starting after a newline in the first block: no
        // checkpoint qualifies, so no forced break.
        let mut bytes = vec![b'a'; 10];
        bytes.push(b'\n');
        bytes.extend(vec![b'x'; 100 * 1024]);
        let mut d = doc(bytes);
        let s = starts(&mut d, Wrap::Off);
        assert!(!s.contains(&BLOCK));
    }

    #[test]
    fn scroll_down_stops_when_the_last_row_is_on_screen() {
        let text: String = (0..10).map(|n| format!("{n}\n")).collect();
        let mut d = doc(text);
        let w = Wrap::Off;
        assert_eq!(d.scroll_down(0, 1, 4, w), 2);
        assert_eq!(d.scroll_down(0, 100, 4, w), d.last_top(4, w));
        assert_eq!(d.last_top(4, w), 12); // rows "6".."9"
        assert_eq!(d.scroll_down(12, 1, 4, w), 12);
        assert_eq!(
            d.scroll_down(0, 1, 20, w),
            0,
            "file shorter than the screen"
        );
    }

    #[test]
    fn scroll_up_stops_at_the_start() {
        let mut d = doc("a\nb\nc\n");
        assert_eq!(d.scroll_up(4, 1, Wrap::Off), 2);
        assert_eq!(d.scroll_up(4, 9, Wrap::Off), 0);
        assert_eq!(d.scroll_up(0, 1, Wrap::Off), 0);
    }

    #[test]
    fn end_of_newline_only_file() {
        let mut d = doc("\n\n\n");
        assert_eq!(d.last_top(2, Wrap::Off), 1);
        assert_eq!(d.rows(1, 5, Wrap::Off).len(), 2);
        let mut empty = doc("");
        assert_eq!(empty.last_top(5, Wrap::Off), 0);
        assert!(empty.rows(0, 5, Wrap::Off).is_empty());
        assert_eq!(empty.row_start_at(0, Wrap::Off), 0);
    }

    #[test]
    fn rewrapping_keeps_the_row_containing_the_top() {
        let mut d = doc("hello big world\nnext");
        let narrow = Wrap::Columns(6);
        let top = d.scroll_down(0, 2, 1, narrow); // "world"
        assert_eq!(d.row_start_at(top, Wrap::Off), 0);
        assert_eq!(d.row_start_at(16, narrow), 16);
    }

    #[test]
    fn counts_newlines_in_a_range() {
        let mut d = doc("a\nb\n\nc");
        assert_eq!(d.count_newlines(0, 6), 3);
        assert_eq!(d.count_newlines(2, 4), 1);
    }

    #[test]
    fn cache_serves_repeated_reads() {
        let mut d = doc(noise(3 * BLOCK as usize, 3, 50));
        let a = d.bytes(BLOCK - 10, 20);
        let b = d.bytes(BLOCK - 10, 20);
        assert_eq!(a, b);
        assert_eq!(a.len(), 20);
        assert_eq!(d.bytes(3 * BLOCK - 5, 100).len(), 5, "clipped at the end");
    }

    /// Fails reads at or past `fail_at`; the first read is `Interrupted`.
    struct Flaky {
        bytes: Vec<u8>,
        fail_at: u64,
        interrupted: std::cell::Cell<bool>,
    }

    impl Source for Flaky {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_at(&self, buf: &mut [u8], pos: u64) -> std::io::Result<usize> {
            if !self.interrupted.replace(true) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if pos >= self.fail_at {
                return Err(std::io::Error::other("disk on fire"));
            }
            let n = buf.len().min((self.fail_at - pos) as usize);
            self.bytes.read_at(&mut buf[..n], pos)
        }
    }

    fn flaky(fail_at: u64) -> Document<Flaky> {
        Document::new(Flaky {
            bytes: b"a\n".repeat(BLOCK as usize),
            fail_at,
            interrupted: Default::default(),
        })
    }

    #[test]
    fn read_errors_end_the_file_and_are_reported() {
        let mut d = flaky(BLOCK + 10);
        assert!(d.error().is_none());
        let rows = d.rows(0, usize::MAX, Wrap::Off);
        assert_eq!(rows.last().unwrap().end, BLOCK + 10);
        assert_eq!(d.error(), Some("disk on fire"));
        assert_eq!(d.len(), BLOCK + 10);
        assert!(!d.is_empty());
        assert_eq!(d.source().fail_at, BLOCK + 10);
    }

    #[test]
    fn count_newlines_stops_at_read_errors() {
        let mut d = flaky(100);
        assert_eq!(d.count_newlines(0, 1000), 50);
        assert_eq!(d.error(), Some("disk on fire"));
        // Short reads end the count too.
        let mut d = doc("a\nb\n");
        assert_eq!(d.count_newlines(0, 100), 2);
    }

    /// A huge file of zeros that counts its reads.
    struct Huge {
        len: u64,
        reads: std::cell::Cell<usize>,
    }

    impl Source for Huge {
        fn len(&self) -> u64 {
            self.len
        }

        fn read_at(&self, buf: &mut [u8], pos: u64) -> std::io::Result<usize> {
            self.reads.set(self.reads.get() + 1);
            let n = (self.len.saturating_sub(pos) as usize).min(buf.len());
            buf[..n].fill(0);
            Ok(n)
        }
    }

    #[test]
    fn hex_rows_are_16_bytes_at_multiples_of_16() {
        let mut d = doc((0..40u8).collect::<Vec<u8>>());
        let rows = d.rows(0, 10, Wrap::Hex);
        let spans: Vec<(u64, u64)> = rows.iter().map(|r| (r.start, r.end)).collect();
        assert_eq!(spans, [(0, 16), (16, 32), (32, 40)]);
        assert!(rows[2].text.starts_with("00000020  20 21 "));
        assert_eq!(d.row_start_at(37, Wrap::Hex), 32);
        assert_eq!(d.row_start_at(100, Wrap::Hex), 32, "clamped");
        let marked = d.marked_rows(0, 3, Wrap::Hex, Highlight::mark(&(15..17)));
        assert_eq!((marked[0].marks.len(), marked[1].marks.len()), (2, 2));
        assert!(doc(Vec::new()).rows(0, 3, Wrap::Hex).is_empty());
        assert_eq!(doc(Vec::new()).row_start_at(5, Wrap::Hex), 0);
    }

    #[test]
    fn hex_scrolling_is_arithmetic() {
        let mut d = doc(vec![b'x'; 100]); // rows at 0, 16, ..., 96
        assert_eq!(d.scroll_down(0, 2, 3, Wrap::Hex), 32);
        assert_eq!(d.scroll_down(32, 10, 3, Wrap::Hex), 64, "the last screen");
        assert_eq!(d.last_top(3, Wrap::Hex), 64);
        assert_eq!(d.scroll_up(64, 1, Wrap::Hex), 48);
        assert_eq!(d.scroll_up(16, 5, Wrap::Hex), 0);
        assert_eq!(d.scroll_up(100, 1, Wrap::Hex), 96, "from the end");
        assert_eq!(d.scroll_down(200, 1, 3, Wrap::Hex), 64, "past the end");
        assert_eq!(doc(vec![b'x'; 5]).last_top(3, Wrap::Hex), 0);
    }

    #[test]
    fn hex_navigation_never_reads_a_huge_file() {
        let len = 5 << 30;
        let mut d = Document::new(Huge {
            len,
            reads: Default::default(),
        });
        let last = d.last_top(10, Wrap::Hex);
        assert_eq!(last, len - 10 * HEX_ROW);
        assert_eq!(d.scroll_up(last, 3, Wrap::Hex), last - 48);
        assert_eq!(d.row_start_at(len / 2 + 7, Wrap::Hex), len / 2);
        assert_eq!(d.source().reads.get(), 0);
        let rows = d.rows(last, 10, Wrap::Hex);
        assert_eq!(rows.len(), 10);
        assert!(
            rows[0].text.starts_with("13FFFFF60  00 "),
            "{}",
            rows[0].text
        );
    }

    #[test]
    fn marked_rows_carry_the_selection() {
        let mut d = doc(b"one\ntwo\nthree\n".to_vec());
        let sel = 2..9;
        let rows = d.marked_rows(0, 3, Wrap::Off, Highlight::selection(&sel));
        let parts: Vec<&str> = rows
            .iter()
            .flat_map(|r| r.selection.iter().map(|s| &r.text[s.clone()]))
            .collect();
        assert_eq!(parts, ["e", "two", "t"]);
        let rows = d.marked_rows(0, 1, Wrap::Hex, Highlight::selection(&sel));
        assert_eq!(rows[0].selection.len(), 2);
    }

    #[test]
    fn word_at_finds_letters_digits_and_underscores() {
        let mut d = doc("say hello_wörld2, ok".as_bytes().to_vec());
        assert_eq!(d.word_at(6), 4..17, "hello_wörld2 (ö is 2 bytes)");
        assert_eq!(d.word_at(4), 4..17, "from its first byte");
        assert_eq!(d.word_at(3), 3..4, "a space alone");
        assert_eq!(d.word_at(17), 17..18, "the comma alone");
        assert_eq!(d.word_at(0), 0..3, "at the start of the file");
        assert_eq!(d.word_at(19), 19..21, "at its end");
        assert_eq!(d.word_at(21), 21..21, "past the end");
    }

    #[test]
    fn word_at_looks_at_most_4_kib_each_way() {
        let mut d = doc(vec![b'x'; 20_000]);
        assert_eq!(d.word_at(10_000), 10_000 - 4096..10_000 + 4096);
    }

    #[test]
    fn line_at_selects_the_file_line_with_its_newline() {
        let mut d = doc(b"one\ntwo three\nlast".to_vec());
        assert_eq!(d.line_at(6), 4..14);
        assert_eq!(d.line_at(4), 4..14);
        assert_eq!(d.line_at(15), 14..18, "the last line has no newline");
        assert_eq!(d.line_at(0), 0..4);
        assert_eq!(d.line_at(18), 18..18, "past the end");
    }

    #[test]
    fn line_at_stops_at_the_segment_of_a_giant_line() {
        let mut d = doc(vec![b'x'; 5 * BLOCK as usize]);
        assert_eq!(d.line_at(BLOCK * 2 + 10), BLOCK * 2..BLOCK * 3);
    }
}
