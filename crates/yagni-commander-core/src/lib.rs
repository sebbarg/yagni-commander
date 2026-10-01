//! UI-agnostic file manager core: directory listing, panel state and commands.
//!
//! The UI translates input into [`Command`]s, calls [`Commander::execute`],
//! and renders the resulting state. Nothing in here knows about pixels or keys.

mod commander;
pub mod config;
mod entry;
pub mod file_ops;
mod format;
mod fs_ops;
pub mod launch;
mod listing;
pub mod oplog;
mod panel;
mod quick_search;
mod sort;
pub mod storage;
pub mod viewer;

pub use commander::{Command, Commander, Outcome, Side};
pub use config::{Config, Setting};
pub use entry::{Entry, EntryKind};
pub use format::{format_modified, format_permissions, format_size};
pub use listing::{Listing, LoadRequest, read_listing};
pub use panel::{Loading, Panel, Summary};
pub use sort::{Sort, SortKey};
