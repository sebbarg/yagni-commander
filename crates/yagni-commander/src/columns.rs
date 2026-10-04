//! The panel's table layout: which columns exist, their size, and cell text.

use std::sync::Arc;

use crate::zoom::{rems_from_px, scaled};
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

/// Space between two cells, in px at the 16 px base.
pub const CELL_SPACING: f32 = 10.0;

/// The narrowest Name gets before optional columns make way for it.
pub const NAME_MIN_WIDTH: f32 = 120.0;

/// The columns the panels show: Name and Size, then the optional ones the
/// config turns on (see `Commander::hide_columns`), as many as fit.
/// `width` is the cells' room in a row, in px at UI level `ui`.
pub fn visible_columns(commander: &Commander, width: f32, ui: f32) -> Vec<&'static Column> {
    fit(
        COLUMNS
            .iter()
            .filter(|c| commander.shows_column(c.key))
            .collect(),
        width,
        ui,
    )
}

/// Drops optional columns from the end until Name keeps `NAME_MIN_WIDTH`.
/// Name and Size always stay, even if Name gets narrower.
fn fit(mut columns: Vec<&'static Column>, width: f32, ui: f32) -> Vec<&'static Column> {
    let needed = |columns: &[&Column]| {
        let fixed: f32 = columns.iter().filter_map(|c| c.width).sum();
        let gaps = columns.len().saturating_sub(1) as f32 * CELL_SPACING;
        scaled(NAME_MIN_WIDTH + fixed + gaps, ui)
    };
    while columns.len() > 2 && needed(&columns) > width {
        columns.pop();
    }
    columns
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

    fn keys(columns: &[&Column]) -> Vec<SortKey> {
        columns.iter().map(|c| c.key).collect()
    }

    /// Room for Name at its minimum plus `columns`, at the 16 px base.
    fn room(columns: &[SortKey]) -> f32 {
        let fixed: f32 = columns.iter().map(|&k| column(k).width.unwrap()).sum();
        NAME_MIN_WIDTH + fixed + columns.len() as f32 * CELL_SPACING
    }

    #[test]
    fn optional_columns_make_way_for_name_from_the_last() {
        use SortKey::*;
        let all = || COLUMNS.iter().collect::<Vec<_>>();
        let full = room(&[Size, Modified, Owner, Permissions]);
        assert_eq!(keys(&fit(all(), full, 16.0)).len(), 5);
        assert_eq!(
            keys(&fit(all(), full - 1.0, 16.0)),
            [Name, Size, Modified, Owner]
        );
        assert_eq!(
            keys(&fit(all(), room(&[Size, Modified]), 16.0)),
            [Name, Size, Modified]
        );
        // Name and Size stay however narrow the panel gets.
        assert_eq!(keys(&fit(all(), 0.0, 16.0)), [Name, Size]);
    }

    #[test]
    fn fitting_counts_at_the_ui_level() {
        use SortKey::*;
        let all = || COLUMNS.iter().collect::<Vec<_>>();
        let full = room(&[Size, Modified, Owner, Permissions]);
        assert_eq!(keys(&fit(all(), full * 2.0, 32.0)).len(), 5);
        assert_eq!(keys(&fit(all(), full * 2.0 - 1.0, 32.0)).len(), 4);
    }

    #[test]
    fn a_column_turned_off_frees_its_room() {
        use SortKey::*;
        let some = COLUMNS.iter().filter(|c| c.key != Owner).collect();
        assert_eq!(
            keys(&fit(some, room(&[Size, Modified, Permissions]), 16.0)),
            [Name, Size, Modified, Permissions]
        );
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
