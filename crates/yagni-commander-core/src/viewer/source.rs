//! Where the viewer's bytes come from: positional reads, so any part of a
//! huge file can be read without the rest.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

use nix::fcntl::OFlag;

pub trait Source {
    /// Length when opened. Reads may find less if the file shrank since.
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Length now, to notice a file that shrank (a cheap `fstat`).
    fn current_len(&self) -> io::Result<u64> {
        Ok(self.len())
    }
    /// Reads up to `buf.len()` bytes at `pos`; fewer (or 0) at the end.
    fn read_at(&self, buf: &mut [u8], pos: u64) -> io::Result<usize>;
}

/// For tests.
impl Source for Vec<u8> {
    fn len(&self) -> u64 {
        self.len() as u64
    }

    fn read_at(&self, buf: &mut [u8], pos: u64) -> io::Result<usize> {
        let start = (pos as usize).min(self.len());
        let n = buf.len().min(self.len() - start);
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }
}

/// A regular file opened for viewing.
pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    /// Opens `path` for reading. Anything but a regular file is refused:
    /// reading a FIFO or a device would block or never end, and merely
    /// opening some devices has effects (a serial line toggles DTR, a
    /// watchdog arms). So the type is checked before opening, and again on
    /// the opened file in case it was swapped in between; `O_NONBLOCK` and
    /// `O_NOCTTY` keep that window harmless. Regular files ignore both flags.
    pub fn open(path: &Path) -> io::Result<Self> {
        if !std::fs::metadata(path)?.is_file() {
            return Err(not_regular());
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOCTTY).bits())
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(not_regular());
        }
        Ok(Self {
            file,
            len: meta.len(),
        })
    }

    /// A second handle on the same file, for the background line count.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            len: self.len,
        })
    }
}

fn not_regular() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not a regular file")
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn current_len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn read_at(&self, buf: &mut [u8], pos: u64) -> io::Result<usize> {
        self.file.read_at(buf, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::{Document, Wrap};

    #[test]
    fn reads_a_file_by_position() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, b"hello\nworld\n").unwrap();
        let source = FileSource::open(&path).unwrap();
        assert_eq!(source.len(), 12);
        let mut buf = [0; 5];
        assert_eq!(source.read_at(&mut buf, 6).unwrap(), 5);
        assert_eq!(&buf, b"world");
        let clone = source.try_clone().unwrap();
        assert_eq!(clone.len(), 12);
    }

    #[test]
    fn directories_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let err = FileSource::open(tmp.path()).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn fifo_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pipe");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).unwrap();
        // Without O_NONBLOCK this open would wait for a writer forever.
        let err = FileSource::open(&path).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn shrinking_file_clamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        let text: String = (0..1000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, &text).unwrap();
        let mut doc = Document::new(FileSource::open(&path).unwrap());
        std::fs::write(&path, b"short\n").unwrap();
        let top = doc.last_top(5, Wrap::Off);
        assert_eq!(doc.len(), 6);
        assert_eq!(top, 0);
        assert_eq!(doc.rows(0, 10, Wrap::Off).len(), 1);
        assert!(doc.rows(4000, 10, Wrap::Off).is_empty());
    }

    #[test]
    fn shrinking_giant_line_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        std::fs::write(&path, vec![b'x'; 300_000]).unwrap();
        for cut in [0, 1000, 64 * 1024, 128 * 1024, 140 * 1024] {
            let mut doc = Document::new(FileSource::open(&path).unwrap());
            let wrap = Wrap::Columns(120);
            let top = doc.row_start_at(178_000, wrap);
            doc.rows(top, 50, wrap);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(cut)
                .unwrap();
            let last = doc.last_top(50, wrap);
            assert!(last <= cut, "cut {cut}: {last}");
            std::fs::write(&path, vec![b'x'; 300_000]).unwrap();
        }
    }

    #[test]
    fn scrolling_down_after_a_shrink_clamps() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f");
        let text: String = (0..1000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&path, &text).unwrap();
        let mut doc = Document::new(FileSource::open(&path).unwrap());
        let top = doc.last_top(5, Wrap::Off);
        std::fs::write(&path, b"a\nb\nc\n").unwrap();
        // The old top is past the new end: the next scroll shows the end.
        assert_eq!(doc.scroll_down(top, 1, 5, Wrap::Off), 0);
        assert_eq!(doc.rows(0, 5, Wrap::Off).len(), 3);
    }

    #[test]
    fn opening_a_fifo_does_not_open_its_read_end() {
        // Opening the read end would let a blocked writer through: the
        // refusal must come from a stat before any open.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pipe");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(path))
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(FileSource::open(&path).is_err());
        std::thread::sleep(std::time::Duration::from_millis(100));
        let opened = writer.is_finished();
        // Release the writer either way.
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&path);
        writer.join().unwrap().unwrap();
        assert!(!opened, "FileSource::open opened the FIFO");
    }
}
