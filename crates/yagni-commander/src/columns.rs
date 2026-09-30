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

#[cfg(test)]
mod tests {
    use super::*;
    use yagni_commander_core::Commander;

    fn column(key: SortKey) -> &'static Column {
        COLUMNS.iter().find(|c| c.key == key).unwrap()
    }

    /// Real entries from a directory with a dir `d` and a 5-byte file `f`.
    fn entries() -> (tempfile::TempDir, Vec<Entry>) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        std::fs::write(tmp.path().join("f"), b"12345").unwrap();
        let entries = Commander::new(tmp.path(), tmp.path())
            .unwrap()
            .panel(yagni_commander_core::Side::Left)
            .entries()
            .to_vec();
        (tmp, entries)
    }

    #[test]
    fn name_is_the_only_flexible_column_and_comes_first() {
        assert_eq!(COLUMNS[0].key, SortKey::Name);
        assert!(COLUMNS[0].width.is_none());
        assert!(COLUMNS[1..].iter().all(|c| c.width.is_some()));
    }

    #[test]
    fn header_marks_only_the_sorted_column() {
        let sort = Sort::default().toggled(SortKey::Size);
        assert_eq!(column(SortKey::Size).header(sort), "Size ▼");
        assert_eq!(
            column(SortKey::Size).header(sort.toggled(SortKey::Size)),
            "Size ▲"
        );
        assert_eq!(column(SortKey::Name).header(sort), "Name");
        assert_eq!(column(SortKey::Name).header(Sort::default()), "Name ▲");
    }

    #[test]
    fn size_text_depends_on_entry_kind() {
        let (_tmp, entries) = entries();
        let size = column(SortKey::Size);
        let texts: Vec<_> = entries.iter().map(|e| size.text(e)).collect();
        assert_eq!(texts, ["", "<DIR>", "5 B"]);
    }

    #[test]
    fn detail_columns_are_filled_for_real_entries_and_empty_for_parent() {
        let (_tmp, entries) = entries();
        let (parent, file) = (&entries[0], &entries[2]);
        for key in [SortKey::Modified, SortKey::Owner, SortKey::Permissions] {
            assert_eq!(column(key).text(parent), "", "{key:?} for ..");
            assert!(!column(key).text(file).is_empty(), "{key:?} for file");
        }
        assert_eq!(column(SortKey::Name).text(file), "f");
        assert_eq!(column(SortKey::Permissions).text(file).len(), 10);
    }
}
