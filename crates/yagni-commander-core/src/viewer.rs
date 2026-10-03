//! The F3 viewer's byte work: reading huge files by position, splitting them
//! into display rows, and counting lines in the background. No gpui here.

mod copy;
mod document;
mod hex;
mod layout;
mod lines;
mod search;
mod source;

pub use copy::{COPY_CAP, Copy, TooLarge};
pub use document::Document;
pub use hex::{HEX_ROW, offset_digits};
pub use layout::{BLOCK, HexColumn, Highlight, MAX_ROW_CHARS, ROW_WINDOW, Row, Visible, Wrap};
pub use lines::{LineIndex, count_lines};
pub use search::{Direction, find};
pub use source::{FileSource, Source};
