//! Ctrl-F / F3 / Shift-F3 in the viewer: the next or previous match of a
//! [`TextQuery`] from a byte position, read block by block so a multi-GB
//! file is never held in memory. Matches are Alt-F7's (`find_at`: one line
//! at most), and forward blocks are cut at line ends like Alt-F7's, so a
//! search from byte 0 finds whatever Alt-F7 found.

use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::source::Source;
use crate::find::text::{BLOCK, TextQuery};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The first match starting at or after the position.
    Forward,
    /// The last match starting before the position.
    Backward,
}

/// Bytes read before a forward search's start, so `^` and `\b` see the
/// character before it (UTF-8: at most 4 bytes).
const CONTEXT: u64 = 4;

/// Finds the match nearest `from` in `direction`. `progress` holds the byte
/// position reached; `cancel` is checked per block (`Interrupted`).
pub fn find(
    source: &impl Source,
    query: &TextQuery,
    from: u64,
    direction: Direction,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> io::Result<Option<Range<u64>>> {
    find_with(BLOCK, source, query, from, direction, cancel, progress)
}

fn find_with(
    block: usize,
    source: &impl Source,
    query: &TextQuery,
    from: u64,
    direction: Direction,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> io::Result<Option<Range<u64>>> {
    let search = Search {
        block,
        source,
        query,
        cancel,
        progress,
    };
    match direction {
        Direction::Forward => search.forward(from),
        Direction::Backward => search.backward(from),
    }
}

struct Search<'a, S: Source> {
    block: usize,
    source: &'a S,
    query: &'a TextQuery,
    cancel: &'a AtomicBool,
    progress: &'a AtomicU64,
}

impl<S: Source> Search<'_, S> {
    fn step(&self, pos: u64) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.progress.store(pos, Ordering::Relaxed);
        Ok(())
    }

    /// Up to `n` bytes at `pos`, fewer at the end of the file.
    fn read(&self, pos: u64, n: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0; n];
        let mut got = 0;
        while got < n {
            match self.source.read_at(&mut buf[got..], pos + got as u64) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        buf.truncate(got);
        Ok(buf)
    }

    fn forward(&self, from: u64) -> io::Result<Option<Range<u64>>> {
        let mut base = from.saturating_sub(CONTEXT);
        // Where matches may start, relative to `base`.
        let mut at = (from - base) as usize;
        loop {
            self.step(base)?;
            let buf = self.read(base, self.block)?;
            let eof = buf.len() < self.block;
            // Up to the last line end, like `is_match_in`; a block without
            // one is searched as a piece.
            let cut = match buf.iter().rposition(|&b| b == b'\n') {
                Some(i) if !eof => i + 1,
                _ => buf.len(),
            };
            if let Some(m) = self.query.find_at(&buf[..cut], at) {
                return Ok(Some(base + m.start as u64..base + m.end as u64));
            }
            if eof {
                self.progress
                    .store(base + buf.len() as u64, Ordering::Relaxed);
                return Ok(None);
            }
            at = at.saturating_sub(cut);
            base += cut as u64;
        }
    }

    fn backward(&self, from: u64) -> io::Result<Option<Range<u64>>> {
        // A match starting before `from` may end after it, up to its line's
        // end (or a block on, in a giant line).
        let tail = self.read(from, self.block)?;
        let mut hi = from + tail.iter().position(|&b| b == b'\n').unwrap_or(tail.len()) as u64;
        // Matches must start before `limit`.
        let mut limit = from;
        while limit > 0 {
            self.step(limit)?;
            let raw = limit.saturating_sub(self.block as u64);
            let buf = self.read(raw, (hi - raw) as usize)?;
            let before = &buf[..((limit - raw) as usize).min(buf.len())];
            // Start after the first line end, keeping the `\n` as context for
            // `^`; without one (a giant line) the block is a piece.
            let (start, mut at) = match before.iter().position(|&b| b == b'\n') {
                Some(i) if raw > 0 && i + 1 < before.len() => (i, 1),
                _ => (0, 0),
            };
            let hay = &buf[start..];
            let base = raw + start as u64;
            // The next window ends where this one's matches may start.
            let next = base + at as u64;
            let mut last = None;
            while let Some(m) = self.query.find_at(hay, at) {
                if base + m.start as u64 >= limit {
                    break;
                }
                at = m.start + 1;
                last = Some(m);
            }
            if let Some(m) = last {
                return Ok(Some(base + m.start as u64..base + m.end as u64));
            }
            hi = next;
            limit = next;
        }
        self.progress.store(0, Ordering::Relaxed);
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::text::Text;

    fn text(pattern: &str) -> Text {
        Text {
            pattern: pattern.into(),
            ..Text::default()
        }
    }

    fn re(pattern: &str) -> Text {
        Text {
            regex: true,
            ..text(pattern)
        }
    }

    fn run(
        block: usize,
        t: &Text,
        bytes: &[u8],
        from: u64,
        direction: Direction,
    ) -> Option<Range<u64>> {
        let q = t.compile().unwrap();
        let progress = AtomicU64::new(0);
        find_with(
            block,
            &bytes.to_vec(),
            &q,
            from,
            direction,
            &AtomicBool::new(false),
            &progress,
        )
        .unwrap()
    }

    fn next(t: &Text, bytes: &str, from: u64) -> Option<Range<u64>> {
        run(BLOCK, t, bytes.as_bytes(), from, Direction::Forward)
    }

    fn prev(t: &Text, bytes: &str, from: u64) -> Option<Range<u64>> {
        run(BLOCK, t, bytes.as_bytes(), from, Direction::Backward)
    }

    #[test]
    fn forward_finds_the_first_match_from_a_position() {
        let hay = "one two\none two";
        assert_eq!(next(&text("two"), hay, 0), Some(4..7));
        assert_eq!(next(&text("two"), hay, 5), Some(12..15));
        assert_eq!(next(&text("two"), hay, 13), None);
        assert_eq!(next(&text("aa"), "aaa", 1), Some(1..3), "overlapping");
        assert_eq!(next(&text("x"), "", 0), None);
    }

    #[test]
    fn forward_sees_the_character_before_it() {
        let words = Text {
            whole_words: true,
            ..text("foo")
        };
        assert_eq!(next(&words, "xfoo foo", 1), Some(5..8));
        assert_eq!(next(&re("^b"), "ab\nb", 1), Some(3..4));
    }

    #[test]
    fn backward_finds_the_last_match_before_a_position() {
        let hay = "one two\none two";
        assert_eq!(prev(&text("two"), hay, 15), Some(12..15));
        assert_eq!(prev(&text("two"), hay, 12), Some(4..7));
        assert_eq!(prev(&text("two"), hay, 4), None);
        assert_eq!(prev(&text("aa"), "aaa", 1), Some(0..2));
        assert_eq!(prev(&text("aa"), "aaa", 2), Some(1..3), "overlapping");
        assert_eq!(prev(&text("x"), "x", 0), None);
    }

    #[test]
    fn backward_sees_a_match_running_past_the_position() {
        // The end of the screen can cut a match in two.
        assert_eq!(prev(&text("needle"), "a needle b\n", 4), Some(2..8));
        assert_eq!(prev(&re("^b"), "ab\nb", 4), Some(3..4));
    }

    /// Every start a search can stop at: the leftmost match from each byte.
    fn oracle(t: &Text, bytes: &[u8]) -> Vec<Range<u64>> {
        let q = t.compile().unwrap();
        let mut found: Vec<Range<u64>> = (0..=bytes.len())
            .filter_map(|at| q.find_at(bytes, at))
            .map(|m| m.start as u64..m.end as u64)
            .collect();
        found.dedup();
        found
    }

    #[test]
    fn small_blocks_agree_with_searching_the_whole_file() {
        let bytes = b"cat dog\ncatalog cat\n\ncat\nconcat cat dog cat\nx cat".to_vec();
        for t in [
            text("cat"),
            Text {
                whole_words: true,
                ..text("cat")
            },
            re("^c"),
            re(r"cat\s*$"),
            re("t d"),
        ] {
            let all = oracle(&t, &bytes);
            // Blocks larger than every line, smaller than the file.
            for block in [24, 32, 64] {
                for from in 0..=bytes.len() as u64 {
                    let want = all.iter().find(|m| m.start >= from).cloned();
                    let got = run(block, &t, &bytes, from, Direction::Forward);
                    assert_eq!(got, want, "{t:?} forward from {from} in blocks of {block}");
                    let want = all.iter().rev().find(|m| m.start < from).cloned();
                    let got = run(block, &t, &bytes, from, Direction::Backward);
                    assert_eq!(got, want, "{t:?} backward from {from} in blocks of {block}");
                }
            }
        }
    }

    #[test]
    fn a_line_longer_than_a_block_is_searched_in_pieces() {
        let mut bytes = vec![b'x'; 40];
        bytes.extend_from_slice(b"needle");
        bytes.extend(vec![b'x'; 40]);
        let t = text("needle");
        assert_eq!(run(16, &t, &bytes, 0, Direction::Forward), Some(40..46));
        assert_eq!(
            run(16, &t, &bytes, bytes.len() as u64, Direction::Backward),
            Some(40..46)
        );
        assert_eq!(run(16, &t, &bytes, 41, Direction::Forward), None);
        assert_eq!(run(16, &t, &bytes, 40, Direction::Backward), None);
    }

    #[test]
    fn forward_from_the_start_finds_what_alt_f7_finds() {
        // A match right after a block's last line end, and one in a line
        // that crosses a block edge.
        let mut bytes = vec![b'\n'; BLOCK - 3];
        bytes.extend_from_slice(b"needle\n");
        let q = text("needle").compile().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, &bytes).unwrap();
        assert!(q.is_match_in(&path, &AtomicBool::new(false)).unwrap());
        let start = (BLOCK - 3) as u64;
        assert_eq!(
            run(BLOCK, &text("needle"), &bytes, 0, Direction::Forward),
            Some(start..start + 6)
        );
    }

    #[test]
    fn cancel_stops_and_progress_reports_the_position() {
        let q = text("absent").compile().unwrap();
        let bytes = b"a\nb\nc\nd\n".to_vec();
        let progress = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let r = find_with(4, &bytes, &q, 0, Direction::Forward, &cancel, &progress);
        assert_eq!(r.unwrap(), None);
        assert_eq!(progress.load(Ordering::Relaxed), 8);
        let r = find_with(4, &bytes, &q, 8, Direction::Backward, &cancel, &progress);
        assert_eq!(r.unwrap(), None);
        assert_eq!(progress.load(Ordering::Relaxed), 0);
        cancel.store(true, Ordering::Relaxed);
        for direction in [Direction::Forward, Direction::Backward] {
            let e = find(&bytes, &q, 4, direction, &cancel, &progress).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        }
    }

    struct Failing;

    impl Source for Failing {
        fn len(&self) -> u64 {
            10
        }

        fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<usize> {
            Err(io::Error::other("broken"))
        }
    }

    #[test]
    fn read_errors_are_returned() {
        let q = text("x").compile().unwrap();
        let (cancel, progress) = (AtomicBool::new(false), AtomicU64::new(0));
        for direction in [Direction::Forward, Direction::Backward] {
            assert!(find(&Failing, &q, 5, direction, &cancel, &progress).is_err());
        }
    }
}
