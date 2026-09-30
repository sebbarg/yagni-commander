//! UI-agnostic file manager core: directory listing, panel state and commands.
//!
//! Frontends translate input into [`Command`]s, call [`Commander::execute`],
//! and render the resulting state. Nothing in here knows about pixels or keys.

mod columns;
mod commander;
mod entry;
mod format;
mod panel;
mod sort;
pub mod theme;
mod viewport;

pub use columns::{COLUMNS, Column};
pub use commander::{Command, Commander, Outcome, Side};
pub use entry::{Entry, EntryKind, read_entries};
pub use format::{format_modified, format_permissions, format_size};
pub use panel::{Activation, Panel};
pub use sort::{Sort, SortKey, sort_entries};
pub use viewport::scroll_offset;
