//! UI-agnostic file manager core: directory listing, panel state and commands.
//!
//! The UI translates input into [`Command`]s, calls [`Commander::execute`],
//! and renders the resulting state. Nothing in here knows about pixels or keys.

pub mod archive;
mod commander;
pub mod config;
mod entry;
pub mod file_ops;
pub mod find;
mod format;
mod fs_ops;
pub mod hotlist;
pub mod icons;
pub mod info;
pub mod launch;
mod listing;
pub mod mounts;
pub mod oplog;
mod panel;
mod quick_search;
pub mod shell_path;
mod sort;
pub mod storage;
mod tabs;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_archives;
pub mod update;
pub mod viewer;
pub mod watch;

pub use archive::ArchiveIndex;
pub use commander::{Command, Commander, Outcome, Side, StartTabs};
pub use config::{Config, Setting};
pub use entry::{Entry, EntryKind};
pub use format::{format_count, format_modified, format_permissions, format_size};
pub use listing::{ArchiveRead, Listing, LoadRequest, dir_modified, read_listing};
pub use panel::{Loading, Panel, PanelId, Summary};
pub use sort::{Sort, SortKey};
pub use tabs::{Tabs, label};
