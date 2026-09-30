//! The panel's table layout: which columns exist, their size, and cell text.

use gpui::{Div, div, prelude::*, px};
use yagni_commander_core::{
    Entry, EntryKind, Sort, SortKey, format_modified, format_permissions, format_size,
};

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

    pub fn text(&self, entry: &Entry) -> String {
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

    /// An empty cell sized and aligned for this column, shared by headers and rows.
    pub fn cell(&self) -> Div {
        let cell = match self.width {
            Some(width) => div().w(px(width)).flex_none(),
            None => div().flex_1().min_w_0(),
        };
        cell.truncate().when(self.align_right, |d| d.text_right())
    }
}
