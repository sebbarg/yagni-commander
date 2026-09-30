//! UI-agnostic file manager core: directory listing, panel state and commands.
//!
//! The UI translates input into [`Command`]s, calls [`Commander::execute`],
//! and renders the resulting state. Nothing in here knows about pixels or keys.

mod commander;
mod config;
mod entry;
mod format;
mod panel;
mod sort;
pub mod storage;

pub use commander::{Command, Commander, Outcome, Side};
pub use config::Config;
pub use entry::{Entry, EntryKind};
pub use format::{format_modified, format_permissions, format_size};
pub use panel::Panel;
pub use sort::{Sort, SortKey};
