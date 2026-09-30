use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use crate::entry::{Entry, EntryKind, read_entries};
use crate::sort::{Sort, SortKey, sort_entries};

/// One side of the commander: a directory listing with a cursor.
///
/// Paths are kept logical (not canonicalized), so entering a symlinked
/// directory and then going to ".." returns to where you came from.
#[derive(Debug)]
pub struct Panel {
    path: PathBuf,
    entries: Vec<Entry>,
    cursor: usize,
    sort: Sort,
    /// Names of selected entries. Kept by name so it survives re-sorting.
    selection: HashSet<OsString>,
}

/// Counts and sizes for the panel footer. Directory sizes are unknown, so
/// byte totals cover files only.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub selected_dirs: usize,
    pub selected_files: usize,
    pub selected_bytes: u64,
}

impl Summary {
    pub fn selected(&self) -> usize {
        self.selected_dirs + self.selected_files
    }

    pub fn total(&self) -> usize {
        self.dirs + self.files
    }
}

/// What happened when the entry under the cursor was activated.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Activation {
    /// The panel now shows a different directory.
    Navigated,
    /// The cursor is on a file. The UI decides what to do with it.
    File(PathBuf),
    /// Nothing to activate (empty listing).
    None,
}

impl Panel {
    pub(crate) fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let sort = Sort::default();
        let mut entries = read_entries(&path)?;
        sort_entries(&mut entries, sort);
        Ok(Self {
            path,
            entries,
            cursor: 0,
            sort,
            selection: HashSet::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// Column header click: sort by `key`, or reverse if already sorted by it.
    /// The cursor stays on the same entry.
    pub(crate) fn sort_by(&mut self, key: SortKey) {
        self.resort(self.sort.toggled(key));
    }

    fn resort(&mut self, sort: Sort) {
        let keep = self.cursor_entry().map(|e| e.name.clone());
        self.sort = sort;
        sort_entries(&mut self.entries, self.sort);
        self.cursor = keep
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(0);
    }

    /// Switches between case-sensitive and -insensitive name sorting,
    /// keeping the cursor on the same entry.
    pub(crate) fn set_case_sensitive(&mut self, case_sensitive: bool) {
        self.resort(Sort {
            case_sensitive,
            ..self.sort
        });
    }

    pub fn is_selected(&self, entry: &Entry) -> bool {
        self.selection.contains(&entry.name)
    }

    /// Selected entries in display order.
    pub fn selection(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| self.is_selected(e))
    }

    /// What file operations act on: the selection, or else the entry under
    /// the cursor. Never "..".
    pub fn targets(&self) -> Vec<&Entry> {
        if self.selection.is_empty() {
            self.cursor_entry()
                .filter(|e| e.kind != EntryKind::Parent)
                .into_iter()
                .collect()
        } else {
            self.selection().collect()
        }
    }

    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for entry in &self.entries {
            let selected = self.is_selected(entry);
            let bytes = entry.size.unwrap_or(0);
            match entry.kind {
                EntryKind::Parent => {}
                EntryKind::Dir => {
                    summary.dirs += 1;
                    summary.selected_dirs += usize::from(selected);
                }
                EntryKind::File => {
                    summary.files += 1;
                    summary.bytes += bytes;
                    if selected {
                        summary.selected_files += 1;
                        summary.selected_bytes += bytes;
                    }
                }
            }
        }
        summary
    }

    /// Space: toggles selection of the entry under the cursor and moves the
    /// cursor down, so repeated presses select a run of entries. ".." can't be
    /// selected, but the cursor still moves past it.
    pub(crate) fn toggle_selection(&mut self) {
        if let Some(entry) = self.cursor_entry().filter(|e| e.kind != EntryKind::Parent) {
            let name = entry.name.clone();
            if !self.selection.remove(&name) {
                self.selection.insert(name);
            }
        }
        self.move_cursor(1);
    }

    /// Ctrl-A: selects every file and directory.
    pub(crate) fn select_all(&mut self) {
        self.selection = self
            .entries
            .iter()
            .filter(|e| e.kind != EntryKind::Parent)
            .map(|e| e.name.clone())
            .collect();
    }

    /// The entry under the cursor.
    pub fn cursor_entry(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub(crate) fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(self.cursor.saturating_add_signed(delta));
    }

    /// Moves the cursor to `index`, clamped to the listing.
    pub(crate) fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.entries.len().saturating_sub(1));
    }

    /// Enter: descend into a directory, go up on "..", or report a file.
    pub(crate) fn activate(&mut self) -> io::Result<Activation> {
        let Some(entry) = self.cursor_entry() else {
            return Ok(Activation::None);
        };
        match entry.kind {
            EntryKind::Parent => self.go_up().map(|_| Activation::Navigated),
            EntryKind::Dir => {
                let target = self.path.join(&entry.name);
                self.navigate(target, None).map(|_| Activation::Navigated)
            }
            EntryKind::File => Ok(Activation::File(self.path.join(&entry.name))),
        }
    }

    /// Goes to the parent directory and puts the cursor on the directory we left.
    /// Does nothing at the filesystem root.
    pub(crate) fn go_up(&mut self) -> io::Result<()> {
        let Some(parent) = self.path.parent().map(Path::to_path_buf) else {
            return Ok(());
        };
        let came_from = self.path.file_name().map(|n| n.to_os_string());
        self.navigate(parent, came_from.as_deref())
    }

    /// Loads `target` and only commits the change if reading succeeded, so a
    /// failed navigation leaves the panel untouched. Changing directory clears
    /// the selection; re-reading the same one keeps names that still exist.
    fn navigate(&mut self, target: PathBuf, select: Option<&OsStr>) -> io::Result<()> {
        let mut entries = read_entries(&target)?;
        sort_entries(&mut entries, self.sort);
        let cursor = select
            .and_then(|name| entries.iter().position(|e| e.name == name))
            .unwrap_or(0);
        if target == self.path {
            self.selection
                .retain(|name| entries.iter().any(|e| &e.name == name));
        } else {
            self.selection.clear();
        }
        self.path = target;
        self.entries = entries;
        self.cursor = cursor;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("alpha/inner")).unwrap();
        fs::create_dir(tmp.path().join("beta")).unwrap();
        fs::write(tmp.path().join("file.txt"), b"hello").unwrap();
        tmp
    }

    fn select(panel: &mut Panel, label: &str) {
        let ix = panel
            .entries()
            .iter()
            .position(|e| e.label == label)
            .unwrap();
        panel.set_cursor(ix);
    }

    #[test]
    fn cursor_is_clamped() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.move_cursor(-5);
        assert_eq!(panel.cursor(), 0);
        panel.move_cursor(100);
        assert_eq!(panel.cursor(), panel.entries().len() - 1);
    }

    #[test]
    fn enter_descends_and_parent_returns_to_previous_dir() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "beta");

        assert_eq!(panel.activate().unwrap(), Activation::Navigated);
        assert_eq!(panel.path(), tmp.path().join("beta"));
        assert_eq!(panel.cursor(), 0);
        assert_eq!(panel.cursor_entry().unwrap().kind, EntryKind::Parent);

        assert_eq!(panel.activate().unwrap(), Activation::Navigated);
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.cursor_entry().unwrap().label, "beta");
    }

    #[test]
    fn enter_on_file_reports_it_without_navigating() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "file.txt");
        assert_eq!(
            panel.activate().unwrap(),
            Activation::File(tmp.path().join("file.txt"))
        );
        assert_eq!(panel.path(), tmp.path());
    }

    #[test]
    fn failed_navigation_leaves_panel_unchanged() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "alpha");
        fs::remove_dir_all(tmp.path().join("alpha")).unwrap();

        assert!(panel.activate().is_err());
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.cursor_entry().unwrap().label, "alpha");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_dir_keeps_logical_path() {
        let tmp = fixture();
        std::os::unix::fs::symlink(tmp.path().join("alpha"), tmp.path().join("link")).unwrap();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "link");
        panel.activate().unwrap();
        assert_eq!(panel.path(), tmp.path().join("link"));
        panel.go_up().unwrap();
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.cursor_entry().unwrap().label, "link");
    }

    #[test]
    fn sorting_keeps_cursor_on_same_entry_and_survives_navigation() {
        let tmp = fixture();
        fs::write(tmp.path().join("beta/small"), b"1").unwrap();
        fs::write(tmp.path().join("beta/big"), b"12345").unwrap();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "beta");

        panel.sort_by(SortKey::Name);
        assert_eq!(panel.entries()[1].label, "beta");
        assert_eq!(panel.cursor_entry().unwrap().label, "beta");

        panel.sort_by(SortKey::Size);
        panel.activate().unwrap();
        let labels: Vec<_> = panel.entries().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["..", "big", "small"]);
    }

    #[test]
    fn go_up_at_root_is_a_no_op() {
        let mut panel = Panel::open("/").unwrap();
        panel.go_up().unwrap();
        assert_eq!(panel.path(), Path::new("/"));
    }

    #[test]
    fn open_fails_for_missing_or_non_directory_path() {
        let tmp = fixture();
        assert!(Panel::open(tmp.path().join("missing")).is_err());
        assert!(Panel::open(tmp.path().join("file.txt")).is_err());
    }

    #[test]
    fn open_makes_relative_paths_absolute() {
        let panel = Panel::open(".").unwrap();
        assert!(panel.path().is_absolute());
    }

    #[test]
    fn starts_sorted_by_name_with_cursor_on_first_entry() {
        let tmp = fixture();
        let panel = Panel::open(tmp.path()).unwrap();
        assert_eq!(panel.sort(), Sort::default());
        assert_eq!(panel.cursor(), 0);
        let labels: Vec<_> = panel.entries().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["..", "alpha", "beta", "file.txt"]);
    }

    #[test]
    fn empty_listing_has_no_selection_and_activates_to_none() {
        let mut panel = Panel {
            path: PathBuf::from("/"),
            entries: Vec::new(),
            cursor: 0,
            sort: Sort::default(),
            selection: HashSet::new(),
        };
        assert!(panel.cursor_entry().is_none());
        panel.move_cursor(3);
        assert_eq!(panel.cursor(), 0);
        assert_eq!(panel.activate().unwrap(), Activation::None);
    }

    #[test]
    fn sorting_falls_back_to_first_entry_without_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.entries.clear();
        panel.sort_by(SortKey::Size);
        assert_eq!(panel.cursor(), 0);
    }

    #[test]
    fn go_up_to_root_has_no_parent_entry() {
        let mut panel = Panel::open("/tmp").unwrap();
        panel.go_up().unwrap();
        assert_eq!(panel.path(), Path::new("/"));
        assert!(panel.entries().iter().all(|e| e.kind != EntryKind::Parent));
        assert_eq!(panel.cursor_entry().unwrap().label, "tmp");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn navigates_into_directory_with_non_utf8_name() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let tmp = fixture();
        let raw = OsStr::from_bytes(b"caf\xe9");
        fs::create_dir(tmp.path().join(raw)).unwrap();

        let mut panel = Panel::open(tmp.path()).unwrap();
        let ix = panel.entries().iter().position(|e| e.name == raw).unwrap();
        assert_eq!(panel.entries()[ix].label, "caf\u{fffd}");
        panel.set_cursor(ix);
        assert_eq!(panel.activate().unwrap(), Activation::Navigated);
        assert_eq!(panel.path(), tmp.path().join(raw));
    }

    #[test]
    fn case_sensitivity_resorts_and_keeps_cursor() {
        let tmp = fixture();
        fs::create_dir(tmp.path().join("Zeta")).unwrap();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "alpha");

        panel.set_case_sensitive(true);
        assert!(panel.sort().case_sensitive);
        assert_eq!(panel.entries()[1].label, "Zeta");
        assert_eq!(panel.cursor_entry().unwrap().label, "alpha");

        panel.set_case_sensitive(false);
        assert_eq!(panel.entries()[3].label, "Zeta");
        assert_eq!(panel.cursor_entry().unwrap().label, "alpha");
    }

    #[test]
    fn case_sensitivity_survives_navigation() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.set_case_sensitive(true);
        select(&mut panel, "alpha");
        panel.activate().unwrap();
        assert!(panel.sort().case_sensitive);
    }

    fn selected_labels(panel: &Panel) -> Vec<&str> {
        panel.selection().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn toggle_selects_then_moves_down_and_toggles_back() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "alpha");

        panel.toggle_selection();
        assert_eq!(selected_labels(&panel), ["alpha"]);
        assert_eq!(panel.cursor_entry().unwrap().label, "beta");

        panel.toggle_selection();
        assert_eq!(selected_labels(&panel), ["alpha", "beta"]);

        select(&mut panel, "alpha");
        panel.toggle_selection();
        assert_eq!(selected_labels(&panel), ["beta"]);
    }

    #[test]
    fn parent_entry_is_never_selected_but_cursor_moves_past_it() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        assert_eq!(panel.cursor(), 0);
        panel.toggle_selection();
        assert!(selected_labels(&panel).is_empty());
        assert_eq!(panel.cursor(), 1);
    }

    #[test]
    fn toggle_on_last_entry_stays_on_it() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "file.txt");
        panel.toggle_selection();
        assert_eq!(selected_labels(&panel), ["file.txt"]);
        assert_eq!(panel.cursor_entry().unwrap().label, "file.txt");
    }

    #[test]
    fn select_all_skips_parent() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.select_all();
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
        assert!(!panel.is_selected(&panel.entries()[0]));
    }

    #[test]
    fn selection_survives_sorting_in_display_order() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        select(&mut panel, "alpha");
        panel.toggle_selection();
        panel.toggle_selection();
        panel.sort_by(SortKey::Name);
        assert_eq!(selected_labels(&panel), ["beta", "alpha"]);
    }

    #[test]
    fn changing_directory_clears_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.select_all();
        select(&mut panel, "alpha");
        panel.activate().unwrap();
        assert!(selected_labels(&panel).is_empty());
        panel.go_up().unwrap();
        assert!(selected_labels(&panel).is_empty());
    }

    #[test]
    fn rereading_same_directory_keeps_surviving_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.select_all();
        fs::remove_dir(tmp.path().join("beta")).unwrap();
        let path = panel.path().to_path_buf();
        panel.navigate(path, None).unwrap();
        assert_eq!(selected_labels(&panel), ["alpha", "file.txt"]);
    }

    #[test]
    fn failed_navigation_keeps_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        panel.select_all();
        select(&mut panel, "alpha");
        fs::remove_dir_all(tmp.path().join("alpha")).unwrap();
        assert!(panel.activate().is_err());
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
    }

    #[test]
    fn targets_are_selection_or_cursor_entry_but_never_parent() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path()).unwrap();
        assert!(panel.targets().is_empty(), "cursor on ..");

        select(&mut panel, "beta");
        let labels: Vec<_> = panel.targets().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["beta"]);

        select(&mut panel, "file.txt");
        panel.toggle_selection();
        select(&mut panel, "alpha");
        panel.toggle_selection();
        let labels: Vec<_> = panel.targets().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["alpha", "file.txt"]);
    }

    #[test]
    fn summary_counts_totals_and_selection() {
        let tmp = fixture();
        fs::write(tmp.path().join("more.txt"), b"123").unwrap();
        let mut panel = Panel::open(tmp.path()).unwrap();
        assert_eq!(
            panel.summary(),
            Summary {
                dirs: 2,
                files: 2,
                bytes: 8,
                ..Summary::default()
            }
        );

        select(&mut panel, "alpha");
        panel.toggle_selection();
        select(&mut panel, "more.txt");
        panel.toggle_selection();
        let summary = panel.summary();
        assert_eq!((summary.selected_dirs, summary.selected_files), (1, 1));
        assert_eq!(summary.selected_bytes, 3);
        assert_eq!((summary.selected(), summary.total()), (2, 4));
    }
}
