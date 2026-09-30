//! Colors, stored as a gpui global so any view can read them.
//! Hardcoded Tokyo Night (Omarchy's default) until themes become configurable.

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Rgba, rgb};

/// The same palette for gpui-component's widgets (dialogs, inputs, menus).
const COMPONENT_THEME: &str = include_str!("../assets/tokyo-night.json");
const COMPONENT_THEME_NAME: &str = "Tokyo Night";

pub struct Theme {
    pub window_bg: Rgba,
    pub panel_bg: Rgba,
    pub header_bg: Rgba,
    pub header_active_bg: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub text_on_accent: Rgba,
    pub dir: Rgba,
    pub symlink: Rgba,
    pub accent: Rgba,
    pub cursor_inactive_bg: Rgba,
    /// Text of selected entries, and the cursor bar on a selected entry.
    pub selected: Rgba,
    pub border: Rgba,
    pub error: Rgba,
}

impl Global for Theme {}

impl Theme {
    pub fn tokyo_night() -> Self {
        Self {
            window_bg: rgb(0x16161e),
            panel_bg: rgb(0x1a1b26),
            header_bg: rgb(0x1f2335),
            header_active_bg: rgb(0x2f3549),
            text: rgb(0xc0caf5),
            text_dim: rgb(0x565f89),
            text_on_accent: rgb(0x1a1b26),
            dir: rgb(0x7aa2f7),
            symlink: rgb(0x7dcfff),
            accent: rgb(0x7aa2f7),
            cursor_inactive_bg: rgb(0x292e42),
            selected: rgb(0xff9e64),
            border: rgb(0x292e42),
            error: rgb(0xf7768e),
        }
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

/// Installs our palette as gpui-component's dark theme and activates it.
/// Call after `gpui_kit::init`. Colors the JSON leaves out keep the
/// component library's dark defaults.
pub fn apply_component_theme(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    registry
        .load_themes_from_str(COMPONENT_THEME)
        .expect("bundled theme is valid");
    let config = registry.themes()[COMPONENT_THEME_NAME].clone();
    ComponentTheme::global_mut(cx).dark_theme = config;
    ComponentTheme::change(ThemeMode::Dark, None, cx);
}
