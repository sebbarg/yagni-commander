//! The panel's column layout, shared so every frontend shows the same table.

use crate::entry::{Entry, EntryKind};
use crate::format::{format_modified, format_permissions, format_size};
use crate::sort::{Sort, SortKey};

pub struct Column {
    pub key: SortKey,
    pub title: &'static str,
    /// Width in logical pixels, or `None` to take the remaining space.
    pub width: Option<f32>,
    pub align_right: bool,
}

pub const COLUMNS: [Column; 5] = [
    Column {
        key: SortKey::Name,
        title: "Name",
        width: None,
        align_right: false,
    },
    Column {
        key: SortKey::Size,
        title: "Size",
        width: Some(70.0),
        align_right: true,
    },
    Column {
        key: SortKey::Modified,
        title: "Modified",
        width: Some(118.0),
        align_right: false,
    },
    Column {
        key: SortKey::Owner,
        title: "Owner",
        width: Some(100.0),
        align_right: false,
    },
    Column {
        key: SortKey::Permissions,
        title: "Permissions",
        width: Some(78.0),
        align_right: false,
    },
];

impl Column {
    /// Header text, with an arrow on the column the panel is sorted by.
    pub fn header(&self, sort: Sort) -> String {
        match (sort.key == self.key, sort.descending) {
            (false, _) => self.title.to_owned(),
            (true, false) => format!("{} ▲", self.title),
            (true, true) => format!("{} ▼", self.title),
        }
    }

    pub fn cell(&self, entry: &Entry) -> String {
        match self.key {
            SortKey::Name => entry.label.clone(),
            SortKey::Size => match entry.kind {
                EntryKind::Parent => String::new(),
                EntryKind::Dir => "<DIR>".into(),
                EntryKind::File => entry.size.map(format_size).unwrap_or_default(),
            },
            SortKey::Modified => entry.modified.map(format_modified).unwrap_or_default(),
            SortKey::Owner => entry.owner.as_deref().unwrap_or_default().to_owned(),
            SortKey::Permissions => format_permissions(entry),
        }
    }
}
