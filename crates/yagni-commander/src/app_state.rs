//! What the app remembers across runs (the state file): the main window's
//! position and size, each side's tab folders and the active side, whether
//! hidden files are shown, and where the last viewer was.

use std::path::{Path, PathBuf};

use gpui_kit::{App, Bounds, Entity, Global, Pixels, Size, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use yagni_commander_core::{Commander, Side, StartTabs, storage};

/// A new window's size without a display to go by.
const DEFAULT_WIDTH: f32 = 1200.0;
const DEFAULT_HEIGHT: f32 = 800.0;

/// A new window takes these shares of its display's width and height, at
/// least `MIN_WIDTH` x `MIN_HEIGHT` and never more than the display. On
/// Wayland gpui divides the mode by the output's integer scale, which a
/// compositor rounds up from a fractional one, so the display can seem
/// smaller than it is (1920x1080 at 1.5 gives 960x540, not 1280x720); the
/// minimum keeps the window usable there.
const WIDTH_SHARE: f32 = 0.6;
const HEIGHT_SHARE: f32 = 0.75;
const MIN_WIDTH: f32 = 900.0;
const MIN_HEIGHT: f32 = 600.0;

/// The size of a new window on `display`.
fn default_size(display: Option<&Bounds<Pixels>>) -> Size<Pixels> {
    let Some(display) = display else {
        return size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT));
    };
    let side = |room: Pixels, share: f32, min: f32| {
        let room = f32::from(room);
        px((room * share).max(min).min(room))
    };
    size(
        side(display.size.width, WIDTH_SHARE, MIN_WIDTH),
        side(display.size.height, HEIGHT_SHARE, MIN_HEIGHT),
    )
}

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub window: Option<SavedWindow>,
    pub show_hidden: bool,
    /// Where the last viewer window was.
    pub viewer: Option<SavedWindow>,
    /// Each side's tab folders and the index of the tab in front, at the
    /// last quit.
    pub left_tabs: Vec<PathBuf>,
    pub right_tabs: Vec<PathBuf>,
    pub left_tab: usize,
    pub right_tab: usize,
    /// The active side at the last quit.
    pub active: Option<Side>,
    /// The find dialog's "Skip folders" choice; `None` for the defaults.
    pub find_skip: Option<Vec<String>>,
    /// One folder per side, from state files written before tabs; read
    /// when the tab lists are missing, never written.
    #[serde(skip_serializing)]
    pub left: Option<PathBuf>,
    #[serde(skip_serializing)]
    pub right: Option<PathBuf>,
    /// Zoom levels in px (see `zoom`); 16 when missing.
    #[serde(default = "default_zoom")]
    pub ui_zoom: f32,
    #[serde(default = "default_zoom")]
    pub viewer_zoom: f32,
}

fn default_zoom() -> f32 {
    crate::zoom::DEFAULT
}

impl Default for State {
    fn default() -> Self {
        Self {
            window: None,
            show_hidden: false,
            viewer: None,
            left_tabs: Vec::new(),
            right_tabs: Vec::new(),
            left_tab: 0,
            right_tab: 0,
            active: None,
            find_skip: None,
            left: None,
            right: None,
            ui_zoom: crate::zoom::DEFAULT,
            viewer_zoom: crate::zoom::DEFAULT,
        }
    }
}

impl State {
    /// The saved zoom levels, clamped (a hand-edited file may hold anything).
    pub fn zoom(&self) -> crate::zoom::Zoom {
        crate::zoom::Zoom {
            ui: crate::zoom::clamp(self.ui_zoom),
            viewer: crate::zoom::clamp(self.viewer_zoom),
        }
    }

    /// The tabs to open: the saved ones (or an old file's single folder,
    /// else `home`), with a command-line folder in place of the folder of
    /// that side's active tab (left first, then right). Nothing is read
    /// here; the background read falls back to a parent or home if needed.
    pub fn startup_tabs(&self, args: &[String], home: &Path) -> (StartTabs, StartTabs) {
        let side =
            |tabs: &[PathBuf], old: &Option<PathBuf>, active: usize, arg: Option<&String>| {
                let mut dirs = if tabs.is_empty() {
                    vec![old.clone().unwrap_or_else(|| home.to_path_buf())]
                } else {
                    tabs.to_vec()
                };
                let active = active.min(dirs.len() - 1);
                if let Some(arg) = arg {
                    dirs[active] = PathBuf::from(arg);
                }
                StartTabs { dirs, active }
            };
        (
            side(&self.left_tabs, &self.left, self.left_tab, args.first()),
            side(&self.right_tabs, &self.right, self.right_tab, args.get(1)),
        )
    }
}

/// The latest state, kept current while the app runs and written to the
/// state file on quit.
#[derive(Default)]
pub struct AppState {
    path: Option<PathBuf>,
    pub state: State,
}

impl Global for AppState {}

impl AppState {
    pub fn remember_zoom(zoom: crate::zoom::Zoom, cx: &mut App) {
        let state = &mut cx.global_mut::<Self>().state;
        state.ui_zoom = zoom.ui;
        state.viewer_zoom = zoom.viewer;
    }
}

impl SavedWindow {
    /// What to save for a window now at `bounds`. A maximized or fullscreen
    /// window's size is the compositor's, not the user's (Hyprland reports a
    /// lone tiled window as maximized at the display's size), so it keeps the
    /// size saved before, else `default`; restoring then maximizes over that
    /// size, and un-maximizing or floating the window gives it back.
    pub fn remember(
        previous: Option<SavedWindow>,
        bounds: WindowBounds,
        default: Bounds<Pixels>,
    ) -> Self {
        match bounds {
            WindowBounds::Windowed(_) => Self::from_bounds(bounds),
            WindowBounds::Maximized(_) | WindowBounds::Fullscreen(_) => Self {
                maximized: true,
                ..previous.unwrap_or_else(|| Self::from_bounds(WindowBounds::Windowed(default)))
            },
        }
    }

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

/// Whether `display` is too small for the default shares, which would come
/// out under the minimum (below 1500 wide or 800 tall).
fn is_small(display: &Bounds<Pixels>) -> bool {
    f32::from(display.size.width) * WIDTH_SHARE < MIN_WIDTH
        || f32::from(display.size.height) * HEIGHT_SHARE < MIN_HEIGHT
}

/// The display a new window opens on: the primary one (on Wayland, which has
/// none, the first one).
fn default_display(cx: &App) -> Option<Bounds<Pixels>> {
    cx.primary_display()
        .or_else(|| cx.displays().into_iter().next())
        .map(|d| d.bounds())
}

/// A new window: the default size, centered on its display.
fn default_bounds(cx: &App) -> Bounds<Pixels> {
    match default_display(cx) {
        Some(display) => centered_default(&display),
        None => Bounds::centered(None, default_size(None), cx),
    }
}

fn centered_default(display: &Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::centered_at(display.center(), default_size(Some(display)))
}

/// The first window on `display`: maximized on a small one, so the window
/// manager fits it to the room its panels leave (a window the display's size
/// would sit under them), else the default size; either way the default
/// bounds are what un-maximizing gives.
fn first_window(display: &Bounds<Pixels>) -> WindowBounds {
    let bounds = centered_default(display);
    if is_small(display) {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
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
            .unwrap_or_else(|| match default_display(cx) {
                Some(display) => first_window(&display),
                None => WindowBounds::Windowed(default_bounds(cx)),
            })
    }

    pub fn remember_window(bounds: WindowBounds, cx: &mut App) {
        let default = default_bounds(cx);
        let state = &mut cx.global_mut::<Self>().state;
        state.window = Some(SavedWindow::remember(state.window, bounds, default));
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
        let default = default_bounds(cx);
        let state = &mut cx.global_mut::<Self>().state;
        state.viewer = Some(SavedWindow::remember(state.viewer, bounds, default));
    }

    pub fn remember_panels(commander: &Entity<Commander>, cx: &mut App) {
        let commander = commander.read(cx);
        let side = |side: Side| {
            let tabs = commander.tabs(side);
            let dirs: Vec<PathBuf> = tabs.iter().map(|p| p.real_dir().to_path_buf()).collect();
            (dirs, tabs.index())
        };
        let (left_tabs, left_tab) = side(Side::Left);
        let (right_tabs, right_tab) = side(Side::Right);
        let active = commander.active();
        let state = &mut cx.global_mut::<Self>().state;
        state.left_tabs = left_tabs;
        state.left_tab = left_tab;
        state.right_tabs = right_tabs;
        state.right_tab = right_tab;
        state.active = Some(active);
        state.left = None;
        state.right = None;
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

    #[test]
    fn zoom_levels_round_trip_and_default_to_16() {
        let state: State = toml::from_str("").unwrap();
        assert_eq!((state.ui_zoom, state.viewer_zoom), (16.0, 16.0));
        let state = State {
            ui_zoom: 20.0,
            viewer_zoom: 12.0,
            ..State::default()
        };
        let back: State = toml::from_str(&toml::to_string(&state).unwrap()).unwrap();
        assert_eq!((back.ui_zoom, back.viewer_zoom), (20.0, 12.0));
    }

    #[test]
    fn bad_zoom_values_are_clamped() {
        for (text, want) in [("nan", 16.0), ("-5.0", 10.0), ("1000.0", 32.0)] {
            let state: State =
                toml::from_str(&format!("ui_zoom = {text}\nviewer_zoom = {text}\n")).unwrap();
            assert_eq!(
                state.zoom(),
                crate::zoom::Zoom {
                    ui: want,
                    viewer: want
                },
                "{text}"
            );
        }
    }

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
    fn a_maximized_window_keeps_the_size_saved_before() {
        let before = saved(100.0, 50.0);
        let screen = WindowBounds::Maximized(display());
        let now = SavedWindow::remember(Some(before), screen, display());
        assert_eq!(
            now,
            SavedWindow {
                maximized: true,
                ..before
            }
        );
        let fullscreen = WindowBounds::Fullscreen(display());
        assert_eq!(
            SavedWindow::remember(Some(before), fullscreen, display()),
            now
        );
        // Restoring maximizes over the earlier size.
        let restored = now.restore(&[display()]).unwrap();
        assert_eq!(restored, WindowBounds::Maximized(before.bounds()));
    }

    #[test]
    fn a_window_maximized_from_the_start_keeps_the_default() {
        // Hyprland: a lone tiled window is maximized from its first frame.
        let default = saved(200.0, 100.0).bounds();
        let screen = WindowBounds::Maximized(display());
        let now = SavedWindow::remember(None, screen, default);
        assert_eq!(
            now,
            SavedWindow {
                maximized: true,
                ..saved(200.0, 100.0)
            }
        );
    }

    #[test]
    fn an_ordinary_window_saves_its_own_size() {
        let window = saved(10.0, 20.0);
        let now = SavedWindow::remember(
            Some(saved(0.0, 0.0)),
            WindowBounds::Windowed(window.bounds()),
            display(),
        );
        assert_eq!(now, window);
    }

    fn display_of(width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(width), px(height)))
    }

    #[test]
    fn the_default_size_is_a_share_of_the_display() {
        let want = |w: f32, h: f32| size(px(w), px(h));
        assert_eq!(default_size(Some(&display())), want(1152.0, 810.0));
        assert_eq!(
            default_size(Some(&display_of(3840.0, 2160.0))),
            want(2304.0, 1620.0)
        );
        // At least 900x600 ...
        assert_eq!(
            default_size(Some(&display_of(1280.0, 720.0))),
            want(900.0, 600.0)
        );
        // ... but never more than the display: 1920x1080 at 1.5, which
        // gpui sees on Wayland as 960x540.
        assert_eq!(
            default_size(Some(&display_of(960.0, 540.0))),
            want(900.0, 540.0)
        );
        assert_eq!(
            default_size(Some(&display_of(800.0, 500.0))),
            want(800.0, 500.0)
        );
        assert_eq!(default_size(None), want(1200.0, 800.0));
    }

    #[test]
    fn the_first_window_on_a_small_display_is_maximized() {
        for (width, height) in [
            (1280.0, 720.0),
            (1366.0, 768.0),
            (1440.0, 900.0),
            (1920.0, 790.0),
        ] {
            let display = display_of(width, height);
            assert_eq!(
                first_window(&display),
                WindowBounds::Maximized(centered_default(&display)),
                "{width}x{height}"
            );
        }
        for (width, height) in [(1500.0, 800.0), (1920.0, 1080.0), (3840.0, 2160.0)] {
            let display = display_of(width, height);
            assert_eq!(
                first_window(&display),
                WindowBounds::Windowed(centered_default(&display)),
                "{width}x{height}"
            );
        }
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
            find_skip: Some(vec!["bin".into()]),
            window: Some(saved(10.0, 20.0)),
            show_hidden: true,
            viewer: Some(saved(30.0, 40.0)),
            left_tabs: vec!["/a/b".into(), "/d".into()],
            right_tabs: vec!["/c".into()],
            left_tab: 1,
            right_tab: 0,
            active: Some(Side::Right),
            left: None,
            right: None,
            ui_zoom: 20.0,
            viewer_zoom: 12.0,
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

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn startup_uses_saved_tabs_without_reading_them() {
        let state = State {
            left_tabs: vec!["/net/nas/gone".into(), "/a".into()],
            left_tab: 1,
            right_tabs: vec!["/b".into()],
            ..State::default()
        };
        let (left, right) = state.startup_tabs(&[], Path::new("/home/u"));
        assert_eq!(
            left,
            StartTabs {
                dirs: vec!["/net/nas/gone".into(), "/a".into()],
                active: 1
            }
        );
        assert_eq!(right, StartTabs::one("/b".into()));
    }

    #[test]
    fn an_old_state_file_gives_one_tab_per_side() {
        let state: State = toml::from_str("left = \"/a\"\nright = \"/b\"\n").unwrap();
        let (left, right) = state.startup_tabs(&[], Path::new("/home/u"));
        assert_eq!(
            (left, right),
            (StartTabs::one("/a".into()), StartTabs::one("/b".into()))
        );
    }

    #[test]
    fn startup_without_saved_paths_opens_home() {
        let (left, right) = State::default().startup_tabs(&[], Path::new("/home/u"));
        assert_eq!(left, StartTabs::one("/home/u".into()));
        assert_eq!(right, left);
    }

    #[test]
    fn a_saved_tab_index_out_of_range_is_clamped() {
        let state = State {
            left_tabs: vec!["/a".into(), "/b".into()],
            left_tab: 9,
            ..State::default()
        };
        let (left, _) = state.startup_tabs(&args(&["x"]), Path::new("/home/u"));
        assert_eq!(
            left,
            StartTabs {
                dirs: vec!["/a".into(), "x".into()],
                active: 1
            }
        );
    }

    #[test]
    fn arguments_replace_the_active_tabs_folder() {
        let state = State {
            left_tabs: vec!["/a".into(), "/b".into()],
            left_tab: 0,
            right_tabs: vec!["/c".into()],
            ..State::default()
        };
        let (left, right) = state.startup_tabs(&args(&["x", "y"]), Path::new("/home/u"));
        assert_eq!(
            left,
            StartTabs {
                dirs: vec!["x".into(), "/b".into()],
                active: 0
            }
        );
        assert_eq!(right, StartTabs::one("y".into()));
        // One argument: the right side keeps its saved tabs.
        let (_, right) = state.startup_tabs(&args(&["x"]), Path::new("/home/u"));
        assert_eq!(right, StartTabs::one("/c".into()));
    }
}
