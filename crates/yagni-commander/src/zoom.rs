//! Zoom: two levels, each a base size in px. The UI level is gpui-component's
//! theme `font_size`, which its `Root` makes the window's rem size on every
//! frame; our views size everything in rems (`rems_from_px`), so it all
//! scales. The viewer level is the rem size of the viewer's content
//! (`RemScope`). The technique is the Zed editor's (UI font size, editor
//! font size); the code is ours.

use gpui_kit::component::Theme as ComponentTheme;
use gpui_kit::{App, Global, Pixels, Rems, px, rems};

pub const DEFAULT: f32 = 16.0;
pub const MIN: f32 = 10.0;
pub const MAX: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    /// The main window, dialogs and popups.
    pub ui: f32,
    /// The content of every viewer window.
    pub viewer: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            ui: DEFAULT,
            viewer: DEFAULT,
        }
    }
}

impl Global for Zoom {}

impl Zoom {
    /// The levels in use; the defaults until `main` installs the global.
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().copied().unwrap_or_default()
    }
}

/// A level within `MIN..=MAX`; a non-finite one is the default.
pub fn clamp(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

/// `size` moved by `delta` px, kept within the range.
pub fn step(size: f32, delta: f32) -> f32 {
    clamp(size + delta)
}

/// A length given in px at the 16 px base, as rems: it scales with the level.
pub fn rems_from_px(px: f32) -> Rems {
    rems(px / DEFAULT)
}

/// `px` at the 16 px base, in px at `level`: for arithmetic on sizes.
pub fn scaled(px: f32, level: f32) -> f32 {
    px * level / DEFAULT
}

/// A dialog width given in px at the 16 px base, in px at the UI level:
/// gpui-component's `Dialog::w` takes only `Pixels`.
pub fn dialog_width(px_at_base: f32, cx: &App) -> Pixels {
    px(scaled(px_at_base, Zoom::get(cx).ui))
}

/// Makes the UI level the rem size of every window (through gpui-component's
/// `Root`) and redraws them.
pub fn apply_ui(cx: &mut App) {
    let ui = Zoom::get(cx).ui;
    ComponentTheme::global_mut(cx).font_size = px(ui);
    cx.refresh_windows();
}

/// Moves the UI level by `delta` px, applies and remembers it.
pub fn change_ui(delta: f32, cx: &mut App) {
    set(
        Zoom {
            ui: step(Zoom::get(cx).ui, delta),
            ..Zoom::get(cx)
        },
        cx,
    );
}

pub fn reset_ui(cx: &mut App) {
    set(
        Zoom {
            ui: DEFAULT,
            ..Zoom::get(cx)
        },
        cx,
    );
}

/// Moves the viewer level by `delta` px for every viewer window, and
/// remembers it.
pub fn change_viewer(delta: f32, cx: &mut App) {
    set(
        Zoom {
            viewer: step(Zoom::get(cx).viewer, delta),
            ..Zoom::get(cx)
        },
        cx,
    );
}

pub fn reset_viewer(cx: &mut App) {
    set(
        Zoom {
            viewer: DEFAULT,
            ..Zoom::get(cx)
        },
        cx,
    );
}

fn set(zoom: Zoom, cx: &mut App) {
    cx.set_global(zoom);
    crate::app_state::AppState::remember_zoom(zoom, cx);
    apply_ui(cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rems;

    #[test]
    fn steps_by_one_px_within_the_range() {
        assert_eq!(step(16.0, 1.0), 17.0);
        assert_eq!(step(16.0, -1.0), 15.0);
        assert_eq!(step(MAX, 1.0), MAX);
        assert_eq!(step(MIN, -1.0), MIN);
    }

    #[test]
    fn clamps_bad_values() {
        assert_eq!(clamp(1000.0), MAX);
        assert_eq!(clamp(-5.0), MIN);
        assert_eq!(clamp(f32::NAN), DEFAULT);
        assert_eq!(clamp(f32::INFINITY), DEFAULT);
        assert_eq!(clamp(20.0), 20.0);
    }

    #[test]
    fn px_become_rems_of_the_16_px_base() {
        assert_eq!(rems_from_px(16.0), rems(1.0));
        assert_eq!(rems_from_px(22.0), rems(1.375));
        assert_eq!(scaled(22.0, 24.0), 33.0);
    }
}
