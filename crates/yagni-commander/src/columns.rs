//! The panel's table layout: which columns exist, their size, and cell text.

use std::sync::Arc;

use crate::zoom::{rems_from_px, scaled};
use gpui_kit::component::ActiveTheme;
use gpui_kit::{App, Div, Font, FontFeatures, Global, TextRun, Window, div, font, prelude::*, px};
use yagni_commander_core::{
    Commander, Entry, EntryKind, Sort, SortKey, format_modified, format_permissions, format_size,
};

pub struct Column {
    pub key: SortKey,
    pub title: &'static str,
    /// Width in logical pixels at the 16 px base, or `None` to take the
    /// remaining space. The least it gets: `measure` widens it to fit its
    /// `samples` in the UI font in use.
    pub width: Option<f32>,
    /// The widest cell texts, measured at `CELL_TEXT` px.
    pub samples: &'static [&'static str],
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
        samples: &[],
        align_right: false,
        tabular_figures: false,
    },
    Column {
        key: SortKey::Size,
        title: "Size",
        width: Some(70.0),
        // `format_size` rounds to one decimal, so 1023.95 shows as 1024.0.
        samples: &["1024.0 MiB", "<DIR>"],
        align_right: true,
        tabular_figures: true,
    },
    Column {
        key: SortKey::Modified,
        title: "Modified",
        width: Some(118.0),
        // Every digit, in case the font has no tabular figures.
        samples: &["0000-00-00 00:00", "8888-88-88 88:88"],
        align_right: false,
        tabular_figures: true,
    },
    Column {
        key: SortKey::Owner,
        title: "Owner",
        // Without a measurement (app tests); else sized to the listing's
        // owners, `Widths::with_owners`.
        width: Some(100.0),
        samples: &[],
        align_right: false,
        tabular_figures: false,
    },
    Column {
        key: SortKey::Permissions,
        title: "Permissions",
        width: Some(78.0),
        samples: &["drwxrwxrwx"],
        align_right: false,
        tabular_figures: false,
    },
];

/// Space between two cells, in px at the 16 px base.
pub const CELL_SPACING: f32 = 10.0;

/// Text sizes of the detail cells and of the column headers, in px at the
/// 16 px base.
pub const CELL_TEXT: f32 = 13.0;
pub const HEADER_TEXT: f32 = 12.0;

/// The widest Owner gets, in px at the 16 px base; longer names truncate.
pub const OWNER_MAX_WIDTH: f32 = 160.0;

/// The fixed columns' widths in px at the 16 px base, in `COLUMNS` order:
/// each column's `width`, widened by `measure` to fit its samples and its
/// header (with the sort arrow) in the UI font. Fixed widths fitted the fonts
/// tried; a wider one (Omarchy) cut "2026-10-04 19:34" off at every zoom
/// level, since zoom scales text and width alike. Owner is sized per panel
/// to the owners listed (`with_owners`), so `seb:seb` leaves no gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    fixed: [f32; COLUMNS.len()],
    /// The Owner header's width, once measured; `None` keeps Owner fixed.
    owner_least: Option<f32>,
}

impl Global for Widths {}

impl Default for Widths {
    fn default() -> Self {
        Self {
            fixed: COLUMNS.map(|c| c.width.unwrap_or(0.0)),
            owner_least: None,
        }
    }
}

impl Widths {
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }

    /// The width of `column`, `None` for Name.
    pub fn of(&self, column: &Column) -> Option<f32> {
        let ix = COLUMNS.iter().position(|c| c.key == column.key)?;
        column.width.map(|_| self.fixed[ix])
    }

    /// Owner as wide as the widest of `owners` (`width_of` measures one at
    /// `CELL_TEXT` px), at least its header, at most `OWNER_MAX_WIDTH`.
    pub fn with_owners(mut self, owners: &[String], width_of: impl Fn(&str) -> f32) -> Self {
        let Some(least) = self.owner_least else {
            return self;
        };
        let widest = owners.iter().map(|o| width_of(o)).fold(0.0, f32::max);
        let ix = owner_index();
        self.fixed[ix] = (widest.ceil() + 1.0).clamp(least, OWNER_MAX_WIDTH);
        self
    }

    /// Widens each fixed column to `measured` (its widest sample or header
    /// text, in px at the base), plus a pixel against rounding.
    fn fitted(measured: impl Fn(&Column) -> f32) -> Self {
        let mut widths = Self::default();
        for (ix, column) in COLUMNS.iter().enumerate() {
            if let Some(least) = column.width {
                widths.fixed[ix] = least.max(measured(column).ceil() + 1.0);
            }
        }
        let header = measured(&COLUMNS[owner_index()]);
        widths.owner_least = (header > 0.0).then(|| header.ceil() + 1.0);
        widths
    }
}

fn owner_index() -> usize {
    COLUMNS
        .iter()
        .position(|c| c.key == SortKey::Owner)
        .unwrap()
}

/// The width of `text` in the UI font at `size` px.
pub fn text_width(window: &Window, cx: &App, text: &str, size: f32, features: FontFeatures) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: Font {
            features,
            ..font(cx.theme().font_family.clone())
        },
        color: crate::theme::Theme::get(cx).colors.text.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.to_owned().into(), px(size), &[run], None);
    f32::from(line.width)
}

/// Measures the columns with the UI font, once per run (the font doesn't
/// change, and zoom scales widths with the text). The text system of app
/// tests measures nothing, so they keep the fixed widths.
pub fn measure(window: &Window, cx: &mut App) {
    let width_of = |text: String, size: f32, features: FontFeatures| {
        text_width(window, cx, &text, size, features)
    };
    let widths = Widths::fitted(|column| {
        let features = if column.tabular_figures {
            tabular_figures()
        } else {
            FontFeatures::default()
        };
        let cells = column
            .samples
            .iter()
            .map(|s| width_of(s.to_string(), CELL_TEXT, features.clone()));
        let header = width_of(
            format!("{} ▼", column.title),
            HEADER_TEXT,
            Default::default(),
        );
        cells.fold(header, f32::max)
    });
    log::info!("column widths: {widths:?}");
    cx.set_global(widths);
}

/// The narrowest Name gets before optional columns make way for it.
pub const NAME_MIN_WIDTH: f32 = 120.0;

/// The columns the panels show: Name and Size, then the optional ones the
/// config turns on (see `Commander::hide_columns`), as many as fit.
/// `width` is the cells' room in a row, in px at UI level `ui`.
pub fn visible_columns(
    commander: &Commander,
    width: f32,
    ui: f32,
    widths: &Widths,
) -> Vec<&'static Column> {
    fit(
        COLUMNS
            .iter()
            .filter(|c| commander.shows_column(c.key))
            .collect(),
        width,
        ui,
        widths,
    )
}

/// Drops optional columns from the end until Name keeps `NAME_MIN_WIDTH`.
/// Name and Size always stay, even if Name gets narrower.
fn fit(
    mut columns: Vec<&'static Column>,
    width: f32,
    ui: f32,
    widths: &Widths,
) -> Vec<&'static Column> {
    let needed = |columns: &[&Column]| {
        let fixed: f32 = columns.iter().filter_map(|c| widths.of(c)).sum();
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
    pub fn cell(&self, widths: &Widths) -> Div {
        let cell = match widths.of(self) {
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
        assert_eq!(keys(&fit(all(), full, 16.0, &Widths::default())).len(), 5);
        assert_eq!(
            keys(&fit(all(), full - 1.0, 16.0, &Widths::default())),
            [Name, Size, Modified, Owner]
        );
        assert_eq!(
            keys(&fit(
                all(),
                room(&[Size, Modified]),
                16.0,
                &Widths::default()
            )),
            [Name, Size, Modified]
        );
        // Name and Size stay however narrow the panel gets.
        assert_eq!(
            keys(&fit(all(), 0.0, 16.0, &Widths::default())),
            [Name, Size]
        );
    }

    #[test]
    fn fitting_counts_at_the_ui_level() {
        use SortKey::*;
        let all = || COLUMNS.iter().collect::<Vec<_>>();
        let full = room(&[Size, Modified, Owner, Permissions]);
        assert_eq!(
            keys(&fit(all(), full * 2.0, 32.0, &Widths::default())).len(),
            5
        );
        assert_eq!(
            keys(&fit(all(), full * 2.0 - 1.0, 32.0, &Widths::default())).len(),
            4
        );
    }

    #[test]
    fn measured_text_widens_a_column_but_never_narrows_it() {
        let widths = Widths::fitted(|c| match c.key {
            SortKey::Modified => 130.2,
            _ => 10.0,
        });
        assert_eq!(widths.of(column(SortKey::Modified)), Some(132.0));
        assert_eq!(widths.of(column(SortKey::Size)), Some(70.0));
        assert_eq!(widths.of(column(SortKey::Permissions)), Some(78.0));
        assert_eq!(widths.of(column(SortKey::Name)), None);
        assert_eq!(Widths::fitted(|_| 0.0), Widths::default());
    }

    #[test]
    fn owner_fits_the_owners_listed_between_its_header_and_the_cap() {
        let measured = Widths::fitted(|c| if c.key == SortKey::Owner { 40.0 } else { 0.0 });
        let owner = column(SortKey::Owner);
        let width = |owners: &[&str]| {
            let owners: Vec<String> = owners.iter().map(|o| o.to_string()).collect();
            measured
                .with_owners(&owners, |o| o.len() as f32 * 7.0)
                .of(owner)
                .unwrap()
        };
        assert_eq!(width(&["seb:seb"]), 50.0);
        assert_eq!(width(&["seb:seb", "sebastian:staff"]), 106.0);
        assert_eq!(width(&[]), 41.0, "at least the header");
        assert_eq!(width(&["a"]), 41.0);
        assert_eq!(
            width(&["a-very-long-user:a-very-long-group"]),
            OWNER_MAX_WIDTH
        );
        // Not measured (app tests): the fixed width.
        let fixed = Widths::default().with_owners(&["seb:seb".into()], |_| 49.0);
        assert_eq!(fixed.of(owner), Some(100.0));
    }

    #[test]
    fn fitting_counts_the_measured_widths() {
        use SortKey::*;
        let all = || COLUMNS.iter().collect::<Vec<_>>();
        let full = room(&[Size, Modified, Owner, Permissions]);
        let wider = Widths::fitted(|c| if c.key == Modified { 200.0 } else { 0.0 });
        assert_eq!(keys(&fit(all(), full, 16.0, &wider)).len(), 4);
    }

    #[test]
    fn a_column_turned_off_frees_its_room() {
        use SortKey::*;
        let some = COLUMNS.iter().filter(|c| c.key != Owner).collect();
        assert_eq!(
            keys(&fit(
                some,
                room(&[Size, Modified, Permissions]),
                16.0,
                &Widths::default()
            )),
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
