use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;

use crate::archive::ArchiveIndex;
use crate::entry::{Entry, EntryKind, is_hidden_name, read_entries};
use crate::file_ops::Format;
use crate::find::Results;
use crate::listing::{ArchiveRead, Listing};
use crate::sort::{Sort, SortKey, sort_entries};

/// One side of the commander: a directory listing with a cursor.
///
/// Paths are kept logical (not canonicalized), so entering a symlinked
/// directory and then going to ".." returns to where you came from.
///
/// Everything index-based (cursor, [`Panel::entries`], quick search, summary,
/// selection, targets) sees only the visible entries.
#[derive(Debug)]
pub struct Panel {
    /// Tells this tab from every other one, wherever it moves (Ctrl-U,
    /// closing tabs before it).
    id: PanelId,
    path: PathBuf,
    /// Visible entries in display order.
    entries: Vec<Entry>,
    /// Hidden entries while they are not shown, unsorted.
    hidden: Vec<Entry>,
    show_hidden: bool,
    cursor: usize,
    sort: Sort,
    /// Names of selected entries. Kept by name so it survives re-sorting.
    selection: HashSet<OsString>,
    /// The folder being read for this panel, if any.
    loading: Option<Loading>,
    /// A quiet re-read in progress (directory watcher): no indicator, and
    /// commands keep working on the shown listing.
    refresh: Option<Refresh>,
    /// False until the first listing arrives (startup).
    loaded: bool,
    /// A re-read was asked for while another read was pending: the folder
    /// may have changed after that read listed it.
    pub(crate) stale: bool,
    /// The archive `path` is inside, if any. `path` then runs through the
    /// archive file (`/x/a.zip/src`), so it never names a real folder.
    archive: Option<Arc<ArchiveIndex>>,
    /// The search results shown instead of the folder's entries ("Feed to
    /// panel"). `path` is then the search's root, and every entry's name
    /// is its path relative to it.
    results: Option<Arc<Results>>,
    /// The folder's modification time when its listing was read.
    modified: Option<SystemTime>,
    /// A read that failed while this tab was in the background: shown when
    /// it comes to the front.
    pub(crate) error: Option<String>,
}

/// A panel's identity for the session. A Ctrl-T copy gets a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(u64);

impl PanelId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
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

/// A directory a panel should show next, once read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Navigation {
    pub target: PathBuf,
    /// Entry to put the cursor on.
    pub select: Option<OsString>,
    /// `target` is inside this archive.
    pub archive: Option<ArchiveRead>,
    /// Show these search results (at `target`, their root), or re-check
    /// them on a reload.
    pub results: Option<Arc<Results>>,
    /// A missing `target` falls back to its nearest existing parent.
    pub up_if_missing: bool,
}

/// What Enter on the cursor entry leads to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Enter {
    Dir(Navigation),
    File(PathBuf),
    None,
}

/// A directory being read for a panel. The panel keeps showing its old
/// listing meanwhile.
#[derive(Debug)]
pub struct Loading {
    pub(crate) id: u64,
    pub path: PathBuf,
    pub(crate) select: Option<OsString>,
    pub progress: Arc<AtomicUsize>,
    /// Set once the load took long enough to show the indicator.
    pub visible: bool,
    /// The read's cancel flag, set when this is dropped (see [`Cancel`]).
    pub(crate) _cancel: Cancel,
}

/// Tells a read it is no longer wanted when the panel lets go of it:
/// finished (harmless), cancelled with Escape, or replaced by a newer read.
#[derive(Debug, Default)]
pub(crate) struct Cancel(pub Arc<AtomicBool>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Loading {
    pub fn entries_read(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }
}

/// A quiet re-read of a panel's folder (see [`Panel::is_refreshing`]).
#[derive(Debug)]
pub(crate) struct Refresh {
    pub id: u64,
    /// The panel's folder, if it vanished: this read looks for its nearest
    /// existing parent.
    pub gone: Option<PathBuf>,
    /// Set when this is dropped (see [`Cancel`]).
    pub _cancel: Cancel,
}

/// What happened when the entry under the cursor was activated (the tests'
/// synchronous wrappers).
#[cfg(test)]
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
    /// A panel on `path` with no listing yet (startup).
    pub(crate) fn empty(path: PathBuf, show_hidden: bool) -> Self {
        Self {
            id: PanelId::next(),
            path,
            entries: Vec::new(),
            hidden: Vec::new(),
            show_hidden,
            cursor: 0,
            sort: Sort::default(),
            selection: HashSet::new(),
            loading: None,
            refresh: None,
            loaded: false,
            stale: false,
            archive: None,
            results: None,
            modified: None,
            error: None,
        }
    }

    /// Ctrl-T: a copy of this panel's folder, listing, cursor and sort,
    /// without its selection or any read in progress.
    pub(crate) fn duplicate(&self) -> Self {
        Self {
            id: PanelId::next(),
            path: self.path.clone(),
            entries: self.entries.clone(),
            hidden: self.hidden.clone(),
            show_hidden: self.show_hidden,
            cursor: self.cursor,
            sort: self.sort,
            selection: HashSet::new(),
            loading: None,
            refresh: None,
            loaded: self.loaded,
            stale: false,
            archive: self.archive.clone(),
            results: self.results.clone(),
            modified: self.modified,
            error: None,
        }
    }

    /// Reads `path` right away (tests and [`crate::Commander::new`]).
    pub(crate) fn open(path: impl AsRef<Path>, show_hidden: bool) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let entries = read_entries(&path, &AtomicUsize::new(0))?;
        let mut panel = Self::empty(path.clone(), show_hidden);
        panel.apply(
            Listing {
                path,
                entries,
                archive: None,
                results: None,
                modified: None,
            },
            None,
        );
        Ok(panel)
    }

    pub fn id(&self) -> PanelId {
        self.id
    }

    pub fn loading(&self) -> Option<&Loading> {
        self.loading.as_ref()
    }

    pub(crate) fn loading_mut(&mut self) -> Option<&mut Loading> {
        self.loading.as_mut()
    }

    pub(crate) fn set_loading(&mut self, loading: Loading) {
        self.loading = Some(loading);
    }

    pub(crate) fn take_loading(&mut self) -> Option<Loading> {
        self.loading.take()
    }

    /// Whether a quiet re-read is pending. Unlike [`Panel::loading`], it
    /// blocks nothing and shows nothing.
    pub fn is_refreshing(&self) -> bool {
        self.refresh.is_some()
    }

    pub(crate) fn refresh_id(&self) -> Option<u64> {
        self.refresh.as_ref().map(|r| r.id)
    }

    pub(crate) fn set_refresh(&mut self, refresh: Refresh) {
        self.refresh = Some(refresh);
    }

    pub(crate) fn take_refresh(&mut self) -> Option<Refresh> {
        self.refresh.take()
    }

    /// The folder's modification time when the listing was read (plain
    /// folders only).
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// Whether a listing has arrived yet (false only at startup).
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub(crate) fn enter_target(&self) -> Enter {
        let Some(entry) = self.cursor_entry() else {
            return Enter::None;
        };
        match entry.kind {
            EntryKind::Parent => self.parent_target().map_or(Enter::None, Enter::Dir),
            EntryKind::Dir => Enter::Dir(Navigation {
                target: self.path.join(&entry.name),
                select: None,
                archive: self.archive_read(true),
                results: None,
                up_if_missing: false,
            }),
            // No archives in archives, nothing runs from one.
            EntryKind::File if self.in_archive() => Enter::None,
            EntryKind::File if Format::from_name(&entry.name).is_some() => {
                let file = self.path.join(&entry.name);
                Enter::Dir(Navigation {
                    target: file.clone(),
                    select: None,
                    archive: Some(ArchiveRead { file, known: None }),
                    results: None,
                    up_if_missing: false,
                })
            }
            EntryKind::File => Enter::File(self.path.join(&entry.name)),
        }
    }

    /// Going up: the parent, with the cursor on the folder we came from.
    pub(crate) fn parent_target(&self) -> Option<Navigation> {
        // Out of search results: their root folder.
        if self.results.is_some() {
            return Some(Navigation {
                target: self.path.clone(),
                select: None,
                archive: None,
                results: None,
                // The folder searched may be gone by now.
                up_if_missing: true,
            });
        }
        // At an archive's root, up leaves the archive.
        if let Some(index) = &self.archive
            && self.path == index.file()
        {
            return Some(Navigation {
                target: index.file().parent()?.to_path_buf(),
                select: index.file().file_name().map(|n| n.to_os_string()),
                archive: None,
                results: None,
                up_if_missing: false,
            });
        }
        let parent = self.path.parent()?.to_path_buf();
        Some(Navigation {
            target: parent,
            select: self.path.file_name().map(|n| n.to_os_string()),
            archive: self.archive_read(true),
            results: None,
            up_if_missing: false,
        })
    }

    /// Re-reading this folder, with the cursor on `select` or where it is.
    pub(crate) fn reload_target(&self, select: Option<&OsStr>) -> Navigation {
        let keep = self.cursor_entry().map(|e| e.name.clone());
        Navigation {
            target: self.path.clone(),
            select: select.map(OsStr::to_os_string).or(keep),
            // An archive is read again only if it changed.
            archive: self.archive_read(true),
            // Search results are re-checked, never searched again.
            results: self.results.clone(),
            up_if_missing: false,
        }
    }

    pub fn archive(&self) -> Option<&Arc<ArchiveIndex>> {
        self.archive.as_ref()
    }

    pub fn results(&self) -> Option<&Arc<Results>> {
        self.results.as_ref()
    }

    pub fn in_results(&self) -> bool {
        self.results.is_some()
    }

    pub fn in_archive(&self) -> bool {
        self.archive.is_some()
    }

    /// The real folder: the archive's folder inside an archive, else the
    /// panel's own (watcher, state file).
    pub fn real_dir(&self) -> &Path {
        match &self.archive {
            Some(index) => index.file().parent().unwrap_or(index.file()),
            None => &self.path,
        }
    }

    /// Where this panel's archive comes from, with its index when `keep`.
    pub(crate) fn archive_read(&self, keep: bool) -> Option<ArchiveRead> {
        self.archive.as_ref().map(|index| ArchiveRead {
            file: index.file().to_path_buf(),
            known: keep.then(|| index.clone()),
        })
    }

    /// Shows `nav` inside an archive whose index is at hand: no read.
    pub(crate) fn browse(&mut self, index: Arc<ArchiveIndex>, nav: &Navigation) {
        let listing = crate::archive::listing(index, &nav.target);
        self.apply(listing, nav.select.as_deref());
    }

    /// Shows search results: no read.
    pub(crate) fn show_results(&mut self, results: Arc<Results>, select: Option<&OsStr>) {
        self.apply(results.listing(), select);
    }

    /// Shows a finished read, with the cursor on `select` if it is there.
    pub(crate) fn apply(&mut self, listing: Listing, select: Option<&OsStr>) {
        let Listing {
            path: target,
            entries,
            archive,
            results,
            modified,
        } = listing;
        // The search decided what to show: nothing is hidden there.
        let (mut entries, hidden) = split_hidden(entries, self.show_hidden || results.is_some());
        sort_entries(&mut entries, self.sort);
        // Into or out of search results is a new listing, even at the
        // same path: cursor at the top, nothing selected.
        let same_dir =
            target == self.path && self.loaded && results.is_some() == self.results.is_some();
        let cursor = select
            .and_then(|name| entries.iter().position(|e| e.name == name))
            .unwrap_or(if same_dir {
                self.cursor.min(entries.len().saturating_sub(1))
            } else {
                0
            });
        if same_dir {
            self.selection
                .retain(|name| entries.iter().any(|e| &e.name == name));
        } else {
            self.selection.clear();
        }
        self.path = target;
        self.archive = archive;
        self.results = results;
        self.entries = entries;
        self.hidden = hidden;
        self.cursor = cursor;
        self.modified = modified;
        self.loaded = true;
    }

    pub fn shows_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Shows or hides hidden entries. Hiding deselects them, and if the
    /// cursor was on one it moves to the nearest visible entry (below first).
    pub(crate) fn set_show_hidden(&mut self, show: bool) {
        if show == self.show_hidden {
            return;
        }
        self.show_hidden = show;
        if self.results.is_some() {
            return; // every result shows; leaving them applies the setting
        }
        if show {
            self.entries.append(&mut self.hidden);
            self.resort(self.sort);
            return;
        }
        let (above, below) = self.entries.split_at(self.cursor);
        let keep = below
            .iter()
            .chain(above.iter().rev())
            .find(|e| !e.is_hidden())
            .map(|e| e.name.clone());
        let (entries, hidden) = split_hidden(std::mem::take(&mut self.entries), false);
        self.entries = entries;
        self.hidden = hidden;
        self.selection.retain(|name| !is_hidden_name(name));
        self.cursor = keep
            .and_then(|name| self.entries.iter().position(|e| e.name == name))
            .unwrap_or(0);
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

    /// Back to Name ascending (the column sorted by was hidden).
    pub(crate) fn sort_by_name(&mut self) {
        self.resort(Sort {
            key: SortKey::Name,
            descending: false,
            ..self.sort
        });
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

    pub(crate) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// First entry (in display order, never "..") whose name starts with
    /// `prefix`, ignoring case.
    pub fn find_prefix(&self, prefix: &str) -> Option<usize> {
        let prefix = prefix.to_lowercase();
        self.entries.iter().position(|e| {
            e.kind != EntryKind::Parent && file_name(&e.sort_name).starts_with(&prefix)
        })
    }

    /// The next (or previous) entry after `from` whose name starts with
    /// `prefix`, ignoring case and "..", wrapping around the listing.
    pub fn find_prefix_from(&self, prefix: &str, from: usize, forward: bool) -> Option<usize> {
        let prefix = prefix.to_lowercase();
        let len = self.entries.len();
        (1..=len)
            .map(|step| {
                if forward {
                    (from + step) % len
                } else {
                    (from + len - step % len) % len
                }
            })
            .find(|&ix| {
                let e = &self.entries[ix];
                e.kind != EntryKind::Parent && file_name(&e.sort_name).starts_with(&prefix)
            })
    }

    /// What F4 opens: the entry under the cursor, or the panel's own
    /// directory when the cursor is on "..".
    pub fn cursor_path(&self) -> PathBuf {
        match self.cursor_entry() {
            Some(entry) if entry.kind != EntryKind::Parent => self.path.join(&entry.name),
            _ => self.path.clone(),
        }
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
}

/// A name's last component: a search result's file name (its name is a
/// relative path); any other name as it is.
fn file_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn split_hidden(entries: Vec<Entry>, show_hidden: bool) -> (Vec<Entry>, Vec<Entry>) {
    if show_hidden {
        (entries, Vec::new())
    } else {
        entries.into_iter().partition(|e| !e.is_hidden())
    }
}

#[cfg(test)]
impl Panel {
    /// Reads `nav` now and shows it, like a finished load.
    fn follow(&mut self, nav: Navigation) -> io::Result<()> {
        let listing = crate::listing::read_listing(&crate::listing::LoadRequest {
            id: 0,
            path: nav.target.clone(),
            fallback: None,
            progress: Arc::default(),
            cancel: Arc::default(),
            archive: nav.archive.clone(),
            results: nav.results.clone(),
            up_if_missing: false,
            quiet: false,
        })?;
        self.apply(listing, nav.select.as_deref());
        Ok(())
    }

    pub(crate) fn activate(&mut self) -> io::Result<Activation> {
        match self.enter_target() {
            Enter::Dir(nav) => self.follow(nav).map(|()| Activation::Navigated),
            Enter::File(path) => Ok(Activation::File(path)),
            Enter::None => Ok(Activation::None),
        }
    }

    pub(crate) fn go_up(&mut self) -> io::Result<()> {
        match self.parent_target() {
            Some(nav) => self.follow(nav),
            None => Ok(()),
        }
    }

    pub(crate) fn reload(&mut self, select: Option<&OsStr>) -> io::Result<()> {
        let nav = self.reload_target(select);
        self.follow(nav)
    }

    pub(crate) fn show(&mut self, path: PathBuf, select: Option<&OsStr>) -> io::Result<()> {
        self.follow(Navigation {
            target: path,
            select: select.map(OsStr::to_os_string),
            archive: None,
            results: None,
            up_if_missing: false,
        })
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.move_cursor(-5);
        assert_eq!(panel.cursor(), 0);
        panel.move_cursor(100);
        assert_eq!(panel.cursor(), panel.entries().len() - 1);
    }

    #[test]
    fn enter_descends_and_parent_returns_to_previous_dir() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open("/", true).unwrap();
        panel.go_up().unwrap();
        assert_eq!(panel.path(), Path::new("/"));
    }

    #[test]
    fn open_fails_for_missing_or_non_directory_path() {
        let tmp = fixture();
        assert!(Panel::open(tmp.path().join("missing"), true).is_err());
        assert!(Panel::open(tmp.path().join("file.txt"), true).is_err());
    }

    #[test]
    fn open_makes_relative_paths_absolute() {
        let panel = Panel::open(".", true).unwrap();
        assert!(panel.path().is_absolute());
    }

    #[test]
    fn starts_sorted_by_name_with_cursor_on_first_entry() {
        let tmp = fixture();
        let panel = Panel::open(tmp.path(), true).unwrap();
        assert_eq!(panel.sort(), Sort::default());
        assert_eq!(panel.cursor(), 0);
        let labels: Vec<_> = panel.entries().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["..", "alpha", "beta", "file.txt"]);
    }

    #[test]
    fn empty_listing_has_no_selection_and_activates_to_none() {
        let mut panel = Panel::empty(PathBuf::from("/"), true);
        assert!(panel.cursor_entry().is_none());
        panel.move_cursor(3);
        assert_eq!(panel.cursor(), 0);
        assert_eq!(panel.activate().unwrap(), Activation::None);
    }

    #[test]
    fn sorting_falls_back_to_first_entry_without_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.entries.clear();
        panel.sort_by(SortKey::Size);
        assert_eq!(panel.cursor(), 0);
    }

    #[test]
    fn go_up_to_root_has_no_parent_entry() {
        let mut panel = Panel::open("/tmp", true).unwrap();
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

        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        assert_eq!(panel.cursor(), 0);
        panel.toggle_selection();
        assert!(selected_labels(&panel).is_empty());
        assert_eq!(panel.cursor(), 1);
    }

    #[test]
    fn toggle_on_last_entry_stays_on_it() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        select(&mut panel, "file.txt");
        panel.toggle_selection();
        assert_eq!(selected_labels(&panel), ["file.txt"]);
        assert_eq!(panel.cursor_entry().unwrap().label, "file.txt");
    }

    #[test]
    fn select_all_skips_parent() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.select_all();
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
        assert!(!panel.is_selected(&panel.entries()[0]));
    }

    #[test]
    fn selection_survives_sorting_in_display_order() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        select(&mut panel, "alpha");
        panel.toggle_selection();
        panel.toggle_selection();
        panel.sort_by(SortKey::Name);
        assert_eq!(selected_labels(&panel), ["beta", "alpha"]);
    }

    #[test]
    fn changing_directory_clears_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.select_all();
        fs::remove_dir(tmp.path().join("beta")).unwrap();
        let path = panel.path().to_path_buf();
        panel.show(path, None).unwrap();
        assert_eq!(selected_labels(&panel), ["alpha", "file.txt"]);
    }

    #[test]
    fn failed_navigation_keeps_selection() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.select_all();
        select(&mut panel, "alpha");
        fs::remove_dir_all(tmp.path().join("alpha")).unwrap();
        assert!(panel.activate().is_err());
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
    }

    #[test]
    fn targets_are_selection_or_cursor_entry_but_never_parent() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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
        let mut panel = Panel::open(tmp.path(), true).unwrap();
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

    #[test]
    fn find_prefix_is_case_insensitive_and_skips_parent() {
        let tmp = fixture();
        let panel = Panel::open(tmp.path(), true).unwrap();
        assert_eq!(panel.find_prefix("B"), Some(2));
        assert_eq!(panel.find_prefix("file"), Some(3));
        assert_eq!(panel.find_prefix("."), None);
        assert_eq!(panel.find_prefix("zzz"), None);
    }

    #[test]
    fn find_prefix_from_steps_through_matches_and_wraps() {
        let tmp = fixture();
        fs::create_dir(tmp.path().join("apple")).unwrap();
        let panel = Panel::open(tmp.path(), true).unwrap();
        // "..", "alpha", "apple", "beta", "file.txt"
        assert_eq!(panel.find_prefix_from("a", 1, true), Some(2));
        assert_eq!(panel.find_prefix_from("a", 2, true), Some(1), "wraps");
        assert_eq!(panel.find_prefix_from("a", 1, false), Some(2), "wraps back");
        assert_eq!(panel.find_prefix_from("A", 2, false), Some(1));
        assert_eq!(panel.find_prefix_from("f", 4, true), Some(4), "only match");
        assert_eq!(panel.find_prefix_from("z", 0, true), None);
        let empty = Panel {
            entries: Vec::new(),
            ..panel
        };
        assert_eq!(empty.find_prefix_from("a", 0, true), None);
    }

    #[test]
    fn cursor_path_is_entry_or_own_directory_on_parent() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        assert_eq!(panel.cursor_path(), tmp.path());
        select(&mut panel, "file.txt");
        assert_eq!(panel.cursor_path(), tmp.path().join("file.txt"));
    }

    #[test]
    fn reload_can_move_cursor_to_a_given_entry() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        fs::create_dir(tmp.path().join("gamma")).unwrap();
        panel.reload(Some(OsStr::new("gamma"))).unwrap();
        assert_eq!(panel.cursor_entry().unwrap().label, "gamma");
        panel.reload(None).unwrap();
        assert_eq!(panel.cursor_entry().unwrap().label, "gamma");
    }
    fn labels(panel: &Panel) -> Vec<&str> {
        panel.entries().iter().map(|e| e.label.as_str()).collect()
    }

    /// Visible: "..", "alpha", "beta", "file.txt". Hidden: ".config" (dir), ".rc" (5 bytes).
    fn hidden_fixture() -> tempfile::TempDir {
        let tmp = fixture();
        fs::create_dir(tmp.path().join(".config")).unwrap();
        fs::write(tmp.path().join(".rc"), b"12345").unwrap();
        tmp
    }

    #[test]
    fn hidden_entries_are_left_out_of_everything_index_based() {
        let tmp = hidden_fixture();
        let mut panel = Panel::open(tmp.path(), false).unwrap();
        assert!(!panel.shows_hidden());
        assert_eq!(labels(&panel), ["..", "alpha", "beta", "file.txt"]);
        assert_eq!(panel.find_prefix(".r"), None);
        assert_eq!((panel.summary().dirs, panel.summary().files), (2, 1));
        assert_eq!(panel.summary().bytes, 5);
        panel.select_all();
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
        panel.set_cursor(usize::MAX);
        assert_eq!(panel.cursor_entry().unwrap().label, "file.txt");
    }

    #[test]
    fn showing_hidden_entries_sorts_them_in_and_keeps_the_cursor() {
        let tmp = hidden_fixture();
        let mut panel = Panel::open(tmp.path(), false).unwrap();
        select(&mut panel, "beta");
        panel.set_show_hidden(true);
        assert!(panel.shows_hidden());
        assert_eq!(
            labels(&panel),
            ["..", ".config", "alpha", "beta", ".rc", "file.txt"]
        );
        assert_eq!(panel.cursor_entry().unwrap().label, "beta");
        assert_eq!(panel.find_prefix(".r"), Some(4));
        panel.set_show_hidden(true);
        assert_eq!(panel.entries().len(), 6, "showing twice changes nothing");
    }

    #[test]
    fn hiding_deselects_hidden_entries_and_keeps_visible_selection() {
        let tmp = hidden_fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.select_all();
        panel.set_show_hidden(false);
        assert_eq!(selected_labels(&panel), ["alpha", "beta", "file.txt"]);
        panel.set_show_hidden(true);
        assert_eq!(
            selected_labels(&panel),
            ["alpha", "beta", "file.txt"],
            "showing again does not reselect"
        );
    }

    #[test]
    fn hiding_moves_cursor_to_nearest_visible_entry() {
        let tmp = hidden_fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();

        select(&mut panel, ".rc");
        panel.set_show_hidden(false);
        assert_eq!(
            panel.cursor_entry().unwrap().label,
            "file.txt",
            "below first"
        );

        panel.set_show_hidden(true);
        select(&mut panel, "alpha");
        panel.set_show_hidden(false);
        assert_eq!(
            panel.cursor_entry().unwrap().label,
            "alpha",
            "visible stays"
        );

        fs::remove_file(tmp.path().join("file.txt")).unwrap();
        panel.set_show_hidden(true);
        panel.reload(None).unwrap();
        select(&mut panel, ".rc");
        panel.set_show_hidden(false);
        assert_eq!(panel.cursor_entry().unwrap().label, "beta", "else above");
    }

    #[test]
    fn hiding_everything_leaves_an_empty_listing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".only"), b"").unwrap();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        panel.entries.remove(0); // as in a root directory: no ".."
        assert_eq!(panel.cursor_entry().unwrap().label, ".only");
        panel.set_show_hidden(false);
        assert!(panel.entries().is_empty());
        assert!(panel.cursor_entry().is_none());
    }

    #[test]
    fn navigation_and_reload_respect_the_hidden_setting() {
        let tmp = hidden_fixture();
        fs::write(tmp.path().join("alpha/.inner-rc"), b"").unwrap();
        let mut panel = Panel::open(tmp.path(), false).unwrap();
        select(&mut panel, "alpha");
        panel.activate().unwrap();
        assert_eq!(labels(&panel), ["..", "inner"]);
        panel.go_up().unwrap();
        fs::write(tmp.path().join(".new"), b"").unwrap();
        panel.reload(None).unwrap();
        assert!(!labels(&panel).contains(&".new"));
        panel.set_show_hidden(true);
        assert!(labels(&panel).contains(&".new"));
    }

    #[test]
    fn reload_keeps_the_row_when_the_cursor_entry_disappears() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), false).unwrap();
        select(&mut panel, "beta");
        fs::rename(tmp.path().join("beta"), tmp.path().join(".beta")).unwrap();
        panel.reload(None).unwrap();
        assert_eq!(panel.cursor_entry().unwrap().label, "file.txt");

        select(&mut panel, "file.txt");
        fs::remove_file(tmp.path().join("file.txt")).unwrap();
        panel.reload(None).unwrap();
        assert_eq!(panel.cursor_entry().unwrap().label, "alpha", "clamped");
    }
    #[test]
    fn targets_describe_navigation_without_reading() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        select(&mut panel, "beta");
        let Enter::Dir(nav) = panel.enter_target() else {
            panic!("beta is a directory")
        };
        assert_eq!(nav.target, tmp.path().join("beta"));
        assert_eq!(panel.path(), tmp.path(), "nothing read yet");
        let up = panel.parent_target().unwrap();
        assert_eq!(up.target, tmp.path().parent().unwrap());
        assert_eq!(up.select.as_deref(), tmp.path().file_name());
        let again = panel.reload_target(None);
        assert_eq!(
            (again.target, again.select),
            (tmp.path().to_path_buf(), Some("beta".into()))
        );
    }

    #[test]
    fn an_empty_panel_is_not_loaded_until_a_listing_arrives() {
        let tmp = fixture();
        let mut panel = Panel::empty(tmp.path().to_path_buf(), true);
        assert!(!panel.is_loaded());
        assert!(panel.entries().is_empty());
        let entries = read_entries(tmp.path(), &AtomicUsize::new(0)).unwrap();
        panel.apply(
            Listing {
                path: tmp.path().to_path_buf(),
                entries,
                archive: None,
                results: None,
                modified: None,
            },
            Some(OsStr::new("beta")),
        );
        assert!(panel.is_loaded());
        assert_eq!(panel.cursor_entry().unwrap().label, "beta");
    }

    #[test]
    fn loading_state_is_set_and_taken() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), true).unwrap();
        let progress = Arc::new(AtomicUsize::new(7));
        panel.set_loading(Loading {
            id: 3,
            path: tmp.path().join("beta"),
            select: None,
            progress,
            visible: false,
            _cancel: Cancel::default(),
        });
        assert_eq!(panel.loading().unwrap().entries_read(), 7);
        panel.loading_mut().unwrap().visible = true;
        assert!(panel.take_loading().unwrap().visible);
        assert!(panel.loading().is_none());
    }

    #[test]
    fn duplicate_copies_the_listing_but_not_the_selection_or_reads() {
        let tmp = fixture();
        let mut panel = Panel::open(tmp.path(), false).unwrap();
        panel.set_cursor(1);
        panel.toggle_selection();
        panel.sort_by(SortKey::Size);
        let copy = panel.duplicate();
        assert_eq!(copy.path(), panel.path());
        assert_eq!(copy.cursor(), panel.cursor());
        assert_eq!(copy.sort(), panel.sort());
        assert_eq!(copy.entries().len(), panel.entries().len());
        assert!(copy.is_loaded());
        assert_eq!(copy.selection().count(), 0);
        assert!(copy.loading().is_none() && !copy.is_refreshing());
    }
}
