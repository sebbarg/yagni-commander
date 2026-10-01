//! What the app remembers across runs (the state file): the main window's
//! position and size, and whether hidden files are shown.

use std::path::PathBuf;

use gpui_kit::{App, Bounds, Global, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use yagni_commander_core::storage;

const DEFAULT_WIDTH: f32 = 1200.0;
const DEFAULT_HEIGHT: f32 = 800.0;

/// Window geometry in logical pixels, as saved in the state file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

/// The state file's contents. Missing keys take their defaults, so older
/// files keep working.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub window: Option<SavedWindow>,
    pub show_hidden: bool,
    /// Where the last viewer window was.
    pub viewer: Option<SavedWindow>,
}

/// The latest state, kept current while the app runs and written to the
/// state file on quit.
#[derive(Default)]
pub struct AppState {
    path: Option<PathBuf>,
    pub state: State,
}

impl Global for AppState {}

impl SavedWindow {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let (inner, maximized) = match bounds {
            WindowBounds::Windowed(b) => (b, false),
            WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
        };
        Self {
            x: f32::from(inner.origin.x),
            y: f32::from(inner.origin.y),
            width: f32::from(inner.size.width),
            height: f32::from(inner.size.height),
            maximized,
        }
    }

    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }

    /// The saved geometry if it is sane and still overlaps one of `displays`
    /// (a monitor may have been unplugged since).
    pub fn restore(&self, displays: &[Bounds<Pixels>]) -> Option<WindowBounds> {
        let sane =
            self.width >= 200.0 && self.height >= 150.0 && self.x.is_finite() && self.y.is_finite();
        let bounds = self.bounds();
        if !sane || !displays.iter().any(|d| d.intersects(&bounds)) {
            return None;
        }
        Some(if self.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        })
    }
}

impl AppState {
    /// Loads the state file. Problems are logged and treated as "no saved
    /// state"; losing window geometry is not worth bothering the user.
    pub fn load() -> Self {
        let path = storage::state_file();
        let state = path
            .as_deref()
            .and_then(|path| {
                storage::load::<State>(path)
                    .inspect_err(|e| log::warn!("ignoring saved state: {e}"))
                    .ok()
            })
            .unwrap_or_default();
        Self { path, state }
    }

    /// Where to open the main window: the saved geometry if it still fits
    /// the connected displays, otherwise centered at the default size.
    pub fn initial_bounds(&self, cx: &App) -> WindowBounds {
        let displays: Vec<_> = cx.displays().iter().map(|d| d.bounds()).collect();
        self.state
            .window
            .and_then(|saved| saved.restore(&displays))
            .unwrap_or_else(|| {
                WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)),
                    cx,
                ))
            })
    }

    pub fn remember_window(bounds: WindowBounds, cx: &mut App) {
        cx.global_mut::<Self>().state.window = Some(SavedWindow::from_bounds(bounds));
    }

    /// Where to open a viewer: where the last one was, if that still fits the
    /// displays, otherwise over the main window.
    pub fn viewer_bounds(&self, main: WindowBounds, cx: &App) -> WindowBounds {
        let displays: Vec<_> = cx.displays().iter().map(|d| d.bounds()).collect();
        self.state
            .viewer
            .and_then(|saved| saved.restore(&displays))
            .unwrap_or(main)
    }

    pub fn remember_viewer(bounds: WindowBounds, cx: &mut App) {
        cx.global_mut::<Self>().state.viewer = Some(SavedWindow::from_bounds(bounds));
    }

    pub fn remember_show_hidden(show_hidden: bool, cx: &mut App) {
        cx.global_mut::<Self>().state.show_hidden = show_hidden;
    }

    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(e) = storage::save(path, &self.state) {
            log::warn!("cannot save state: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)))
    }

    fn saved(x: f32, y: f32) -> SavedWindow {
        SavedWindow {
            x,
            y,
            width: 800.0,
            height: 600.0,
            maximized: false,
        }
    }

    #[test]
    fn round_trips_windowed_and_maximized_bounds() {
        let window = saved(100.0, 50.0);
        let restored = window.restore(&[display()]).unwrap();
        assert_eq!(restored, WindowBounds::Windowed(window.bounds()));
        assert_eq!(SavedWindow::from_bounds(restored), window);

        let maximized = SavedWindow {
            maximized: true,
            ..window
        };
        let restored = maximized.restore(&[display()]).unwrap();
        assert!(matches!(restored, WindowBounds::Maximized(_)));
        assert_eq!(SavedWindow::from_bounds(restored), maximized);
    }

    #[test]
    fn fullscreen_is_remembered_as_maximized() {
        let bounds = WindowBounds::Fullscreen(saved(0.0, 0.0).bounds());
        assert!(SavedWindow::from_bounds(bounds).maximized);
    }

    #[test]
    fn off_screen_or_tiny_windows_are_not_restored() {
        assert!(saved(5000.0, 5000.0).restore(&[display()]).is_none());
        assert!(saved(100.0, 100.0).restore(&[]).is_none());
        let tiny = SavedWindow {
            width: 10.0,
            ..saved(100.0, 100.0)
        };
        assert!(tiny.restore(&[display()]).is_none());
        assert!(saved(f32::NAN, 0.0).restore(&[display()]).is_none());
    }

    #[test]
    fn partially_visible_window_is_restored() {
        assert!(saved(1800.0, 1000.0).restore(&[display()]).is_some());
    }

    #[test]
    fn state_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.toml");
        let state = State {
            window: Some(saved(10.0, 20.0)),
            show_hidden: true,
            viewer: Some(saved(30.0, 40.0)),
        };
        AppState {
            path: Some(path.clone()),
            state: state.clone(),
        }
        .save();
        let loaded: State = storage::load(&path).unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn missing_keys_take_defaults() {
        let state: State = toml::from_str("").unwrap();
        assert_eq!(state, State::default());
        assert!(!state.show_hidden, "hidden files are hidden by default");
    }
}
