//! The panel's table layout: which columns exist, their size, and cell text.

use std::sync::Arc;

use crate::zoom::rems_from_px;
use gpui_kit::{Div, FontFeatures, div, prelude::*};
use yagni_commander_core::{
    Commander, Entry, EntryKind, Sort, SortKey, format_modified, format_permissions, format_size,
};

pub struct Column {
    pub key: SortKey,
    pub title: &'static str,
    /// Width in logical pixels, or `None` to take the remaining space.
    pub width: Option<f32>,
    pub align_right: bool,
    /// Digits all one width (the font's `tnum` feature), so numbers line up
    /// from row to row: the system font on macOS has proportional digits.
    pub tabular_figures: bool,
}

pub const COLUMNS: [Column; 5] = [
    Column {
        key: SortKey::Name,
        title: "Name",
        width: None,
        align_right: false,
        tabular_figures: false,
    },
    Column {
        key: SortKey::Size,
        title: "Size",
        width: Some(70.0),
        align_right: true,
        tabular_figures: true,
    },
    Column {
        key: SortKey::Modified,
        title: "Modified",
        width: Some(118.0),
        align_right: false,
        tabular_figures: true,
    },
    Column {
        key: SortKey::Owner,
        title: "Owner",
        width: Some(100.0),
        align_right: false,
        tabular_figures: false,
    },
    Column {
        key: SortKey::Permissions,
        title: "Permissions",
        width: Some(78.0),
        align_right: false,
        tabular_figures: false,
    },
];

/// The columns the panels show: Name and Size, then the optional ones the
/// config turns on (see `Commander::hide_columns`).
pub fn visible_columns(commander: &Commander) -> impl Iterator<Item = &'static Column> + '_ {
    COLUMNS.iter().filter(|c| commander.shows_column(c.key))
}

impl Column {
    /// Header text, with an arrow on the column the panel is sorted by.
    pub fn header(&self, sort: Sort) -> String {
        match (sort.key == self.key, sort.descending) {
            (false, _) => self.title.to_owned(),
            (true, false) => format!("{} ▲", self.title),
            (true, true) => format!("{} ▼", self.title),
        }
    }

    /// Cell text. Without icons, directories show as `[name]`; this is
    /// display only, so sorting and quick search still use the bare name.
    pub fn text(&self, entry: &Entry, icons: bool) -> String {
        match self.key {
            SortKey::Name => match entry.kind {
                EntryKind::Dir if !icons => format!("[{}]", entry.label),
                EntryKind::Parent | EntryKind::Dir | EntryKind::File => entry.label.clone(),
            },
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
            Some(width) => div().w(rems_from_px(width)).flex_none(),
            None => div().flex_1().min_w_0(),
        };
        cell.truncate()
            .when(self.align_right, |d| d.text_right())
            .when(self.tabular_figures, |d| d.font_features(tabular_figures()))
    }
}

/// OpenType tabular figures. A font without the feature draws as before.
pub fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(key: SortKey) -> &'static Column {
        COLUMNS.iter().find(|c| c.key == key).unwrap()
    }

    /// Real entries from a directory with a dir `d` and a 5-byte file `f`.
    fn entries() -> (tempfile::TempDir, Vec<Entry>) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        std::fs::write(tmp.path().join("f"), b"12345").unwrap();
        let entries = Commander::new(tmp.path(), tmp.path(), false)
            .unwrap()
            .panel(yagni_commander_core::Side::Left)
            .entries()
            .to_vec();
        (tmp, entries)
    }

    #[test]
    fn number_columns_use_tabular_figures() {
        let tabular: Vec<_> = COLUMNS
            .iter()
            .filter(|c| c.tabular_figures)
            .map(|c| c.key)
            .collect();
        assert_eq!(tabular, [SortKey::Size, SortKey::Modified]);
        assert_eq!(
            tabular_figures().tag_value_list(),
            [("tnum".to_string(), 1)]
        );
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
    fn directory_names_are_bracketed_only_without_icons() {
        let (_tmp, entries) = entries();
        let name = column(SortKey::Name);
        let texts = |icons| {
            entries
                .iter()
                .map(|e| name.text(e, icons))
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(false), ["..", "[d]", "f"]);
        assert_eq!(texts(true), ["..", "d", "f"]);
    }

    #[test]
    fn size_text_depends_on_entry_kind() {
        let (_tmp, entries) = entries();
        let size = column(SortKey::Size);
        let texts: Vec<_> = entries.iter().map(|e| size.text(e, false)).collect();
        assert_eq!(texts, ["", "<DIR>", "5 B"]);
    }

    #[test]
    fn detail_columns_are_filled_for_real_entries_and_empty_for_parent() {
        let (_tmp, entries) = entries();
        let (parent, file) = (&entries[0], &entries[2]);
        for key in [SortKey::Modified, SortKey::Owner, SortKey::Permissions] {
            assert_eq!(column(key).text(parent, false), "", "{key:?} for ..");
            assert!(
                !column(key).text(file, false).is_empty(),
                "{key:?} for file"
            );
        }
        assert_eq!(column(SortKey::Name).text(file, false), "f");
        assert_eq!(column(SortKey::Permissions).text(file, false).len(), 10);
    }
}
