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

/// Everything a frontend can ask the core to do. Frontends own the keymap
/// and translate key presses into these.
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
}

/// Result of a command that the frontend may need to act on.
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
    /// Last error, shown by the frontend until the next successful command.
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

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Runs a command. I/O errors are stored in [`Commander::error`] rather than
    /// returned, since the frontend's only sensible reaction is to display them.
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
}
