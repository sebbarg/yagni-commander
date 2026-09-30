use std::io;
use std::path::{Path, PathBuf};

use crate::panel::{Activation, Panel};
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
}

impl Commander {
    pub fn new(left: impl AsRef<Path>, right: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            left: Panel::open(left)?,
            right: Panel::open(right)?,
            active: Side::Left,
            error: None,
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
        let mut c = Commander::new(tmp.path(), tmp.path()).unwrap();

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
        let mut c = Commander::new(tmp.path(), tmp.path()).unwrap();
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
        let mut c = Commander::new(tmp.path(), tmp.path()).unwrap();
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
        let c = Commander::new(tmp.path(), tmp.path()).unwrap();
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
        assert!(Commander::new(&missing, tmp.path()).is_err());
        assert!(Commander::new(tmp.path(), &missing).is_err());
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
}
