//! The F3 viewer's byte work: reading huge files by position, splitting them
//! into display rows, and counting lines in the background. No gpui here.

mod document;
mod layout;
mod lines;
mod source;

pub use document::Document;
pub use layout::{BLOCK, MAX_ROW_CHARS, ROW_WINDOW, Row, Wrap};
pub use lines::{LineIndex, count_lines};
pub use source::{FileSource, Source};
