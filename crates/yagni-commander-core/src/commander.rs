use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::entry::{Entry, EntryKind};
use crate::fs_ops;
use crate::listing::{ArchiveRead, Listing, LoadRequest, read_listing};
use crate::oplog::OperationLog;
use crate::panel::{Enter, Loading, Navigation, Panel, Refresh};
use crate::quick_search::QuickSearch;
use crate::sort::SortKey;
use crate::tabs::Tabs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Everything the UI can ask the core to do. The UI owns the keymap and
/// translates key presses into these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    CursorUp,
    CursorDown,
    CursorHome,
    CursorEnd,
    /// Moves the cursor by a signed number of rows (page up/down, mouse wheel).
    CursorBy(isize),
    /// Moves the cursor to a row (mouse click).
    CursorTo(Side, usize),
    SwitchPanel,
    Focus(Side),
    Activate,
    GoUp,
    /// Column header click on a panel.
    SortBy(Side, SortKey),
    /// Space: toggle selection of the entry under the cursor, then move down.
    ToggleSelection,
    /// Ctrl-A: select all files and directories.
    SelectAll,
    /// Ctrl-U: swap the two panels.
    SwapPanels,
    /// Alt-Z: show the active panel's directory in the other panel too.
    SyncOtherPanel,
    /// Ctrl-R: re-read both panels.
    Reload,
    /// Ctrl-.: show or hide hidden entries in both panels.
    ToggleHidden,
    /// Ctrl-T: a copy of the active side's front tab, after it.
    NewTab,
    /// Ctrl-W: close the active side's front tab (never the last one).
    CloseTab,
    /// Ctrl-Tab / Ctrl-Shift-Tab on the active side, wrapping around.
    NextTab,
    PrevTab,
    /// A click on a tab: brings that side and tab to the front.
    SelectTab(Side, usize),
}

/// Result of a command that the UI may need to act on.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Enter was pressed on a file.
    OpenFile(PathBuf),
}

/// One side's folders at startup and which one is in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartTabs {
    pub dirs: Vec<PathBuf>,
    pub active: usize,
}

impl StartTabs {
    pub fn one(dir: PathBuf) -> Self {
        Self {
            dirs: vec![dir],
            active: 0,
        }
    }
}

/// A tab: its side and its index there.
type TabRef = (Side, usize);

/// Dual-panel state.
#[derive(Debug)]
pub struct Commander {
    /// Each side's tabs, indexed by `Side as usize`.
    tabs: [Tabs; 2],
    active: Side,
    /// Last error, shown by the UI until the next successful command.
    error: Option<String>,
    /// Quick search in the active panel. Any command ends it.
    search: QuickSearch,
    /// Where file changes are logged, if logging is on.
    log: Option<Arc<OperationLog>>,
    /// Where startup and Escape-at-startup fall back to.
    home: PathBuf,
    /// Id of the last load requested.
    next_load: u64,
    /// Loads the UI has not taken yet (see [`Commander::take_requests`]).
    requests: Vec<LoadRequest>,
    /// Columns turned off in the config; panels can't be sorted by them.
    hidden_columns: Vec<SortKey>,
    /// Panels show an icon before each name (`Config::icons`).
    icons: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    Navigate,
    /// Same folder again; dropped while a navigation is pending.
    Reload,
    /// With the parents-then-home fallback.
    Startup,
}

impl Commander {
    /// Reads both folders right away (tests, tools). The app uses
    /// [`Commander::start`], which reads in the background.
    pub fn new(
        left: impl AsRef<Path>,
        right: impl AsRef<Path>,
        show_hidden: bool,
    ) -> io::Result<Self> {
        Ok(Self::with_panels(
            Panel::open(left, show_hidden)?,
            Panel::open(right, show_hidden)?,
            crate::storage::home_dir(),
        ))
    }

    /// Nothing is read yet: both panels ask for their folder (falling back
    /// to its parents, then `home`). See [`Commander::take_requests`].
    pub fn start(left: PathBuf, right: PathBuf, show_hidden: bool, home: PathBuf) -> Self {
        Self::start_tabs(
            StartTabs::one(left),
            StartTabs::one(right),
            show_hidden,
            home,
        )
    }

    /// Like [`Commander::start`] with several tabs per side. Only the tab
    /// in front of each side is read; the others on their first visit.
    pub fn start_tabs(left: StartTabs, right: StartTabs, show_hidden: bool, home: PathBuf) -> Self {
        let side = |start: StartTabs| {
            let dirs = if start.dirs.is_empty() {
                vec![home.clone()]
            } else {
                start.dirs
            };
            let panels = dirs
                .into_iter()
                .map(|dir| {
                    let path = std::path::absolute(&dir).unwrap_or_else(|_| home.clone());
                    Panel::empty(path, show_hidden)
                })
                .collect();
            Tabs::from_panels(panels, start.active)
        };
        let mut commander = Self::with_tabs([side(left), side(right)], home.clone());
        for side in [Side::Left, Side::Right] {
            let target = commander.panel(side).path().to_path_buf();
            commander.request(
                side,
                Navigation {
                    target,
                    select: None,
                    archive: None,
                },
                LoadKind::Startup,
            );
        }
        commander
    }

    fn with_panels(left: Panel, right: Panel, home: PathBuf) -> Self {
        Self::with_tabs([Tabs::new(left), Tabs::new(right)], home)
    }

    fn with_tabs(tabs: [Tabs; 2], home: PathBuf) -> Self {
        Self {
            tabs,
            active: Side::Left,
            error: None,
            search: QuickSearch::default(),
            log: None,
            home,
            next_load: 0,
            requests: Vec::new(),
            hidden_columns: Vec::new(),
            icons: true,
        }
    }

    fn request(&mut self, side: Side, nav: Navigation, kind: LoadKind) {
        // Moving inside an archive whose index is at hand needs no read.
        if kind == LoadKind::Navigate
            && let Some(index) = nav.archive.as_ref().and_then(|a| a.known.clone())
        {
            let panel = self.panel_mut(side);
            // Like a new read, this replaces a pending one (Alt-Z onto a
            // loading panel). A quiet re-read means the archive changed:
            // read it again for the new location.
            panel.take_loading();
            let reread = panel.take_refresh().is_some() || panel.stale;
            panel.stale = false;
            panel.browse(index, &nav);
            if reread {
                self.refresh(self.visible(side), None);
            }
            return;
        }
        if kind == LoadKind::Reload && self.panel(side).loading().is_some() {
            // Re-read once the pending read is done: it may list the folder
            // before this change, or be cancelled, or fail.
            self.panel_mut(side).stale = true;
            return;
        }
        // A navigation or reload replaces a quiet re-read; its result will
        // be dropped as stale.
        self.panel_mut(side).take_refresh();
        let fallback = (kind == LoadKind::Startup).then(|| self.home.clone());
        let (id, progress) = self.push_request(nav.target.clone(), fallback, nav.archive);
        self.panel_mut(side).set_loading(Loading {
            id,
            path: nav.target,
            select: nav.select,
            progress,
            visible: false,
        });
    }

    /// Queues a read for the UI to take (see [`Commander::take_requests`]).
    fn push_request(
        &mut self,
        path: PathBuf,
        fallback: Option<PathBuf>,
        archive: Option<ArchiveRead>,
    ) -> (u64, Arc<AtomicUsize>) {
        self.next_load += 1;
        let id = self.next_load;
        let progress = Arc::new(AtomicUsize::new(0));
        self.requests.push(LoadRequest {
            id,
            path,
            fallback,
            progress: progress.clone(),
            archive,
        });
        (id, progress)
    }

    /// The directory watcher saw a change in `side`'s folder: re-read it
    /// quietly. Not a [`Command`]: the quick search and the error stay.
    pub fn watch_reload(&mut self, side: Side) {
        self.refresh(self.visible(side), None);
    }

    /// Starts a quiet re-read, or marks the panel stale if a read is
    /// pending. `gone`: the panel's folder vanished, so the read walks up
    /// to its nearest readable parent (then home).
    fn refresh(&mut self, at: TabRef, gone: Option<PathBuf>) {
        let panel = self.panel_at_mut(at);
        if !panel.is_loaded() {
            return; // the startup read is still to come
        }
        if panel.loading().is_some() || panel.is_refreshing() {
            panel.stale = true;
            return;
        }
        // A vanished folder (or archive) is looked for from its real folder up.
        let (path, archive) = match &gone {
            Some(dir) => (dir.clone(), None),
            None => (panel.path().to_path_buf(), panel.archive_read(true)),
        };
        let fallback = gone.is_some().then(|| self.home.clone());
        let (id, _) = self.push_request(path, fallback, archive);
        self.panel_at_mut(at).set_refresh(Refresh { id, gone });
    }

    /// A tab came to the front: re-read it quietly, or read it for the
    /// first time (a tab restored at startup) with the startup fallback.
    fn bring_to_front(&mut self, side: Side) {
        let panel = self.panel(side);
        if panel.loading().is_some() {
            return;
        }
        if panel.is_loaded() {
            self.refresh(self.visible(side), None);
        } else {
            let target = panel.path().to_path_buf();
            let nav = Navigation {
                target,
                select: None,
                archive: None,
            };
            self.request(side, nav, LoadKind::Startup);
        }
    }

    fn refresh_tab(&self, id: u64) -> Option<TabRef> {
        self.all_tabs()
            .find(|(_, panel)| panel.refresh_id() == Some(id))
            .map(|(at, _)| at)
    }

    /// Shows a quiet re-read's result. The cursor stays on the entry it is
    /// on now, which may differ from when the read started.
    fn finish_refresh(&mut self, at: TabRef, result: io::Result<Listing>) {
        let panel = self.panel_at_mut(at);
        let refresh = panel.take_refresh().expect("refresh_tab found it");
        match result {
            Ok(listing) => {
                let select = match refresh.gone {
                    None => panel.cursor_entry().map(|e| e.name.clone()),
                    Some(_) => None,
                };
                panel.apply(listing, select.as_deref());
            }
            Err(e)
                if refresh.gone.is_none()
                    && matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
            {
                let gone = panel.real_dir().to_path_buf();
                self.refresh(at, Some(gone));
                return;
            }
            // A flaky network read or lost permission: keep the listing and
            // say nothing; the user didn't ask for this read.
            Err(_) => {}
        }
        self.reread_if_stale(at);
    }

    /// The reads to run, oldest first. Requests replaced before they were
    /// taken are left out.
    pub fn take_requests(&mut self) -> Vec<LoadRequest> {
        let requests = std::mem::take(&mut self.requests);
        requests
            .into_iter()
            .filter(|r| self.is_pending(r.id))
            .collect()
    }

    /// Entries read so far by load `id`, while it is pending.
    pub fn panel_loading_count(&self, id: u64) -> Option<usize> {
        let at = self.loading_tab(id)?;
        self.panel_at(at).loading().map(|l| l.entries_read())
    }

    /// Whether load `id` is still wanted by a panel.
    pub fn is_pending(&self, id: u64) -> bool {
        self.loading_tab(id).is_some() || self.refresh_tab(id).is_some()
    }

    fn loading_tab(&self, id: u64) -> Option<TabRef> {
        self.all_tabs()
            .find(|(_, panel)| panel.loading().is_some_and(|l| l.id == id))
            .map(|(at, _)| at)
    }

    /// Shows a finished read in the panel that asked for it. Returns false
    /// for a stale result (cancelled or replaced), which is dropped.
    pub fn finish_load(&mut self, id: u64, result: io::Result<Listing>) -> bool {
        if let Some(at) = self.refresh_tab(id) {
            self.finish_refresh(at, result);
            return true;
        }
        let Some(at) = self.loading_tab(id) else {
            return false;
        };
        let panel = self.panel_at_mut(at);
        let loading = panel.take_loading().expect("loading_tab found it");
        match result {
            Ok(listing) => panel.apply(listing, loading.select.as_deref()),
            Err(e) => self.error = Some(format!("{}: {e}", loading.path.display())),
        }
        self.reread_if_stale(at);
        true
    }

    /// Runs a re-read that was held back while another read was pending.
    /// It is quiet: the user already sees this folder.
    fn reread_if_stale(&mut self, at: TabRef) {
        let panel = self.panel_at_mut(at);
        if !panel.stale || !panel.is_loaded() || panel.loading().is_some() || panel.is_refreshing()
        {
            return;
        }
        panel.stale = false;
        self.refresh(at, None);
    }

    /// The load took long enough to show the indicator.
    pub fn show_loading(&mut self, id: u64) -> bool {
        let Some(at) = self.loading_tab(id) else {
            return false;
        };
        if let Some(loading) = self.panel_at_mut(at).loading_mut() {
            loading.visible = true;
        }
        true
    }

    /// Escape while loading: stop waiting and stay on the old listing. A
    /// panel that has none yet (startup) goes to the home folder instead.
    pub fn cancel_load(&mut self, side: Side) -> bool {
        if self.panel_mut(side).take_loading().is_none() {
            return false;
        }
        if !self.panel(side).is_loaded() {
            let target = self.home.clone();
            self.request(
                side,
                Navigation {
                    target,
                    select: None,
                    archive: None,
                },
                LoadKind::Startup,
            );
        }
        self.reread_if_stale(self.visible(side));
        true
    }

    /// Ctrl-D: the active panel goes to `dir` (a hotlist entry) as a normal
    /// navigation. Like a command, ignored while that panel is loading, and
    /// ends the quick search. A failed read leaves the panel where it was
    /// and sets [`Commander::error`].
    pub fn go_to(&mut self, dir: PathBuf) {
        let active = self.active;
        if self.panel(active).loading().is_some() {
            return;
        }
        self.error = None;
        self.search.clear();
        self.request(
            active,
            Navigation {
                target: dir,
                select: None,
                archive: None,
            },
            LoadKind::Navigate,
        );
    }

    /// The home folder (hotlist `~` paths, startup fallback).
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Runs all pending reads on this thread (tests, and the app's tests).
    pub fn run_loads_now(&mut self) {
        loop {
            let requests = self.take_requests();
            if requests.is_empty() {
                break;
            }
            for request in requests {
                let result = read_listing(&request);
                self.finish_load(request.id, result);
            }
        }
    }

    /// The panel a command acts on, if it acts on one. Commands for a panel
    /// that is loading are ignored.
    fn command_side(&self, command: Command) -> Option<Side> {
        match command {
            Command::SwitchPanel
            | Command::SwapPanels
            | Command::Reload
            | Command::ToggleHidden
            | Command::Focus(_)
            | Command::CloseTab
            | Command::NextTab
            | Command::PrevTab
            | Command::SelectTab(..) => None,
            Command::CursorTo(side, _) | Command::SortBy(side, _) => Some(side),
            _ => Some(self.active),
        }
    }

    /// `side`'s tab in front.
    pub fn panel(&self, side: Side) -> &Panel {
        self.tabs[side as usize].active()
    }

    fn panel_mut(&mut self, side: Side) -> &mut Panel {
        self.tabs[side as usize].active_mut()
    }

    /// `side`'s tabs: labels, the one in front, every tab's folder.
    pub fn tabs(&self, side: Side) -> &Tabs {
        &self.tabs[side as usize]
    }

    fn visible(&self, side: Side) -> TabRef {
        (side, self.tabs[side as usize].index())
    }

    fn panel_at(&self, (side, ix): TabRef) -> &Panel {
        self.tabs[side as usize].get(ix).expect("a tab found by id")
    }

    fn panel_at_mut(&mut self, (side, ix): TabRef) -> &mut Panel {
        self.tabs[side as usize]
            .get_mut(ix)
            .expect("a tab found by id")
    }

    /// Every tab of both sides.
    fn all_tabs(&self) -> impl Iterator<Item = (TabRef, &Panel)> {
        [Side::Left, Side::Right].into_iter().flat_map(move |side| {
            self.tabs[side as usize]
                .iter()
                .enumerate()
                .map(move |(ix, panel)| ((side, ix), panel))
        })
    }

    fn all_panels_mut(&mut self) -> impl Iterator<Item = &mut Panel> {
        self.tabs.iter_mut().flat_map(Tabs::iter_mut)
    }

    pub fn active(&self) -> Side {
        self.active
    }

    pub fn set_active(&mut self, side: Side) {
        self.active = side;
    }

    /// Whether hidden entries are shown. Always the same for both panels.
    pub fn shows_hidden(&self) -> bool {
        self.panel(Side::Left).shows_hidden()
    }

    /// Whether typing `ch` starts or extends a quick search.
    pub fn search_accepts(ch: char) -> bool {
        QuickSearch::accepts(ch)
    }

    /// The quick search prefix, while the search box is open.
    pub fn search(&self) -> Option<&str> {
        self.search.prefix()
    }

    /// Quick search: appends `ch` and moves the active panel's cursor to the
    /// first entry starting with the result. If none does, returns false and
    /// changes nothing (the key is ignored).
    pub fn search_type(&mut self, ch: char) -> bool {
        if self.panel(self.active).loading().is_some() {
            return false;
        }
        let candidate = self.search.candidate(ch);
        let panel = self.panel_mut(self.active);
        let Some(index) = panel.find_prefix(&candidate) else {
            return false;
        };
        panel.set_cursor(index);
        self.search.set(candidate);
        true
    }

    /// Down/Up while searching: the next or previous match, wrapping around.
    /// Returns false if no search is open.
    pub fn search_step(&mut self, forward: bool) -> bool {
        let Some(prefix) = self.search.prefix() else {
            return false;
        };
        let prefix = prefix.to_owned();
        let panel = self.panel_mut(self.active);
        if let Some(index) = panel.find_prefix_from(&prefix, panel.cursor(), forward) {
            panel.set_cursor(index);
        }
        true
    }

    /// Backspace while searching: drops the last character; the cursor stays.
    /// Returns false if no search is open.
    pub fn search_backspace(&mut self) -> bool {
        let open = self.search.prefix().is_some();
        self.search.backspace();
        open
    }

    /// Escape: closes the search box. Returns false if none was open.
    pub fn search_cancel(&mut self) -> bool {
        let open = self.search.prefix().is_some();
        self.search.clear();
        open
    }

    /// Whether the active panel still shows `dir`, the folder a prompt
    /// opened on. The directory watcher can move a panel whose folder
    /// vanished while a dialog was open; acting then would hit the parent.
    pub fn check_dir(&self, dir: &Path) -> io::Result<()> {
        if self.panel(self.active).path() == dir {
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} no longer exists", dir.display()),
        ))
    }

    /// F2: renames `from` in the active panel's directory to `to`, then
    /// puts the cursor on it.
    pub fn rename(&mut self, from: &OsStr, to: &str) -> io::Result<()> {
        self.refuse_in_archive()?;
        let dir = self.panel(self.active).path().to_path_buf();
        let source = dir.join(from);
        let result = fs_ops::rename(&dir, from, to);
        self.note("rename", &source, &result, |source| {
            format!("renamed {} -> {}", source.display(), dir.join(to).display())
        });
        result?;
        self.refresh_after_change(OsStr::new(to));
        Ok(())
    }

    /// Nothing is written inside an archive.
    fn refuse_in_archive(&self) -> io::Result<()> {
        if self.panel(self.active).in_archive() {
            return Err(io::Error::other(crate::archive::IN_ARCHIVE));
        }
        Ok(())
    }

    /// Logs `result` of an action on `path`, if logging is on.
    fn note<T>(
        &self,
        action: &str,
        path: &Path,
        result: &io::Result<T>,
        done: impl FnOnce(&Path) -> String,
    ) {
        let Some(log) = &self.log else { return };
        match result {
            Ok(_) => log.write(format_args!("{action} {}", done(path))),
            Err(e) => log.write(format_args!("{action} failed {}: {e}", path.display())),
        }
    }

    /// Logs file changes from now on (the config's `log` setting).
    pub fn set_log(&mut self, log: Option<Arc<OperationLog>>) {
        self.log = log;
    }

    /// The log for background jobs to share.
    pub fn log(&self) -> Option<&Arc<OperationLog>> {
        self.log.as_ref()
    }

    /// F7: creates `name` (possibly `a/b/c`) in the active panel's directory,
    /// then puts the cursor on it.
    pub fn make_directory(&mut self, name: &str) -> io::Result<()> {
        self.refuse_in_archive()?;
        let dir = self.panel(self.active).path().to_path_buf();
        let result = fs_ops::make_directory(&dir, name);
        self.note("mkdir", &dir.join(name), &result, |path| {
            format!("created directory {}", path.display())
        });
        self.refresh_after_change(&result?);
        Ok(())
    }

    /// Shift-F4: creates an empty file at `name` (a name or a relative path
    /// like `a/b/c.txt`) in the active panel's directory, or keeps an existing
    /// one, puts the cursor on it (or on its first directory) and returns its
    /// path.
    pub fn create_file(&mut self, name: &str) -> io::Result<PathBuf> {
        self.refuse_in_archive()?;
        let dir = self.panel(self.active).path().to_path_buf();
        let result = fs_ops::create_file(&dir, name);
        // Only a file that was created counts as touched.
        if !matches!(&result, Ok(new) if !new.created) {
            self.note("new file", &dir.join(name), &result, |path| {
                format!("created {}", path.display())
            });
        }
        let new = result?;
        self.refresh_after_change(&new.first);
        Ok(new.path)
    }

    /// Re-reads the active panel with the cursor on `select`, and the other
    /// panel too if it shows the same directory.
    fn refresh_after_change(&mut self, select: &OsStr) {
        let active = self.active;
        let nav = self.panel(active).reload_target(Some(select));
        self.request(active, nav, LoadKind::Reload);
        if self.panel(Side::Left).path() == self.panel(Side::Right).path() {
            let nav = self.panel(active.other()).reload_target(None);
            self.request(active.other(), nav, LoadKind::Reload);
        }
    }

    /// Deselects everything in `side`'s front tab, if it still shows `dir`
    /// (a copy's sources; tabs may have changed while it ran).
    pub fn clear_selection(&mut self, side: Side, dir: &Path) {
        let panel = self.panel_mut(side);
        if panel.path() == dir {
            panel.clear_selection();
        }
    }

    /// The config's hidden columns. A tab sorted by one goes back to Name.
    pub fn hide_columns(&mut self, hidden: &[SortKey]) {
        self.hidden_columns = hidden.to_vec();
        for panel in self.all_panels_mut() {
            if hidden.contains(&panel.sort().key) {
                panel.sort_by_name();
            }
        }
    }

    /// Whether the column for `key` is shown (and can be sorted by).
    pub fn shows_column(&self, key: SortKey) -> bool {
        !self.hidden_columns.contains(&key)
    }

    /// Panels show an icon before each name.
    pub fn set_icons(&mut self, on: bool) {
        self.icons = on;
    }

    pub fn shows_icons(&self) -> bool {
        self.icons
    }

    /// The icon for `entry` in the `side` panel: the home glyph for the
    /// home folder itself, else [`crate::icons::icon`]. Compares paths only.
    pub fn icon(&self, side: Side, entry: &Entry) -> char {
        let is_home =
            entry.kind == EntryKind::Dir && self.panel(side).path().join(&entry.name) == self.home;
        if is_home {
            crate::icons::HOME
        } else {
            crate::icons::icon(entry)
        }
    }

    /// Applies the case-sensitive sorting setting to every tab.
    pub fn set_case_sensitive_sort(&mut self, case_sensitive: bool) {
        for panel in self.all_panels_mut() {
            panel.set_case_sensitive(case_sensitive);
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Runs a command. I/O errors are stored in [`Commander::error`] rather than
    /// returned, since the UI's only sensible reaction is to display them.
    pub fn execute(&mut self, command: Command) -> Outcome {
        if let Some(side) = self.command_side(command)
            && self.panel(side).loading().is_some()
        {
            // A click still focuses the loading panel, like Tab; nothing
            // else happens there (so a double-click's Enter is ignored too).
            if let Command::CursorTo(side, _) = command {
                self.active = side;
            }
            return Outcome::Done;
        }
        if let Command::SortBy(_, key) = command
            && !self.shows_column(key)
        {
            return Outcome::Done; // its column is hidden
        }
        self.error = None;
        self.search.clear();
        let active = self.active;
        let panel = self.panel_mut(active);
        let result: io::Result<()> = match command {
            Command::CursorUp => {
                panel.move_cursor(-1);
                Ok(())
            }
            Command::CursorDown => {
                panel.move_cursor(1);
                Ok(())
            }
            Command::CursorHome => {
                panel.set_cursor(0);
                Ok(())
            }
            Command::CursorEnd => {
                panel.set_cursor(usize::MAX);
                Ok(())
            }
            Command::CursorBy(delta) => {
                panel.move_cursor(delta);
                Ok(())
            }
            Command::CursorTo(side, index) => {
                self.active = side;
                self.panel_mut(side).set_cursor(index);
                Ok(())
            }
            Command::SwitchPanel => {
                self.active = active.other();
                Ok(())
            }
            Command::Focus(side) => {
                self.active = side;
                Ok(())
            }
            Command::ToggleSelection => {
                panel.toggle_selection();
                Ok(())
            }
            Command::SelectAll => {
                panel.select_all();
                Ok(())
            }
            Command::SwapPanels => {
                self.tabs.swap(0, 1);
                Ok(())
            }
            Command::ToggleHidden => {
                let show = !self.shows_hidden();
                for panel in self.all_panels_mut() {
                    panel.set_show_hidden(show);
                }
                Ok(())
            }
            Command::NewTab => {
                let copy = panel.duplicate();
                self.tabs[active as usize].open(copy);
                self.bring_to_front(active);
                Ok(())
            }
            Command::CloseTab => {
                if self.tabs[active as usize].close() {
                    self.bring_to_front(active);
                }
                Ok(())
            }
            Command::NextTab => {
                if self.tabs[active as usize].next() {
                    self.bring_to_front(active);
                }
                Ok(())
            }
            Command::PrevTab => {
                if self.tabs[active as usize].prev() {
                    self.bring_to_front(active);
                }
                Ok(())
            }
            Command::SelectTab(side, ix) => {
                self.active = side;
                if self.tabs[side as usize].select(ix) {
                    self.bring_to_front(side);
                }
                Ok(())
            }
            Command::SortBy(side, key) => {
                self.active = side;
                self.panel_mut(side).sort_by(key);
                Ok(())
            }
            Command::GoUp => {
                if let Some(nav) = panel.parent_target() {
                    self.request(active, nav, LoadKind::Navigate);
                }
                Ok(())
            }
            Command::SyncOtherPanel => {
                let nav = Navigation {
                    target: panel.path().to_path_buf(),
                    select: panel.cursor_entry().map(|e| e.name.clone()),
                    archive: panel.archive_read(true),
                };
                self.request(active.other(), nav, LoadKind::Navigate);
                Ok(())
            }
            Command::Reload => {
                for side in [Side::Left, Side::Right] {
                    let nav = self.panel(side).reload_target(None);
                    self.request(side, nav, LoadKind::Reload);
                }
                Ok(())
            }
            Command::Activate => match panel.enter_target() {
                Enter::Dir(nav) => {
                    self.request(active, nav, LoadKind::Navigate);
                    Ok(())
                }
                Enter::File(path) => return Outcome::OpenFile(path),
                Enter::None => Ok(()),
            },
        };
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
        Outcome::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::read_listing;
    use std::fs;

    #[test]
    fn the_home_folder_gets_the_home_icon_and_nothing_else_does() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        for dir in ["me/sub", "other/me", "files"] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        fs::write(base.join("files/me"), b"").unwrap();
        let home = base.join("me");
        let find = |c: &Commander, side: Side, label: &str| {
            let panel = c.panel(side);
            let entry = panel.entries().iter().find(|e| e.label == label).unwrap();
            c.icon(side, entry)
        };

        let mut c = Commander::start(base.to_path_buf(), base.join("other"), false, home.clone());
        c.run_loads_now();
        assert_eq!(find(&c, Side::Left, "me"), crate::icons::HOME);
        let other_me = find(&c, Side::Right, "me");
        assert_ne!(other_me, crate::icons::HOME, "same name, not home");

        let mut c = Commander::start(base.join("files"), home.clone(), false, home);
        c.run_loads_now();
        assert_ne!(find(&c, Side::Left, "me"), crate::icons::HOME, "a file");
        // Inside home, ".." stays an arrow and its folders are ordinary.
        assert_eq!(
            find(&c, Side::Right, ".."),
            crate::icons::icon(&crate::Entry::parent())
        );
        assert_ne!(find(&c, Side::Right, "sub"), crate::icons::HOME);
    }

    #[test]
    fn icons_are_on_until_turned_off() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        assert!(c.shows_icons());
        c.set_icons(false);
        assert!(!c.shows_icons());
    }

    #[test]
    fn switch_panel_toggles_and_commands_target_active_panel() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("a")).unwrap();
        fs::create_dir(tmp.path().join("b")).unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();

        c.execute(Command::CursorDown);
        assert_eq!(c.panel(Side::Left).cursor(), 1);
        assert_eq!(c.panel(Side::Right).cursor(), 0);

        c.execute(Command::SwitchPanel);
        assert_eq!(c.active(), Side::Right);
        c.execute(Command::CursorEnd);
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Right).path(), tmp.path().join("b"));
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
    }

    #[test]
    fn errors_are_recorded_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("gone")).unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        c.execute(Command::CursorDown);
        fs::remove_dir(tmp.path().join("gone")).unwrap();

        run(&mut c, Command::Activate);
        assert!(c.error().is_some());
        c.execute(Command::CursorUp);
        assert!(c.error().is_none());
    }

    #[test]
    fn enter_on_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("f"), b"").unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        c.execute(Command::CursorEnd);
        assert_eq!(
            run(&mut c, Command::Activate),
            Outcome::OpenFile(tmp.path().join("f"))
        );
    }

    /// Left and right both start in a directory with dirs `a`, `b`, `c` and file `f`.
    fn commander() -> (tempfile::TempDir, Commander) {
        let tmp = tempfile::tempdir().unwrap();
        for dir in ["a", "b", "c"] {
            fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        fs::write(tmp.path().join("f"), b"12345").unwrap();
        let c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        (tmp, c)
    }

    fn under_cursor(c: &Commander, side: Side) -> &str {
        &c.panel(side).cursor_entry().unwrap().label
    }

    #[test]
    fn side_other_is_symmetric() {
        assert_eq!(Side::Left.other(), Side::Right);
        assert_eq!(Side::Right.other(), Side::Left);
    }

    #[test]
    fn new_fails_for_missing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        assert!(Commander::new(&missing, tmp.path(), true).is_err());
        assert!(Commander::new(tmp.path(), &missing, true).is_err());
    }

    #[test]
    fn starts_with_left_active_and_no_error() {
        let (_tmp, c) = commander();
        assert_eq!(c.active(), Side::Left);
        assert!(c.error().is_none());
    }

    #[test]
    fn cursor_commands_move_within_bounds() {
        let (_tmp, mut c) = commander();
        let last = c.panel(Side::Left).entries().len() - 1;

        assert_eq!(c.execute(Command::CursorEnd), Outcome::Done);
        assert_eq!(c.panel(Side::Left).cursor(), last);
        c.execute(Command::CursorDown);
        assert_eq!(c.panel(Side::Left).cursor(), last);

        c.execute(Command::CursorHome);
        assert_eq!(c.panel(Side::Left).cursor(), 0);
        c.execute(Command::CursorUp);
        assert_eq!(c.panel(Side::Left).cursor(), 0);

        c.execute(Command::CursorBy(2));
        assert_eq!(c.panel(Side::Left).cursor(), 2);
        c.execute(Command::CursorBy(-10));
        assert_eq!(c.panel(Side::Left).cursor(), 0);
        c.execute(Command::CursorBy(100));
        assert_eq!(c.panel(Side::Left).cursor(), last);
    }

    #[test]
    fn cursor_to_activates_that_side() {
        let (_tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Right, 2));
        assert_eq!(c.active(), Side::Right);
        assert_eq!(c.panel(Side::Right).cursor(), 2);
        assert_eq!(c.panel(Side::Left).cursor(), 0);
    }

    #[test]
    fn focus_only_changes_active_side() {
        let (_tmp, mut c) = commander();
        c.execute(Command::CursorDown);
        c.execute(Command::Focus(Side::Right));
        assert_eq!(c.active(), Side::Right);
        c.execute(Command::Focus(Side::Right));
        assert_eq!(c.active(), Side::Right);
        assert_eq!(c.panel(Side::Left).cursor(), 1);
    }

    #[test]
    fn go_up_returns_to_parent_with_cursor_on_previous_dir() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 2));
        assert_eq!(under_cursor(&c, Side::Left), "b");
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("b"));

        run(&mut c, Command::GoUp);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert_eq!(under_cursor(&c, Side::Left), "b");
    }

    #[test]
    fn sort_by_targets_the_given_side_and_activates_it() {
        let (_tmp, mut c) = commander();
        c.execute(Command::SortBy(Side::Right, SortKey::Name));
        assert_eq!(c.active(), Side::Right);
        assert!(c.panel(Side::Right).sort().descending);
        assert!(!c.panel(Side::Left).sort().descending);

        let labels: Vec<_> = c
            .panel(Side::Right)
            .entries()
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels, ["..", "c", "b", "a", "f"]);
    }

    #[test]
    fn activate_on_parent_navigates_up() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 1));
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        c.execute(Command::CursorHome);
        assert_eq!(run(&mut c, Command::Activate), Outcome::Done);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
    }

    #[cfg(unix)]
    #[test]
    fn failed_go_up_is_reported_and_leaves_panel_in_place() {
        use std::os::unix::fs::PermissionsExt;
        if nix::unistd::geteuid().is_root() {
            return; // root can read anything, so there is no failure to provoke
        }
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 1));
        run(&mut c, Command::Activate);
        let inside = tmp.path().join("a");
        assert_eq!(c.panel(Side::Left).path(), inside);

        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o000)).unwrap();
        run(&mut c, Command::GoUp);
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).unwrap();

        assert!(c.error().is_some());
        assert_eq!(c.panel(Side::Left).path(), inside);
    }

    #[test]
    fn case_sensitive_sort_applies_to_both_panels() {
        let (_tmp, mut c) = commander();
        c.set_case_sensitive_sort(true);
        assert!(c.panel(Side::Left).sort().case_sensitive);
        assert!(c.panel(Side::Right).sort().case_sensitive);
        c.set_case_sensitive_sort(false);
        assert!(!c.panel(Side::Left).sort().case_sensitive);
    }

    #[test]
    fn selection_commands_act_on_the_active_panel() {
        let (_tmp, mut c) = commander();
        c.execute(Command::CursorDown);
        c.execute(Command::ToggleSelection);
        assert_eq!(c.panel(Side::Left).summary().selected(), 1);
        assert_eq!(under_cursor(&c, Side::Left), "b");

        c.execute(Command::SwitchPanel);
        c.execute(Command::SelectAll);
        assert_eq!(c.panel(Side::Right).summary().selected(), 4);
        assert_eq!(c.panel(Side::Left).summary().selected(), 1);
    }

    #[test]
    fn clear_selection_affects_only_the_given_panel() {
        let (_tmp, mut c) = commander();
        c.execute(Command::SelectAll);
        c.execute(Command::SwitchPanel);
        c.execute(Command::SelectAll);
        let left = c.panel(Side::Left).path().to_path_buf();
        c.clear_selection(Side::Left, &left);
        assert_eq!(c.panel(Side::Left).summary().selected(), 0);
        assert_eq!(c.panel(Side::Right).summary().selected(), 4);
    }

    #[test]
    fn swap_panels_exchanges_directories_and_keeps_active_side() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 3));
        c.execute(Command::CursorTo(Side::Right, 1));
        run(&mut c, Command::Activate);
        c.execute(Command::SwapPanels);
        assert_eq!(c.active(), Side::Right);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        assert_eq!(c.panel(Side::Right).path(), tmp.path());
        // Cursors travel with their panels.
        assert_eq!(under_cursor(&c, Side::Right), "c");
        assert_eq!(under_cursor(&c, Side::Left), "..");
    }

    #[test]
    fn sync_other_panel_shows_same_directory_and_entry() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Right, 1));
        run(&mut c, Command::Activate);
        c.execute(Command::Focus(Side::Left));
        c.execute(Command::CursorTo(Side::Left, 3));
        run(&mut c, Command::SyncOtherPanel);
        assert_eq!(c.panel(Side::Right).path(), tmp.path());
        assert_eq!(under_cursor(&c, Side::Right), "c");
        assert_eq!(c.active(), Side::Left);
    }

    #[test]
    fn reload_picks_up_changes_in_both_panels() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 2));
        fs::create_dir(tmp.path().join("new")).unwrap();
        run(&mut c, Command::Reload);
        let has_new = |side| c.panel(side).entries().iter().any(|e| e.label == "new");
        assert!(has_new(Side::Left) && has_new(Side::Right));
        assert_eq!(under_cursor(&c, Side::Left), "b");
        assert!(c.error().is_none());
    }

    #[test]
    fn reload_of_vanished_directory_is_reported() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 1));
        run(&mut c, Command::Activate);
        fs::remove_dir(tmp.path().join("a")).unwrap();
        run(&mut c, Command::Reload);
        assert!(c.error().is_some());
    }

    #[test]
    fn search_jumps_to_first_match_and_ignores_misses() {
        let (_tmp, mut c) = commander();
        assert_eq!(c.search(), None);
        assert!(c.search_type('B'));
        assert_eq!(under_cursor(&c, Side::Left), "b");
        assert_eq!(c.search(), Some("B"));
        assert!(!c.search_type('z'));
        assert_eq!(c.search(), Some("B"), "a miss keeps the prefix");
        assert!(c.search_backspace());
        assert_eq!(c.search(), None, "empty prefix closes the search");
        assert!(!c.search_backspace());
        assert!(!c.search_type('.'), "never matches ..");
        assert_eq!(c.search(), None);
    }

    #[test]
    fn search_steps_through_matches_until_a_command_ends_it() {
        let (tmp, mut c) = commander();
        fs::create_dir(tmp.path().join("bb")).unwrap();
        run(&mut c, Command::Reload);
        assert!(!c.search_step(true), "no search open");
        c.search_type('b');
        assert_eq!(under_cursor(&c, Side::Left), "b");
        assert!(c.search_step(true));
        assert_eq!(under_cursor(&c, Side::Left), "bb");
        c.search_step(true);
        assert_eq!(under_cursor(&c, Side::Left), "b", "wraps");
        c.search_step(false);
        assert_eq!(under_cursor(&c, Side::Left), "bb");
        assert!(c.search_cancel());
        assert!(!c.search_cancel());

        c.search_type('a');
        c.execute(Command::CursorDown);
        assert_eq!(c.search(), None, "any command ends the search");
    }

    #[test]
    fn rename_moves_cursor_to_new_name_and_refreshes_other_panel() {
        let (tmp, mut c) = commander();
        c.rename(OsStr::new("f"), "g").unwrap();
        c.run_loads_now();
        assert_eq!(under_cursor(&c, Side::Left), "g");
        assert!(tmp.path().join("g").exists());
        let right: Vec<_> = c
            .panel(Side::Right)
            .entries()
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert!(right.contains(&"g") && !right.contains(&"f"));
    }

    #[test]
    fn failed_rename_changes_nothing() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 1));
        let err = c.rename(OsStr::new("a"), "b").unwrap_err();
        c.run_loads_now();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(tmp.path().join("a").is_dir());
        assert_eq!(under_cursor(&c, Side::Left), "a");
    }

    #[test]
    fn create_file_selects_it_in_both_panels_showing_the_directory() {
        let (tmp, mut c) = commander();
        let path = c.create_file("new.txt").unwrap();
        c.run_loads_now();
        assert_eq!(path, tmp.path().join("new.txt"));
        assert_eq!(under_cursor(&c, Side::Left), "new.txt");
        assert!(
            c.panel(Side::Right)
                .entries()
                .iter()
                .any(|e| e.label == "new.txt")
        );
        assert!(c.create_file("a").is_err());
    }

    #[test]
    fn make_directory_selects_new_entry_in_active_panel() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Right, 1));
        run(&mut c, Command::Activate);
        c.make_directory("x/y").unwrap();
        c.run_loads_now();
        assert!(tmp.path().join("a/x/y").is_dir());
        assert_eq!(under_cursor(&c, Side::Right), "x");
        assert!(c.make_directory("x").is_err());
        // The left panel shows another directory and is untouched.
        assert_eq!(under_cursor(&c, Side::Left), "..");
    }

    #[test]
    fn toggle_hidden_applies_to_both_panels() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".dot"), b"").unwrap();
        fs::write(tmp.path().join("f"), b"").unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        let labels = |c: &Commander, side| -> Vec<String> {
            c.panel(side)
                .entries()
                .iter()
                .map(|e| e.label.clone())
                .collect()
        };
        assert!(!c.shows_hidden());
        assert_eq!(labels(&c, Side::Right), ["..", "f"]);

        c.execute(Command::ToggleHidden);
        assert!(c.shows_hidden());
        assert_eq!(labels(&c, Side::Left), ["..", ".dot", "f"]);
        assert_eq!(labels(&c, Side::Right), ["..", ".dot", "f"]);

        c.execute(Command::SwapPanels);
        c.execute(Command::ToggleHidden);
        assert!(!c.panel(Side::Left).shows_hidden() && !c.panel(Side::Right).shows_hidden());
    }

    #[test]
    fn rename_mkdir_and_new_file_are_logged_when_logging_is_on() {
        let (tmp, mut c) = commander();
        let logs = tempfile::tempdir().unwrap();
        c.set_log(Some(Arc::new(OperationLog::open(logs.path()).unwrap())));
        assert!(c.log().is_some());
        let d = tmp.path().display();
        c.rename(OsStr::new("f"), "g").unwrap();
        c.run_loads_now();
        c.rename(OsStr::new("a"), "b").unwrap_err();
        c.run_loads_now();
        c.make_directory("x/y").unwrap();
        c.run_loads_now();
        c.create_file("new.txt").unwrap();
        c.run_loads_now();
        c.create_file("new.txt").unwrap();
        c.run_loads_now(); // existed: not logged
        c.create_file("x").unwrap_err();
        c.run_loads_now();
        let file = fs::read_dir(logs.path()).unwrap().next().unwrap().unwrap();
        let lines: Vec<String> = fs::read_to_string(file.path())
            .unwrap()
            .lines()
            .map(|l| l[20..].to_owned())
            .collect();
        assert_eq!(
            lines,
            [
                format!("rename renamed {d}/f -> {d}/g"),
                format!("rename failed {d}/a: “b” already exists"),
                format!("mkdir created directory {d}/x/y"),
                format!("new file created {d}/new.txt"),
                format!("new file failed {d}/x: “x” is a directory"),
            ]
        );
    }

    #[test]
    fn set_active_picks_the_panel() {
        let tmp = tempfile::tempdir().unwrap();
        let mut commander = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        commander.set_active(Side::Right);
        assert_eq!(commander.active(), Side::Right);
        commander.set_active(Side::Left);
        assert_eq!(commander.active(), Side::Left);
    }

    /// Runs a command and finishes the loads it started, like the app does.
    fn run(c: &mut Commander, command: Command) -> Outcome {
        let outcome = c.execute(command);
        c.run_loads_now();
        outcome
    }

    fn tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        tmp
    }

    /// Cursor on "a" (after "..").
    fn on_a(tmp: &tempfile::TempDir) -> Commander {
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::CursorTo(Side::Left, 1));
        c
    }

    #[test]
    fn navigation_keeps_the_old_listing_until_the_load_finishes() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        let requests = c.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, tmp.path().join("a"));
        assert!(requests[0].fallback.is_none());
        assert!(c.is_pending(requests[0].id));
        assert!(c.finish_load(requests[0].id, read_listing(&requests[0])));
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        assert!(c.panel(Side::Left).loading().is_none());
    }

    #[test]
    fn a_cancelled_load_is_ignored_when_it_finishes() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        let request = c.take_requests().remove(0);
        assert!(c.cancel_load(Side::Left));
        assert!(!c.cancel_load(Side::Left), "nothing left to cancel");
        assert!(!c.finish_load(request.id, read_listing(&request)));
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
    }

    #[test]
    fn a_newer_navigation_replaces_a_pending_one() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate); // left: pending "a"
        // Alt-Z twice from the right panel while the left one loads.
        c.execute(Command::SwitchPanel);
        c.execute(Command::CursorTo(Side::Right, 1));
        c.execute(Command::SyncOtherPanel);
        c.execute(Command::CursorTo(Side::Right, 2));
        c.execute(Command::SyncOtherPanel);
        let requests = c.take_requests();
        assert_eq!(
            requests.len(),
            1,
            "only the newest request for the left panel runs"
        );
        assert_eq!(requests[0].id, c.panel(Side::Left).loading().unwrap().id);
        let result = read_listing(&requests[0]);
        c.finish_load(requests[0].id, result);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert_eq!(c.panel(Side::Left).cursor_entry().unwrap().label, "b");
    }

    #[test]
    fn a_reload_does_not_replace_a_pending_navigation() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        let id = c.panel(Side::Left).loading().unwrap().id;
        c.execute(Command::Reload);
        assert_eq!(c.panel(Side::Left).loading().unwrap().id, id);
        assert!(
            c.panel(Side::Right).loading().is_some(),
            "the other panel reloads"
        );
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
    }

    #[test]
    fn commands_for_a_loading_panel_are_ignored() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        c.execute(Command::CursorDown);
        c.execute(Command::CursorTo(Side::Left, 2));
        assert_eq!(c.panel(Side::Left).cursor(), 1);
        assert!(!c.search_type('b'), "quick search waits too");
        c.execute(Command::SwitchPanel);
        assert_eq!(c.active(), Side::Right);
        c.execute(Command::CursorDown);
        assert_eq!(c.panel(Side::Right).cursor(), 1, "the other panel works");
    }

    #[test]
    fn swapping_panels_keeps_each_load_with_its_panel() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        c.execute(Command::SwapPanels);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Right).path(), tmp.path().join("a"));
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
    }

    #[test]
    fn a_failed_load_keeps_the_listing_and_reports_the_folder() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        std::fs::remove_dir(tmp.path().join("a")).unwrap();
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(
            c.error()
                .unwrap()
                .contains(&*tmp.path().join("a").to_string_lossy())
        );
    }

    #[test]
    fn show_loading_marks_the_indicator_visible() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::Activate);
        let id = c.panel(Side::Left).loading().unwrap().id;
        assert!(!c.panel(Side::Left).loading().unwrap().visible);
        assert!(c.show_loading(id));
        assert!(c.panel(Side::Left).loading().unwrap().visible);
        assert!(!c.show_loading(id + 100));
    }

    #[test]
    fn start_reads_nothing_until_the_requests_run() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let mut c = Commander::start(
            tmp.path().join("a"),
            tmp.path().join("gone/deeper"),
            false,
            home.path().to_path_buf(),
        );
        assert!(!c.panel(Side::Left).is_loaded());
        assert_eq!(c.panel(Side::Right).path(), tmp.path().join("gone/deeper"));
        let requests = c.take_requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|r| r.fallback.as_deref() == Some(home.path()))
        );
        for r in requests {
            let result = read_listing(&r);
            c.finish_load(r.id, result);
        }
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        assert_eq!(
            c.panel(Side::Right).path(),
            tmp.path(),
            "fell back to the parent"
        );
    }

    #[test]
    fn cancelling_the_startup_load_goes_home() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let mut c = Commander::start(
            tmp.path().join("a"),
            tmp.path().join("b"),
            false,
            home.path().to_path_buf(),
        );
        let _hung = c.take_requests();
        assert!(c.cancel_load(Side::Left));
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), home.path());
        assert!(c.panel(Side::Left).is_loaded());
    }

    #[test]
    fn a_second_reload_waits_for_the_first() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::Reload);
        let first = c.panel(Side::Left).loading().unwrap().id;
        c.execute(Command::Reload);
        assert_eq!(c.panel(Side::Left).loading().unwrap().id, first);
        assert_eq!(c.take_requests().len(), 2, "one per panel");
    }

    #[test]
    fn clicking_a_loading_panel_focuses_it_but_double_click_does_nothing() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::SyncOtherPanel); // right panel loading
        c.execute(Command::CursorTo(Side::Right, 2));
        assert_eq!(c.active(), Side::Right, "the click focuses it");
        assert_eq!(c.panel(Side::Right).cursor(), 0, "but moves no cursor");
        c.execute(Command::Activate);
        assert!(
            c.panel(Side::Left).loading().is_none(),
            "the left panel is untouched"
        );
        c.execute(Command::Focus(Side::Left));
        assert_eq!(c.active(), Side::Left);
    }

    #[test]
    fn a_reload_dropped_for_a_cancelled_navigation_runs_afterwards() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::SwitchPanel);
        c.execute(Command::CursorTo(Side::Right, 1));
        c.execute(Command::Activate); // right: navigating into "a"
        c.execute(Command::Focus(Side::Left));
        c.make_directory("new").unwrap(); // right shows the same folder: reload
        let _slow = c.take_requests();
        assert!(c.cancel_load(Side::Right));
        c.run_loads_now();
        let names: Vec<_> = c
            .panel(Side::Right)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect();
        assert!(names.contains(&"new".to_owned()), "{names:?}");
    }

    #[test]
    fn a_reload_requested_during_a_reload_runs_again() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::Reload);
        let requests = c.take_requests();
        // Both reads list the folder now, before the change below.
        let results: Vec<_> = requests.iter().map(|r| (r.id, read_listing(r))).collect();
        let mut results = results.into_iter();
        let (left_id, left) = results.next().unwrap();
        c.finish_load(left_id, left);
        c.make_directory("new").unwrap(); // right's reload is still running
        let (right_id, right) = results.next().unwrap();
        c.finish_load(right_id, right); // the old listing
        c.run_loads_now();
        let names: Vec<_> = c
            .panel(Side::Right)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect();
        assert!(names.contains(&"new".to_owned()), "{names:?}");
    }

    fn labels(c: &Commander, side: Side) -> Vec<String> {
        c.panel(side)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect()
    }

    fn cursor_label(c: &Commander, side: Side) -> String {
        c.panel(side).cursor_entry().unwrap().label.clone()
    }

    #[test]
    fn watch_reload_reads_quietly_and_commands_keep_working() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        fs::write(tmp.path().join("new"), b"").unwrap();
        c.watch_reload(Side::Left);
        assert!(c.panel(Side::Left).loading().is_none(), "no indicator");
        assert!(c.panel(Side::Left).is_refreshing());
        c.execute(Command::CursorDown);
        assert_eq!(cursor_label(&c, Side::Left), "b", "commands still work");
        c.run_loads_now();
        assert!(!c.panel(Side::Left).is_refreshing());
        assert!(labels(&c, Side::Left).contains(&"new".to_owned()));
        assert_eq!(cursor_label(&c, Side::Left), "b");
    }

    #[test]
    fn the_cursor_follows_a_move_made_during_the_quiet_read() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.watch_reload(Side::Left);
        c.execute(Command::CursorDown); // onto "b" while the read runs
        fs::create_dir(tmp.path().join("0")).unwrap(); // shifts every index
        c.run_loads_now();
        assert_eq!(cursor_label(&c, Side::Left), "b");
    }

    #[test]
    fn a_quiet_reload_keeps_the_selection() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::ToggleSelection); // selects "a"
        c.watch_reload(Side::Left);
        c.run_loads_now();
        let selected: Vec<_> = c
            .panel(Side::Left)
            .selection()
            .map(|e| e.label.clone())
            .collect();
        assert_eq!(selected, ["a"]);
    }

    #[test]
    fn a_quiet_reload_keeps_the_search_and_the_error() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        fs::remove_dir(tmp.path().join("a")).unwrap();
        run(&mut c, Command::Activate); // fails: sets the error
        assert!(c.error().is_some());
        assert!(c.search_type('b'));
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(c.search(), Some("b"));
        assert!(c.error().is_some());
    }

    #[test]
    fn a_quiet_request_during_a_read_runs_quietly_afterwards() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::Reload);
        let requests = c.take_requests();
        let results: Vec<_> = requests.iter().map(|r| (r.id, read_listing(r))).collect();
        c.watch_reload(Side::Left);
        assert!(!c.panel(Side::Left).is_refreshing(), "waits for the read");
        fs::write(tmp.path().join("new"), b"").unwrap();
        for (id, result) in results {
            c.finish_load(id, result); // listings from before "new"
        }
        assert!(c.panel(Side::Left).loading().is_none());
        assert!(c.panel(Side::Left).is_refreshing(), "the re-read is quiet");
        c.run_loads_now();
        assert!(labels(&c, Side::Left).contains(&"new".to_owned()));
    }

    #[test]
    fn a_second_change_during_a_quiet_read_reads_once_more() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.watch_reload(Side::Left);
        let first = c.take_requests();
        assert_eq!(first.len(), 1);
        let result = read_listing(&first[0]);
        c.watch_reload(Side::Left);
        assert!(c.take_requests().is_empty(), "no second read at once");
        fs::write(tmp.path().join("new"), b"").unwrap();
        c.finish_load(first[0].id, result);
        assert!(c.panel(Side::Left).is_refreshing());
        c.run_loads_now();
        assert!(labels(&c, Side::Left).contains(&"new".to_owned()));
    }

    #[test]
    fn navigation_replaces_a_quiet_read() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.watch_reload(Side::Left);
        let quiet = c.take_requests();
        c.execute(Command::Activate); // into "a": not blocked
        assert!(!c.panel(Side::Left).is_refreshing());
        let result = read_listing(&quiet[0]);
        assert!(
            !c.finish_load(quiet[0].id, result),
            "the quiet result is dropped"
        );
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
    }

    #[test]
    fn ctrl_r_replaces_a_quiet_read() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.watch_reload(Side::Left);
        c.execute(Command::Reload);
        assert!(!c.panel(Side::Left).is_refreshing());
        assert!(c.panel(Side::Left).loading().is_some());
    }

    #[test]
    fn a_reload_held_back_by_ctrl_r_runs_quietly() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::Reload);
        let requests = c.take_requests();
        c.execute(Command::Reload); // stale
        for r in requests {
            let result = read_listing(&r);
            c.finish_load(r.id, result);
        }
        assert!(c.panel(Side::Left).loading().is_none(), "keys work");
        assert!(c.panel(Side::Left).is_refreshing());
    }

    #[test]
    fn a_vanished_folder_moves_the_panel_to_its_nearest_parent() {
        let tmp = tree();
        fs::create_dir_all(tmp.path().join("a/deep/er")).unwrap();
        let mut c = Commander::new(tmp.path().join("a/deep/er"), tmp.path(), false).unwrap();
        fs::remove_dir_all(tmp.path().join("a/deep")).unwrap();
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        assert_eq!(c.panel(Side::Left).cursor(), 0);
        assert!(c.error().is_none());
        assert!(c.panel(Side::Left).loading().is_none());
        assert!(!c.panel(Side::Left).is_refreshing());
    }

    #[test]
    fn other_read_errors_keep_the_listing_silently() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tree();
        let a = tmp.path().join("a");
        fs::write(a.join("x"), b"").unwrap();
        let mut c = Commander::new(&a, tmp.path(), false).unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&a).is_ok() {
            fs::set_permissions(&a, fs::Permissions::from_mode(0o755)).unwrap();
            return; // root reads anyway
        }
        c.watch_reload(Side::Left);
        c.run_loads_now();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(c.panel(Side::Left).path(), a);
        assert!(labels(&c, Side::Left).contains(&"x".to_owned()));
        assert!(c.error().is_none());
        assert!(!c.panel(Side::Left).is_refreshing());
    }

    #[test]
    fn watch_reload_before_the_first_listing_does_nothing() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let mut c = Commander::start(
            tmp.path().join("a"),
            tmp.path().join("b"),
            false,
            home.path().to_path_buf(),
        );
        c.watch_reload(Side::Left);
        assert!(!c.panel(Side::Left).is_refreshing());
        assert_eq!(c.take_requests().len(), 2, "only the startup reads");
    }

    #[test]
    fn a_quiet_read_has_no_indicator_or_count() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.watch_reload(Side::Left);
        let id = c.take_requests()[0].id;
        assert!(c.is_pending(id));
        assert!(!c.show_loading(id));
        assert_eq!(c.panel_loading_count(id), None);
    }

    #[test]
    fn check_dir_refuses_once_the_panel_left_the_folder() {
        let tmp = tree();
        fs::create_dir(tmp.path().join("a/sub")).unwrap();
        let sub = tmp.path().join("a/sub");
        let mut c = Commander::new(&sub, tmp.path(), false).unwrap();
        assert!(c.check_dir(&sub).is_ok());
        fs::remove_dir(&sub).unwrap();
        c.watch_reload(Side::Left);
        c.run_loads_now();
        let err = c.check_dir(&sub).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains(&*sub.to_string_lossy()));
    }

    #[test]
    fn hiding_a_column_resets_panels_sorted_by_it() {
        let (_tmp, mut c) = commander();
        c.execute(Command::SortBy(Side::Left, SortKey::Owner));
        c.execute(Command::SortBy(Side::Right, SortKey::Size));
        c.hide_columns(&[SortKey::Owner, SortKey::Permissions]);
        assert_eq!(c.panel(Side::Left).sort().key, SortKey::Name);
        assert!(!c.panel(Side::Left).sort().descending);
        assert_eq!(c.panel(Side::Right).sort().key, SortKey::Size, "untouched");
    }

    #[test]
    fn a_hidden_column_cannot_be_sorted_by() {
        let (_tmp, mut c) = commander();
        c.hide_columns(&[SortKey::Modified]);
        c.execute(Command::SortBy(Side::Left, SortKey::Modified));
        assert_eq!(c.panel(Side::Left).sort().key, SortKey::Name);
        assert!(c.shows_column(SortKey::Owner));
        assert!(!c.shows_column(SortKey::Modified));
        c.hide_columns(&[]);
        c.execute(Command::SortBy(Side::Left, SortKey::Modified));
        assert_eq!(
            c.panel(Side::Left).sort().key,
            SortKey::Modified,
            "shown again"
        );
    }
    #[test]
    fn start_tabs_reads_only_the_active_tab_of_each_side() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let left = StartTabs {
            dirs: vec![tmp.path().join("a"), tmp.path().join("b")],
            active: 1,
        };
        let mut c = Commander::start_tabs(
            left,
            StartTabs::one(tmp.path().to_path_buf()),
            false,
            home.path().to_path_buf(),
        );
        let requests = c.take_requests();
        let paths: Vec<_> = requests.iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, [tmp.path().join("b"), tmp.path().to_path_buf()]);
        for r in requests {
            let result = read_listing(&r);
            c.finish_load(r.id, result);
        }
        let tabs = c.tabs(Side::Left);
        assert_eq!((tabs.count(), tabs.index()), (2, 1));
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("b"));
        let first = tabs.get(0).unwrap();
        assert_eq!(first.path(), tmp.path().join("a"));
        assert!(
            !first.is_loaded(),
            "a background tab is read on its first visit"
        );
    }

    #[test]
    fn start_tabs_clamps_the_active_index_and_never_starts_empty() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let c = Commander::start_tabs(
            StartTabs {
                dirs: vec![],
                active: 3,
            },
            StartTabs {
                dirs: vec![tmp.path().join("a")],
                active: 5,
            },
            false,
            home.path().to_path_buf(),
        );
        assert_eq!(c.tabs(Side::Left).count(), 1);
        assert_eq!(c.panel(Side::Left).path(), home.path());
        assert_eq!(c.tabs(Side::Right).index(), 0);
    }
    #[test]
    fn new_tab_copies_the_front_tab_without_its_selection() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::ToggleSelection); // selects "a", cursor on "b"
        c.execute(Command::NewTab);
        let tabs = c.tabs(Side::Left);
        assert_eq!((tabs.count(), tabs.index()), (2, 1));
        assert_eq!(
            tabs.get(0).unwrap().selection().count(),
            1,
            "the original keeps it"
        );
        let panel = c.panel(Side::Left);
        assert_eq!(panel.path(), tmp.path());
        assert_eq!(panel.cursor(), 2);
        assert_eq!(panel.selection().count(), 0);
        assert!(panel.is_refreshing(), "re-read quietly");
        assert!(panel.loading().is_none());
    }

    #[test]
    fn close_tab_keeps_the_last_one_and_shows_a_neighbor() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::CloseTab);
        assert_eq!(c.tabs(Side::Left).count(), 1);
        run(&mut c, Command::NewTab);
        run(&mut c, Command::Activate); // the second tab enters "a"
        c.execute(Command::PrevTab);
        c.execute(Command::CloseTab);
        assert_eq!(c.tabs(Side::Left).count(), 1);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
    }

    #[test]
    fn next_and_prev_tab_wrap_on_the_active_side_only() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::NextTab);
        assert_eq!(c.tabs(Side::Left).index(), 0, "one tab: nothing to do");
        c.execute(Command::NewTab);
        c.execute(Command::NewTab);
        assert_eq!(c.tabs(Side::Left).index(), 2);
        c.execute(Command::NextTab);
        assert_eq!(c.tabs(Side::Left).index(), 0);
        c.execute(Command::PrevTab);
        assert_eq!(c.tabs(Side::Left).index(), 2);
        assert_eq!(c.tabs(Side::Right).count(), 1);
    }

    #[test]
    fn switching_tabs_ends_the_quick_search() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::NewTab);
        assert!(c.search_type('b'));
        c.execute(Command::PrevTab);
        assert_eq!(c.search(), None);
    }

    #[test]
    fn a_tab_coming_to_the_front_rereads_quietly() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        run(&mut c, Command::PrevTab);
        fs::create_dir(tmp.path().join("new")).unwrap();
        c.execute(Command::NextTab);
        assert!(c.panel(Side::Left).is_refreshing());
        assert!(c.panel(Side::Left).loading().is_none(), "no loading state");
        c.run_loads_now();
        assert!(
            c.panel(Side::Left)
                .entries()
                .iter()
                .any(|e| e.label == "new")
        );
    }

    #[test]
    fn a_load_finishing_in_a_background_tab_lands_there() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        c.execute(Command::Activate); // the second tab: pending "a"
        let request = c.take_requests().remove(0);
        c.execute(Command::PrevTab);
        assert!(c.is_pending(request.id));
        assert_eq!(
            c.panel_loading_count(request.id),
            Some(0),
            "nothing read yet"
        );
        assert!(c.finish_load(request.id, read_listing(&request)));
        assert_eq!(
            c.panel(Side::Left).path(),
            tmp.path(),
            "the front tab stays"
        );
        let second = c.tabs(Side::Left).get(1).unwrap();
        assert_eq!(second.path(), tmp.path().join("a"));
    }

    #[test]
    fn a_load_for_a_closed_tab_is_dropped() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        c.execute(Command::Activate);
        let request = c.take_requests().remove(0);
        c.execute(Command::CloseTab);
        assert!(!c.is_pending(request.id));
        assert_eq!(c.panel_loading_count(request.id), None);
        assert!(!c.show_loading(request.id));
        assert!(!c.finish_load(request.id, read_listing(&request)));
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
    }

    #[test]
    fn a_loading_tab_can_be_left_or_closed_but_not_copied() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        c.execute(Command::Activate); // pending
        c.execute(Command::NewTab);
        assert_eq!(
            c.tabs(Side::Left).count(),
            2,
            "Ctrl-T is ignored while loading"
        );
        c.execute(Command::PrevTab);
        assert_eq!(c.tabs(Side::Left).index(), 0);
        c.execute(Command::NextTab);
        assert!(c.panel(Side::Left).loading().is_some(), "still loading");
        c.execute(Command::CloseTab);
        assert_eq!(c.tabs(Side::Left).count(), 1);
        assert!(c.panel(Side::Left).loading().is_none());
    }

    #[test]
    fn select_tab_brings_that_side_and_tab_to_the_front() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        c.execute(Command::SwitchPanel);
        c.execute(Command::SelectTab(Side::Left, 0));
        assert_eq!(c.active(), Side::Left);
        assert_eq!(c.tabs(Side::Left).index(), 0);
        c.execute(Command::SelectTab(Side::Left, 9));
        assert_eq!(c.tabs(Side::Left).index(), 0, "out of range: nothing");
    }

    #[test]
    fn a_restored_tab_is_read_on_its_first_visit_with_the_fallback() {
        let tmp = tree();
        let home = tempfile::tempdir().unwrap();
        let left = StartTabs {
            dirs: vec![tmp.path().to_path_buf(), tmp.path().join("gone/deeper")],
            active: 0,
        };
        let right = StartTabs::one(tmp.path().join("b"));
        let mut c = Commander::start_tabs(left, right, false, home.path().to_path_buf());
        c.run_loads_now();
        c.execute(Command::NextTab);
        assert!(c.panel(Side::Left).loading().is_some(), "a normal load");
        let requests = c.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].fallback.as_deref(), Some(home.path()));
        for r in requests {
            let result = read_listing(&r);
            c.finish_load(r.id, result);
        }
        assert_eq!(c.panel(Side::Left).path(), tmp.path(), "the nearest parent");
    }

    #[test]
    fn a_background_tab_whose_folder_vanished_walks_up() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        run(&mut c, Command::Activate); // the second tab shows "a"
        c.watch_reload(Side::Left); // a quiet re-read of "a" is pending
        c.execute(Command::PrevTab);
        fs::remove_dir(tmp.path().join("a")).unwrap();
        c.run_loads_now();
        assert_eq!(c.tabs(Side::Left).get(1).unwrap().path(), tmp.path());
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(c.error().is_none(), "quiet");
    }

    #[test]
    fn swap_panels_swaps_whole_sides_with_their_tabs() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        run(&mut c, Command::NewTab);
        run(&mut c, Command::Activate); // left: [root, a], "a" in front
        c.execute(Command::SwapPanels);
        assert_eq!(c.tabs(Side::Left).count(), 1);
        assert_eq!(c.tabs(Side::Right).count(), 2);
        assert_eq!(c.tabs(Side::Right).index(), 1);
        assert_eq!(c.panel(Side::Right).path(), tmp.path().join("a"));
    }

    #[test]
    fn settings_reach_background_tabs() {
        let tmp = tree();
        fs::write(tmp.path().join(".dot"), b"").unwrap();
        let mut c = on_a(&tmp);
        c.execute(Command::SortBy(Side::Left, SortKey::Size));
        run(&mut c, Command::NewTab);
        run(&mut c, Command::PrevTab);
        c.hide_columns(&[SortKey::Size]);
        c.set_case_sensitive_sort(true);
        c.execute(Command::ToggleHidden);
        for panel in c.tabs(Side::Left).iter() {
            assert_eq!(panel.sort().key, SortKey::Name);
            assert!(panel.sort().case_sensitive);
            assert!(panel.entries().iter().any(|e| e.label == ".dot"));
        }
    }
    #[test]
    fn clear_selection_needs_the_same_folder_in_front() {
        let tmp = tree();
        let mut c = on_a(&tmp);
        c.execute(Command::ToggleSelection);
        run(&mut c, Command::NewTab);
        c.execute(Command::PrevTab); // the selection's tab is in front again
        c.execute(Command::SwitchPanel);
        c.clear_selection(Side::Left, &tmp.path().join("a"));
        assert_eq!(c.panel(Side::Left).selection().count(), 1, "another folder");
        c.execute(Command::SwitchPanel);
        c.execute(Command::NextTab);
        c.execute(Command::ToggleSelection); // the copy selects too
        c.execute(Command::PrevTab);
        c.clear_selection(Side::Left, tmp.path());
        assert_eq!(c.panel(Side::Left).selection().count(), 0);
        let copy = c.tabs(Side::Left).get(1).unwrap();
        assert_eq!(copy.selection().count(), 1, "other tabs keep theirs");
    }
    #[test]
    fn go_to_navigates_the_active_panel_only() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.execute(Command::SwitchPanel);
        c.go_to(tmp.path().join("b"));
        c.run_loads_now();
        assert_eq!(c.panel(Side::Right).path(), tmp.path().join("b"));
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert_eq!(c.panel(Side::Right).cursor(), 0);
    }

    #[test]
    fn go_to_a_missing_folder_keeps_the_panel_and_reports() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.go_to(tmp.path().join("gone"));
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(c.error().unwrap().contains("gone"));
    }

    #[test]
    fn go_to_ends_the_search_and_waits_for_a_loading_panel() {
        let tmp = tree();
        let mut c = Commander::new(tmp.path(), tmp.path(), false).unwrap();
        c.search_type('a');
        c.go_to(tmp.path().join("a"));
        assert_eq!(c.search(), None);
        // Still loading "a": a second go_to is ignored.
        c.go_to(tmp.path().join("b"));
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
    }

    #[test]
    fn home_is_the_one_given_at_start() {
        let c = Commander::start("/".into(), "/".into(), false, "/home/me".into());
        assert_eq!(c.home(), Path::new("/home/me"));
    }
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use crate::file_ops::Format;
    use crate::test_archives::{T, make_tar, make_zip};
    use std::fs;

    fn run(c: &mut Commander, command: Command) -> Outcome {
        let outcome = c.execute(command);
        c.run_loads_now();
        outcome
    }

    /// Both panels on a folder with `pkg.zip` (src/lib/a.rs, src/main.rs,
    /// README) and a file `f`.
    fn with_zip() -> (tempfile::TempDir, PathBuf, Commander) {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("pkg.zip");
        make_zip(
            &zip,
            &[("src/lib/a.rs", "a"), ("src/main.rs", "m"), ("README", "r")],
        );
        fs::write(tmp.path().join("f"), b"").unwrap();
        let c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        (tmp, zip, c)
    }

    fn put_cursor(c: &mut Commander, label: &str) {
        let side = c.active();
        let ix = c
            .panel(side)
            .entries()
            .iter()
            .position(|e| e.label == label)
            .unwrap();
        c.execute(Command::CursorTo(side, ix));
    }

    fn labels(c: &Commander, side: Side) -> Vec<String> {
        c.panel(side)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect()
    }

    /// `with_zip`, the left panel inside the zip.
    fn inside() -> (tempfile::TempDir, PathBuf, Commander) {
        let (tmp, zip, mut c) = with_zip();
        put_cursor(&mut c, "pkg.zip");
        run(&mut c, Command::Activate);
        (tmp, zip, c)
    }

    #[test]
    fn enter_opens_an_archive_and_moves_inside_without_reading() {
        let (tmp, zip, mut c) = with_zip();
        put_cursor(&mut c, "pkg.zip");
        assert_eq!(run(&mut c, Command::Activate), Outcome::Done);
        let left = c.panel(Side::Left);
        assert_eq!(left.path(), zip);
        assert!(left.in_archive());
        assert_eq!(left.real_dir(), tmp.path());
        assert_eq!(labels(&c, Side::Left), ["..", "src", "README"]);

        put_cursor(&mut c, "src");
        c.execute(Command::Activate);
        assert!(c.take_requests().is_empty(), "no read inside");
        assert_eq!(c.panel(Side::Left).path(), zip.join("src"));
        assert_eq!(c.panel(Side::Left).real_dir(), tmp.path());

        c.execute(Command::GoUp);
        assert!(c.take_requests().is_empty());
        assert_eq!(c.panel(Side::Left).path(), zip);
        assert_eq!(c.panel(Side::Left).cursor_entry().unwrap().label, "src");

        run(&mut c, Command::GoUp);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(!c.panel(Side::Left).in_archive());
        assert_eq!(c.panel(Side::Left).cursor_entry().unwrap().label, "pkg.zip");
    }

    #[test]
    fn parent_entry_at_the_root_leaves_the_archive() {
        let (tmp, _zip, mut c) = inside();
        c.execute(Command::CursorHome);
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(!c.panel(Side::Left).in_archive());
    }

    #[test]
    fn enter_on_a_file_inside_does_nothing() {
        let (_tmp, _zip, mut c) = inside();
        put_cursor(&mut c, "README");
        assert_eq!(c.execute(Command::Activate), Outcome::Done);
        assert!(c.take_requests().is_empty());
    }

    #[test]
    fn a_damaged_archive_shows_an_error_and_the_panel_stays() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("bad.zip"), b"nope").unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        put_cursor(&mut c, "bad.zip");
        run(&mut c, Command::Activate);
        assert!(c.error().is_some());
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(!c.panel(Side::Left).in_archive());
    }

    #[test]
    fn alt_z_and_ctrl_t_share_the_index() {
        let (_tmp, zip, mut c) = inside();
        c.execute(Command::SyncOtherPanel);
        assert!(
            c.take_requests().is_empty(),
            "the other panel takes the index"
        );
        let left = c.panel(Side::Left).archive().unwrap().clone();
        let right = c.panel(Side::Right).archive().unwrap().clone();
        assert!(Arc::ptr_eq(&left, &right));
        assert_eq!(c.panel(Side::Right).path(), zip);
        c.execute(Command::NewTab);
        c.run_loads_now();
        assert_eq!(c.tabs(Side::Left).count(), 2);
        assert!(Arc::ptr_eq(c.panel(Side::Left).archive().unwrap(), &right));
        assert_eq!(c.panel(Side::Left).path(), zip);
    }

    #[test]
    fn quiet_rereads_keep_an_unchanged_index_and_follow_changes() {
        let (_tmp, zip, mut c) = inside();
        put_cursor(&mut c, "src");
        c.execute(Command::Activate);
        let before = c.panel(Side::Left).archive().unwrap().clone();

        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert!(
            Arc::ptr_eq(&before, c.panel(Side::Left).archive().unwrap()),
            "unchanged"
        );
        assert_eq!(c.panel(Side::Left).path(), zip.join("src"));

        make_zip(&zip, &[("src/new.rs", "n"), ("README", "r")]);
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), zip.join("src"));
        assert_eq!(labels(&c, Side::Left), ["..", "new.rs"]);
    }

    #[test]
    fn a_vanished_folder_inside_goes_to_its_nearest_parent() {
        let (_tmp, zip, mut c) = inside();
        put_cursor(&mut c, "src");
        c.execute(Command::Activate);
        make_zip(&zip, &[("README", "changed")]);
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), zip);
    }

    #[test]
    fn a_damaged_rewrite_keeps_the_listing() {
        let (_tmp, zip, mut c) = inside();
        fs::write(&zip, b"half written").unwrap();
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(labels(&c, Side::Left), ["..", "src", "README"]);
        assert!(c.error().is_none(), "quiet");
    }

    #[test]
    fn a_vanished_archive_sends_the_panel_to_its_folder() {
        let (tmp, zip, mut c) = inside();
        fs::remove_file(&zip).unwrap();
        c.watch_reload(Side::Left);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), tmp.path());
        assert!(!c.panel(Side::Left).in_archive());
    }

    #[test]
    fn ctrl_r_inside_rereads_only_a_changed_archive() {
        let (_tmp, zip, mut c) = inside();
        let before = c.panel(Side::Left).archive().unwrap().clone();
        run(&mut c, Command::Reload);
        assert!(Arc::ptr_eq(&before, c.panel(Side::Left).archive().unwrap()));
        assert_eq!(c.panel(Side::Left).path(), zip);
        make_zip(&zip, &[("other", "o")]);
        run(&mut c, Command::Reload);
        assert_eq!(labels(&c, Side::Left), ["..", "other"]);
    }

    #[test]
    fn writes_inside_an_archive_are_refused_and_stray_paths_touch_nothing() {
        let (tmp, _zip, mut c) = inside();
        let refused = [
            c.make_directory("d").map(|_| ()),
            c.create_file("n").map(|_| ()),
            c.rename(OsStr::new("README"), "x"),
        ];
        for result in refused {
            assert_eq!(result.unwrap_err().to_string(), crate::archive::IN_ARCHIVE);
        }
        // A command that slipped past the refusals gets a path through the
        // archive file, which can't exist.
        put_cursor(&mut c, "README");
        let target = c.panel(Side::Left).cursor_path();
        assert!(fs::symlink_metadata(&target).is_err());
        let targets: Vec<_> = c
            .panel(Side::Left)
            .targets()
            .iter()
            .map(|e| c.panel(Side::Left).path().join(&e.name))
            .collect();
        assert!(targets.iter().all(|p| fs::symlink_metadata(p).is_err()));
        assert!(tmp.path().join("f").exists());
        assert!(!tmp.path().join("d").exists() && !tmp.path().join("n").exists());
    }

    #[test]
    fn a_tar_gz_opens_too() {
        let tmp = tempfile::tempdir().unwrap();
        make_tar(
            &tmp.path().join("src.tar.gz"),
            Format::TarGz,
            &[T::File("x", "1", 0o644)],
        );
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        put_cursor(&mut c, "src.tar.gz");
        run(&mut c, Command::Activate);
        assert_eq!(labels(&c, Side::Left), ["..", "x"]);
    }

    #[test]
    fn a_folder_named_like_an_archive_is_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("dir.zip")).unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        put_cursor(&mut c, "dir.zip");
        run(&mut c, Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("dir.zip"));
        assert!(!c.panel(Side::Left).in_archive());
    }

    #[test]
    fn real_dir_is_the_path_outside_archives() {
        let (tmp, _zip, c) = with_zip();
        assert_eq!(c.panel(Side::Left).real_dir(), tmp.path());
        assert!(c.panel(Side::Left).archive().is_none());
    }
}

#[cfg(test)]
mod archive_fix_tests {
    use super::*;
    use crate::test_archives::make_zip;
    use std::fs;

    fn put_cursor(c: &mut Commander, label: &str) {
        let side = c.active();
        let ix = c
            .panel(side)
            .entries()
            .iter()
            .position(|e| e.label == label)
            .unwrap();
        c.execute(Command::CursorTo(side, ix));
    }

    fn labels(c: &Commander, side: Side) -> Vec<String> {
        c.panel(side)
            .entries()
            .iter()
            .map(|e| e.label.clone())
            .collect()
    }

    /// Both panels on a folder with `pkg.zip` (src/a) and a folder `d`; the
    /// left panel inside the zip.
    fn inside() -> (tempfile::TempDir, PathBuf, Commander) {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("pkg.zip");
        make_zip(&zip, &[("src/a", "a")]);
        fs::create_dir(tmp.path().join("d")).unwrap();
        let mut c = Commander::new(tmp.path(), tmp.path(), true).unwrap();
        put_cursor(&mut c, "pkg.zip");
        c.execute(Command::Activate);
        c.run_loads_now();
        (tmp, zip, c)
    }

    #[test]
    fn alt_z_onto_a_loading_panel_replaces_its_read() {
        let (_tmp, zip, mut c) = inside();
        c.execute(Command::SwitchPanel);
        put_cursor(&mut c, "d");
        c.execute(Command::Activate); // a read of `d`, still pending
        c.execute(Command::SwitchPanel);
        c.execute(Command::SyncOtherPanel);
        assert!(c.panel(Side::Right).loading().is_none(), "replaced");
        c.run_loads_now();
        assert_eq!(c.panel(Side::Right).path(), zip, "the read of d is dropped");
    }

    #[test]
    fn moving_inside_keeps_a_pending_reread() {
        let (_tmp, zip, mut c) = inside();
        make_zip(&zip, &[("src/a", "a"), ("src/new", "n")]);
        c.watch_reload(Side::Left); // quiet re-read, not run yet
        put_cursor(&mut c, "src");
        c.execute(Command::Activate);
        c.run_loads_now();
        assert_eq!(c.panel(Side::Left).path(), zip.join("src"));
        assert_eq!(labels(&c, Side::Left), ["..", "a", "new"]);
    }
}
