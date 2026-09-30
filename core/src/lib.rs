//! UI-agnostic file manager core: directory listing, panel state and commands.
//!
//! Frontends translate input into [`Command`]s, call [`Commander::execute`],
//! and render the resulting state. Nothing in here knows about pixels or keys.

mod commander;
mod entry;
mod panel;
pub mod theme;
mod viewport;

pub use commander::{Command, Commander, Outcome, Side};
pub use entry::{Entry, EntryKind, format_size, read_entries};
pub use panel::{Activation, Panel};
pub use viewport::scroll_offset;
