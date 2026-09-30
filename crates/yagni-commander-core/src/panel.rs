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
        let keep = self.selected().map(|e| e.name.clone());
        self.sort = self.sort.toggled(key);
        sort_entries(&mut self.entries, self.sort);
        self.cursor = keep
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(0);
    }

    pub fn selected(&self) -> Option<&Entry> {
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
        let Some(entry) = self.selected() else {
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

    /// Loads `target` and only commits the change if reading succeeded,
    /// so a failed navigation leaves the panel untouched.
    fn navigate(&mut self, target: PathBuf, select: Option<&std::ffi::OsStr>) -> io::Result<()> {
        let mut entries = read_entries(&target)?;
        sort_entries(&mut entries, self.sort);
        let cursor = select
            .and_then(|name| entries.iter().position(|e| e.name == name))
            .unwrap_or(0);
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
        assert_eq!(panel.selected().unwrap().kind, EntryKind::Parent);

        assert_eq!(panel.activate().unwrap(), Activation::Navigated);
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.selected().unwrap().label, "beta");
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
        assert_eq!(panel.selected().unwrap().label, "alpha");
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
        assert_eq!(panel.selected().unwrap().label, "link");
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
        assert_eq!(panel.selected().unwrap().label, "beta");

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
}
