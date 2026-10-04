//! Color themes. A theme is a TOML file of colors named by role (see
//! `assets/themes/`). The active theme is a gpui global, and the same colors
//! are fed to gpui-component so its dialogs and inputs match.
//!
//! Views take every color from [`Colors`]; never construct colors elsewhere
//! (enforced by `clippy.toml`).

use gpui_kit::component::{Theme as ComponentTheme, ThemeConfig, ThemeConfigColors, ThemeMode};
use gpui_kit::{App, Global, Rgba, SharedString};
use serde::Deserialize;

const TOKYO_NIGHT: &str = include_str!("../assets/themes/tokyo-night.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    pub mode: Mode,
    pub colors: Colors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// Every background role has a matching text role, so light and dark themes
/// both stay readable.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// Behind the panels and the status line.
    pub window_bg: Rgba,
    /// Panel list background; also dialogs and popups.
    pub panel_bg: Rgba,
    /// Path header and footer of the inactive panel.
    pub header_bg: Rgba,
    /// Path header of the active panel.
    pub header_active_bg: Rgba,
    pub border: Rgba,
    /// Default text, e.g. file names.
    pub text: Rgba,
    /// Secondary text: column details, hints, inactive headers.
    pub text_dim: Rgba,
    pub directory: Rgba,
    pub symlink: Rgba,
    /// Names of hidden entries (dot files), dimmer than `text`.
    pub hidden: Rgba,
    /// Focus color: the active panel's cursor bar and border, primary buttons.
    pub accent: Rgba,
    pub text_on_accent: Rgba,
    /// Cursor bar in the inactive panel.
    pub cursor_inactive_bg: Rgba,
    /// Text of selected entries, and the cursor bar on a selected entry.
    pub selected: Rgba,
    pub text_on_selected: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
    pub info: Rgba,
}

impl Global for Theme {}

impl From<Mode> for ThemeMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Dark => ThemeMode::Dark,
            Mode::Light => ThemeMode::Light,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::parse(TOKYO_NIGHT).expect("bundled theme is valid")
    }
}

impl Theme {
    pub fn parse(toml: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml)
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Makes this the active theme, for our views and for gpui-component.
    /// Call after `gpui_kit::init`.
    pub fn install(self, cx: &mut App) {
        let config = std::rc::Rc::new(self.component_config());
        let mode = ThemeMode::from(self.mode);
        let component = ComponentTheme::global_mut(cx);
        match mode {
            ThemeMode::Dark => component.dark_theme = config,
            ThemeMode::Light => component.light_theme = config,
        }
        ComponentTheme::change(mode, None, cx);
        cx.set_global(self);
    }

    /// gpui-component's theme, derived from our roles. Colors not set here
    /// keep the component library's defaults for the mode.
    fn component_config(&self) -> ThemeConfig {
        let c = &self.colors;
        let hex = |color: Rgba| Some(to_hex(color));
        let hover = |color: Rgba| hex(mix(color, c.text, 0.15));
        let pressed = |color: Rgba| hex(mix(color, c.window_bg, 0.2));
        let mut colors = ThemeConfigColors::default();
        colors.background = hex(c.panel_bg);
        colors.foreground = hex(c.text);
        colors.border = hex(c.border);
        colors.caret = hex(c.text);
        colors.ring = hex(c.accent);
        colors.selection = hex(with_alpha(c.accent, 0.35));
        colors.accent = hex(c.cursor_inactive_bg);
        colors.accent_foreground = hex(c.text);
        colors.primary = hex(c.accent);
        colors.primary_foreground = hex(c.text_on_accent);
        colors.primary_hover = hover(c.accent);
        colors.primary_active = pressed(c.accent);
        colors.secondary = hex(c.cursor_inactive_bg);
        colors.secondary_foreground = hex(c.text);
        colors.secondary_hover = hover(c.cursor_inactive_bg);
        colors.secondary_active = pressed(c.cursor_inactive_bg);
        colors.muted = hex(c.header_bg);
        colors.muted_foreground = hex(c.text_dim);
        colors.popover = hex(c.header_bg);
        colors.popover_foreground = hex(c.text);
        colors.input = hex(c.border);
        colors.list = hex(c.panel_bg);
        colors.list_active = hex(with_alpha(c.accent, 0.2));
        colors.list_active_border = hex(c.accent);
        colors.title_bar = hex(c.window_bg);
        colors.danger = hex(c.error);
        colors.danger_foreground = hex(c.error);
        colors.warning = hex(c.warning);
        colors.warning_foreground = hex(c.warning);
        colors.success = hex(c.success);
        colors.success_foreground = hex(c.success);
        colors.info = hex(c.info);
        colors.info_foreground = hex(c.info);

        ThemeConfig {
            name: SharedString::from(self.name.clone()),
            mode: self.mode.into(),
            colors,
            mono_font_family: Some(crate::fonts::MONO_FAMILY.into()),
            ..Default::default()
        }
    }
}

/// Linear blend: `t = 0` is `a`, `t = 1` is `b`. Keeps `a`'s alpha.
fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: a.a,
    }
}

fn with_alpha(color: Rgba, a: f32) -> Rgba {
    Rgba { a, ..color }
}

/// `#rrggbbaa`, the form gpui-component's theme config accepts.
fn to_hex(color: Rgba) -> SharedString {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b),
        byte(color.a)
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(hex: &str) -> Rgba {
        Rgba::try_from(hex).unwrap()
    }

    #[test]
    fn bundled_theme_parses() {
        let theme = Theme::default();
        assert_eq!(theme.name, "Tokyo Night");
        assert_eq!(theme.mode, Mode::Dark);
        assert_eq!(theme.colors.accent, color("#7aa2f7"));
        assert_eq!(theme.colors.selected, color("#ff9e64"));
    }

    #[test]
    fn missing_colors_and_unknown_keys_are_rejected() {
        let without_selected = TOKYO_NIGHT.replace("selected = \"#ff9e64\"\n", "");
        assert!(Theme::parse(&without_selected).is_err());
        let typo = TOKYO_NIGHT.replace("selected =", "selcted =");
        assert!(Theme::parse(&typo).is_err());
        let bad_mode = TOKYO_NIGHT.replace("mode = \"dark\"", "mode = \"dim\"");
        assert!(Theme::parse(&bad_mode).is_err());
    }

    #[test]
    fn hex_round_trips_with_alpha() {
        assert_eq!(to_hex(color("#7aa2f7")), "#7aa2f7ff");
        assert_eq!(to_hex(color("#7aa2f733")), "#7aa2f733");
    }

    #[test]
    fn mix_blends_linearly_and_keeps_alpha() {
        let black = color("#000000");
        let white = color("#ffffff80");
        assert_eq!(mix(black, white, 0.0), black);
        assert_eq!(to_hex(mix(black, white, 1.0)), "#ffffffff");
        assert_eq!(to_hex(mix(black, white, 0.5)), "#808080ff");
    }

    #[test]
    fn component_theme_is_derived_from_roles() {
        let theme = Theme::default();
        let config = theme.component_config();
        let c = &theme.colors;
        assert_eq!(config.name, "Tokyo Night");
        assert_eq!(config.mode, ThemeMode::Dark);
        assert_eq!(config.colors.background, Some(to_hex(c.panel_bg)));
        assert_eq!(config.colors.primary, Some(to_hex(c.accent)));
        assert_eq!(
            config.colors.primary_foreground,
            Some(to_hex(c.text_on_accent))
        );
        assert_eq!(config.colors.danger, Some(to_hex(c.error)));
        assert_eq!(config.colors.list_active, Some("#7aa2f733".into()));
        assert_ne!(config.colors.primary_hover, config.colors.primary);
    }

    #[test]
    fn monospace_text_uses_the_bundled_font() {
        let config = Theme::default().component_config();
        assert_eq!(
            config.mono_font_family.as_deref(),
            Some(crate::fonts::MONO_FAMILY)
        );
    }
}
