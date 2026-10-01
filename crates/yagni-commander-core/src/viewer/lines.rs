//! Line numbers for the viewer. Counting needs the whole file, so it runs in
//! the background and keeps the count at every 1 MiB; the line of any
//! position is then that count plus a scan of at most 1 MiB.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use super::document::Document;
use super::source::Source;

const CHUNK: u64 = 1 << 20;

pub struct LineIndex {
    /// `\n` bytes before each 1 MiB boundary.
    newlines_before: Vec<u64>,
    lines: u64,
}

/// Counts the lines of `source`. Returns `None` once `cancel` is set (checked
/// per 1 MiB).
pub fn count_lines(source: &impl Source, cancel: &AtomicBool) -> io::Result<Option<LineIndex>> {
    let len = source.len();
    let mut buf = vec![0; CHUNK as usize];
    let mut newlines_before = Vec::new();
    let mut newlines = 0;
    let mut last = None;
    let mut pos = 0;
    while pos < len {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        newlines_before.push(newlines);
        let want = (len - pos).min(CHUNK) as usize;
        let mut got = 0;
        while got < want {
            match source.read_at(&mut buf[got..want], pos + got as u64) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        newlines += buf[..got].iter().filter(|&&b| b == b'\n').count() as u64;
        last = buf[..got].last().copied().or(last);
        if got < want {
            break; // shrank
        }
        pos += CHUNK;
    }
    let unterminated = last.is_some_and(|b| b != b'\n');
    Ok(Some(LineIndex {
        newlines_before,
        lines: newlines + u64::from(unterminated),
    }))
}

impl LineIndex {
    pub fn lines(&self) -> u64 {
        self.lines
    }

    /// The 1-based line containing byte `pos`.
    pub fn line_of<S: Source>(&self, doc: &mut Document<S>, pos: u64) -> u64 {
        let chunk = (pos / CHUNK) as usize;
        let Some(&before) = self.newlines_before.get(chunk) else {
            return self.lines.max(1);
        };
        before + doc.count_newlines(chunk as u64 * CHUNK, pos) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::Document;

    fn index(bytes: &[u8]) -> LineIndex {
        count_lines(&bytes.to_vec(), &AtomicBool::new(false))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn counts_lines_with_and_without_a_final_newline() {
        assert_eq!(index(b"").lines(), 0);
        assert_eq!(index(b"a").lines(), 1);
        assert_eq!(index(b"a\n").lines(), 1);
        assert_eq!(index(b"a\nb").lines(), 2);
        assert_eq!(index(b"\n\n").lines(), 2);
    }

    #[test]
    fn line_of_a_position_across_chunks() {
        // Lines of 10 bytes, spanning several 1 MiB chunks.
        let bytes: Vec<u8> = (0..300_000).flat_map(|_| *b"123456789\n").collect();
        let idx = index(&bytes);
        assert_eq!(idx.lines(), 300_000);
        let mut doc = Document::new(bytes);
        assert_eq!(idx.line_of(&mut doc, 0), 1);
        assert_eq!(idx.line_of(&mut doc, 15), 2);
        assert_eq!(idx.line_of(&mut doc, 2_999_990), 300_000);
    }

    #[test]
    fn cancelled_count_stops() {
        let bytes = vec![b'\n'; 3 << 20];
        assert!(
            count_lines(&bytes, &AtomicBool::new(true))
                .unwrap()
                .is_none()
        );
    }

    struct Failing;

    impl Source for Failing {
        fn len(&self) -> u64 {
            10
        }

        fn read_at(&self, _: &mut [u8], _: u64) -> io::Result<usize> {
            Err(io::Error::other("gone"))
        }
    }

    /// Claims a longer length than it has, like a file that shrank.
    struct Shrunk(Vec<u8>);

    impl Source for Shrunk {
        fn len(&self) -> u64 {
            3 << 20
        }

        fn read_at(&self, buf: &mut [u8], pos: u64) -> io::Result<usize> {
            self.0.read_at(buf, pos)
        }
    }

    #[test]
    fn read_errors_fail_the_count() {
        assert!(count_lines(&Failing, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn a_shrunk_file_counts_what_is_left() {
        let idx = count_lines(&Shrunk(b"a\nb".to_vec()), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(idx.lines(), 2);
        // Past the counted chunks: the last line.
        let mut doc = Document::new(b"a\nb".to_vec());
        assert_eq!(idx.line_of(&mut doc, 5 << 20), 2);
    }
}
