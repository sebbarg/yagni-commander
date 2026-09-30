use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs_ops;
use crate::oplog::OperationLog;
use crate::panel::{Activation, Panel};
use crate::quick_search::QuickSearch;
use crate::sort::SortKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// Result of a command that the UI may need to act on.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Enter was pressed on a file.
    OpenFile(PathBuf),
}

/// Dual-panel state.
#[derive(Debug)]
pub struct Commander {
    left: Panel,
    right: Panel,
    active: Side,
    /// Last error, shown by the UI until the next successful command.
    error: Option<String>,
    /// Quick search in the active panel. Any command ends it.
    search: QuickSearch,
    /// Where file changes are logged, if logging is on.
    log: Option<Arc<OperationLog>>,
}

impl Commander {
    pub fn new(
        left: impl AsRef<Path>,
        right: impl AsRef<Path>,
        show_hidden: bool,
    ) -> io::Result<Self> {
        Ok(Self {
            left: Panel::open(left, show_hidden)?,
            right: Panel::open(right, show_hidden)?,
            active: Side::Left,
            error: None,
            search: QuickSearch::default(),
            log: None,
        })
    }

    pub fn panel(&self, side: Side) -> &Panel {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    fn panel_mut(&mut self, side: Side) -> &mut Panel {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    pub fn active(&self) -> Side {
        self.active
    }

    /// Whether hidden entries are shown. Always the same for both panels.
    pub fn shows_hidden(&self) -> bool {
        self.left.shows_hidden()
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

    /// F2: renames `from` in the active panel's directory to `to`, then
    /// puts the cursor on it.
    pub fn rename(&mut self, from: &OsStr, to: &str) -> io::Result<()> {
        let dir = self.panel(self.active).path().to_path_buf();
        let source = dir.join(from);
        let result = fs_ops::rename(&dir, from, to);
        self.note("rename", &source, &result, |source| {
            format!("renamed {} -> {}", source.display(), dir.join(to).display())
        });
        result?;
        self.refresh_after_change(OsStr::new(to))
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
        let dir = self.panel(self.active).path().to_path_buf();
        let result = fs_ops::make_directory(&dir, name);
        self.note("mkdir", &dir.join(name), &result, |path| {
            format!("created directory {}", path.display())
        });
        self.refresh_after_change(&result?)
    }

    /// Shift-F4: creates an empty file at `name` (a name or a relative path
    /// like `a/b/c.txt`) in the active panel's directory, or keeps an existing
    /// one, puts the cursor on it (or on its first directory) and returns its
    /// path.
    pub fn create_file(&mut self, name: &str) -> io::Result<PathBuf> {
        let dir = self.panel(self.active).path().to_path_buf();
        let result = fs_ops::create_file(&dir, name);
        // Only a file that was created counts as touched.
        if !matches!(&result, Ok(new) if !new.created) {
            self.note("new file", &dir.join(name), &result, |path| {
                format!("created {}", path.display())
            });
        }
        let new = result?;
        self.refresh_after_change(&new.first)?;
        Ok(new.path)
    }

    /// Re-reads the active panel with the cursor on `select`, and the other
    /// panel too if it shows the same directory.
    fn refresh_after_change(&mut self, select: &OsStr) -> io::Result<()> {
        let active = self.active;
        self.panel_mut(active).reload(Some(select))?;
        if self.left.path() == self.right.path() {
            self.panel_mut(active.other()).reload(None)?;
        }
        Ok(())
    }

    /// Deselects everything in one panel, e.g. after its selection was copied.
    pub fn clear_selection(&mut self, side: Side) {
        self.panel_mut(side).clear_selection();
    }

    /// Applies the case-sensitive sorting setting to both panels.
    pub fn set_case_sensitive_sort(&mut self, case_sensitive: bool) {
        self.left.set_case_sensitive(case_sensitive);
        self.right.set_case_sensitive(case_sensitive);
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Runs a command. I/O errors are stored in [`Commander::error`] rather than
    /// returned, since the UI's only sensible reaction is to display them.
    pub fn execute(&mut self, command: Command) -> Outcome {
        self.error = None;
        self.search.clear();
        let active = self.active;
        let panel = self.panel_mut(active);
        let result = match command {
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
            Command::GoUp => panel.go_up(),
            Command::ToggleSelection => {
                panel.toggle_selection();
                Ok(())
            }
            Command::SelectAll => {
                panel.select_all();
                Ok(())
            }
            Command::SwapPanels => {
                std::mem::swap(&mut self.left, &mut self.right);
                Ok(())
            }
            Command::SyncOtherPanel => {
                let path = panel.path().to_path_buf();
                let select = panel.cursor_entry().map(|e| e.name.clone());
                self.panel_mut(active.other()).show(path, select.as_deref())
            }
            Command::Reload => self.left.reload(None).and(self.right.reload(None)),
            Command::ToggleHidden => {
                let show = !self.shows_hidden();
                self.left.set_show_hidden(show);
                self.right.set_show_hidden(show);
                Ok(())
            }
            Command::SortBy(side, key) => {
                self.active = side;
                self.panel_mut(side).sort_by(key);
                Ok(())
            }
            Command::Activate => match panel.activate() {
                Ok(Activation::File(path)) => return Outcome::OpenFile(path),
                Ok(_) => Ok(()),
                Err(e) => Err(e),
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
    use std::fs;

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
        c.execute(Command::Activate);
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

        c.execute(Command::Activate);
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
            c.execute(Command::Activate),
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
        c.execute(Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("b"));

        c.execute(Command::GoUp);
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
        c.execute(Command::Activate);
        assert_eq!(c.panel(Side::Left).path(), tmp.path().join("a"));
        c.execute(Command::CursorHome);
        assert_eq!(c.execute(Command::Activate), Outcome::Done);
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
        c.execute(Command::Activate);
        let inside = tmp.path().join("a");
        assert_eq!(c.panel(Side::Left).path(), inside);

        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o000)).unwrap();
        c.execute(Command::GoUp);
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
        c.clear_selection(Side::Left);
        assert_eq!(c.panel(Side::Left).summary().selected(), 0);
        assert_eq!(c.panel(Side::Right).summary().selected(), 4);
    }

    #[test]
    fn swap_panels_exchanges_directories_and_keeps_active_side() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 3));
        c.execute(Command::CursorTo(Side::Right, 1));
        c.execute(Command::Activate);
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
        c.execute(Command::Activate);
        c.execute(Command::Focus(Side::Left));
        c.execute(Command::CursorTo(Side::Left, 3));
        c.execute(Command::SyncOtherPanel);
        assert_eq!(c.panel(Side::Right).path(), tmp.path());
        assert_eq!(under_cursor(&c, Side::Right), "c");
        assert_eq!(c.active(), Side::Left);
    }

    #[test]
    fn reload_picks_up_changes_in_both_panels() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 2));
        fs::create_dir(tmp.path().join("new")).unwrap();
        c.execute(Command::Reload);
        let has_new = |side| c.panel(side).entries().iter().any(|e| e.label == "new");
        assert!(has_new(Side::Left) && has_new(Side::Right));
        assert_eq!(under_cursor(&c, Side::Left), "b");
        assert!(c.error().is_none());
    }

    #[test]
    fn reload_of_vanished_directory_is_reported() {
        let (tmp, mut c) = commander();
        c.execute(Command::CursorTo(Side::Left, 1));
        c.execute(Command::Activate);
        fs::remove_dir(tmp.path().join("a")).unwrap();
        c.execute(Command::Reload);
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
        c.execute(Command::Reload);
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
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(tmp.path().join("a").is_dir());
        assert_eq!(under_cursor(&c, Side::Left), "a");
    }

    #[test]
    fn create_file_selects_it_in_both_panels_showing_the_directory() {
        let (tmp, mut c) = commander();
        let path = c.create_file("new.txt").unwrap();
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
        c.execute(Command::Activate);
        c.make_directory("x/y").unwrap();
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
        c.rename(OsStr::new("a"), "b").unwrap_err();
        c.make_directory("x/y").unwrap();
        c.create_file("new.txt").unwrap();
        c.create_file("new.txt").unwrap(); // existed: not logged
        c.create_file("x").unwrap_err();
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
}
