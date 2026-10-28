//! Color themes. A theme is a TOML file of colors named by role (see
//! `assets/themes/`). The active theme is a gpui global, and the same colors
//! are fed to gpui-component so its dialogs and inputs match.
//!
//! Views take every color from [`Colors`]; never construct colors elsewhere
//! (enforced by `clippy.toml`).

use gpui_kit::component::{Theme as ComponentTheme, ThemeConfig, ThemeConfigColors, ThemeMode};
use gpui_kit::{App, BoxShadow, Global, Rgba, SharedString, Styled, point, px};
use serde::Deserialize;

const TOKYO_NIGHT: &str = include_str!("../assets/themes/tokyo-night.toml");
const GRUVBOX_DARK: &str = include_str!("../assets/themes/gruvbox-dark.toml");
const EVERFOREST_DARK: &str = include_str!("../assets/themes/everforest-dark.toml");
const CATPPUCCIN_LATTE: &str = include_str!("../assets/themes/catppuccin-latte.toml");
const CLASSIC: &str = include_str!("../assets/themes/classic.toml");

/// The built-in themes: (id, file), Tokyo Night (the default) first, then
/// the other dark ones, then the light ones. The id is what the config's
/// `theme` key holds.
pub const BUILTIN: [(&str, &str); 5] = [
    ("tokyo-night", TOKYO_NIGHT),
    ("gruvbox-dark", GRUVBOX_DARK),
    ("everforest-dark", EVERFOREST_DARK),
    ("catppuccin-latte", CATPPUCCIN_LATTE),
    ("classic", CLASSIC),
];

/// (id, display name) of every built-in, in `BUILTIN` order.
pub fn ids_and_names() -> Vec<(&'static str, String)> {
    BUILTIN
        .iter()
        .map(|(id, _)| {
            (
                *id,
                Theme::builtin(id).expect("bundled theme is valid").name,
            )
        })
        .collect()
}

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
    /// Panel list background; also lists inside dialogs, and text fields in light themes.
    pub panel_bg: Rgba,
    /// Dialog background, applied by `themed_dialog`. Dark themes raise
    /// dialogs like popups (`popup_bg`); the light ones keep their own.
    pub dialog_bg: Rgba,
    /// Path header and footer of the inactive panel.
    pub header_bg: Rgba,
    /// Path header of the active panel.
    pub header_active_bg: Rgba,
    /// Popups (hotlist, mounts, Open with) and the menus: a clear step
    /// apart from the panels they open over.
    pub popup_bg: Rgba,
    /// Edges on the raised surfaces (popups, menus, and dialogs in dark
    /// themes): the popups' own edge, text fields, buttons and our dialog
    /// controls (`field_frame`), dividers. Brighter than `border` in dark
    /// themes, where a quiet edge disappears.
    pub raised_border: Rgba,
    /// Edges on the panels: tabs, the panel frame, lists inside dialogs.
    pub border: Rgba,
    /// Default text, e.g. file names.
    pub text: Rgba,
    /// Secondary text that is read: column details and titles, footers,
    /// status lines, hints and labels. Quieter than `text`, still easy to read.
    pub text_secondary: Rgba,
    /// Text meant to recede: the inactive side's path header and tabs,
    /// placeholders. Still 4.5:1 against its backgrounds.
    pub text_dim: Rgba,
    pub directory: Rgba,
    pub symlink: Rgba,
    /// The viewer's `·` for control characters and invalid bytes, dimmer
    /// than `text`.
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

/// Installs `theme` and redraws every window (viewers included).
pub(crate) fn switch_theme(theme: Theme, cx: &mut App) {
    theme.install(cx);
    cx.refresh_windows();
}

impl Theme {
    pub fn builtin(id: &str) -> Option<Self> {
        BUILTIN
            .iter()
            .find(|(builtin, _)| *builtin == id)
            .map(|(_, text)| Self::parse(text).expect("bundled theme is valid"))
    }

    /// The theme for a config's `theme` id (none: the default), and whether
    /// the id was unknown (then the default too).
    pub fn named(id: Option<&str>) -> (Self, bool) {
        match id.map(Self::builtin) {
            None => (Self::default(), false),
            Some(Some(theme)) => (theme, false),
            Some(None) => (Self::default(), true),
        }
    }

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
        // gpui-component draws only on raised surfaces (menus, dialogs and
        // its controls), so its edges, the menus' separators among them, are
        // ours there; `border` stays the panels'.
        colors.border = hex(c.raised_border);
        colors.caret = hex(c.text);
        colors.ring = hex(c.accent);
        colors.selection = hex(with_alpha(c.accent, 0.35));
        // The menu and dropdown cursor (also the hover of a few widgets).
        colors.accent = hex(menu_cursor(c));
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
        colors.popover = hex(c.popup_bg);
        colors.popover_foreground = hex(c.text);
        // Text fields' and buttons' edges, and in dark mode their tint too:
        // `border` disappears on a raised dialog.
        colors.input = hex(c.raised_border);
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
        // Links (the About box's), in a color tested on dialogs.
        colors.link = hex(c.info);

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

/// The frame of our popups (hotlist, mounts, Open with): `popup_bg`, the
/// `raised_border`, round corners and a shadow, so they stand out from the
/// panels behind them.
pub fn popup_frame<E: Styled>(element: E, theme: &Theme) -> E {
    element
        .bg(theme.colors.popup_bg)
        .border_1()
        .border_color(theme.colors.raised_border)
        .rounded(crate::zoom::rems_from_px(4.0))
        .shadow(theme.popup_shadow())
}

/// The frame of our own controls in dialogs (the dropdown, the find
/// dialog's "Skip folders"), drawn like gpui-component's text fields: the
/// `input` edge, and in dark mode its tint, in light mode `panel_bg`. This
/// copies gpui-component's `Theme::input_background` rule; an upgrade that
/// changes it shows only in the smoke screenshots.
pub fn field_frame<E: Styled>(element: E, theme: &Theme) -> E {
    let c = &theme.colors;
    let fill = match theme.mode {
        Mode::Dark => with_alpha(c.raised_border, 0.3),
        Mode::Light => c.panel_bg,
    };
    element
        .bg(fill)
        .border_1()
        .border_color(c.raised_border)
        .rounded(crate::zoom::rems_from_px(6.0))
}

impl Theme {
    /// Under a popup: black, stronger on dark themes, where a faint shadow
    /// vanishes. Not a role: no theme needs another hue.
    pub fn popup_shadow(&self) -> Vec<BoxShadow> {
        let alpha = match self.mode {
            Mode::Dark => 0.6,
            Mode::Light => 0.25,
        };
        vec![BoxShadow {
            color: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: alpha,
            }
            .into(),
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(14.0),
            spread_radius: px(0.0),
            inset: false,
        }]
    }
}

/// The cursor bar in gpui-component's menus (drawn on `popup_bg`): a
/// touch of the accent, so `text` stays readable on it.
fn menu_cursor(c: &Colors) -> Rgba {
    mix(c.popup_bg, c.accent, 0.15)
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

    /// WCAG relative luminance of an opaque color.
    fn luminance(c: Rgba) -> f32 {
        let channel = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    /// WCAG contrast ratio, from 1 (none) to 21 (black on white).
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn contrast_is_measured_like_wcag() {
        assert!((contrast(color("#000000"), color("#ffffff")) - 21.0).abs() < 0.01);
        assert!((contrast(color("#777777"), color("#ffffff")) - 4.48).abs() < 0.01);
    }

    /// Every text color against every background it is drawn on reaches
    /// WCAG AA for normal text (4.5:1); icons and other non-text marks 3:1.
    #[test]
    fn text_is_readable_on_its_backgrounds() {
        for (id, _) in BUILTIN {
            let c = Theme::builtin(id).unwrap().colors;
            let text_on = |fg: Rgba, bgs: &[Rgba]| -> Vec<(Rgba, Rgba, f32)> {
                bgs.iter().map(|&bg| (fg, bg, 4.5)).collect()
            };
            let pairs: Vec<(Rgba, Rgba, f32)> = [
                text_on(
                    c.text,
                    &[
                        c.window_bg,
                        c.panel_bg,
                        c.dialog_bg,
                        c.header_bg,
                        c.header_active_bg,
                        c.popup_bg,
                        c.cursor_inactive_bg,
                    ],
                ),
                // Column details, column titles, footers, status lines, hints, labels.
                text_on(
                    c.text_secondary,
                    &[
                        c.window_bg,
                        c.panel_bg,
                        c.dialog_bg,
                        c.header_bg,
                        c.popup_bg,
                        c.cursor_inactive_bg,
                    ],
                ),
                // The inactive side's path header and tabs, placeholders.
                text_on(c.text_dim, &[c.panel_bg, c.dialog_bg, c.header_bg]),
                // The viewer's dim marks.
                text_on(c.hidden, &[c.panel_bg, c.cursor_inactive_bg]),
                // File names, also under the inactive cursor bar.
                text_on(c.directory, &[c.panel_bg, c.cursor_inactive_bg]),
                text_on(c.symlink, &[c.panel_bg, c.cursor_inactive_bg]),
                text_on(c.selected, &[c.panel_bg, c.cursor_inactive_bg]),
                text_on(c.text_on_accent, &[c.accent]),
                text_on(c.text_on_selected, &[c.selected]),
                // The status line and dialogs.
                text_on(c.error, &[c.window_bg, c.panel_bg, c.dialog_bg]),
                text_on(c.warning, &[c.panel_bg, c.dialog_bg]),
                text_on(c.success, &[c.panel_bg, c.dialog_bg]),
                text_on(c.info, &[c.panel_bg, c.dialog_bg]),
                // The active side's folder glyph.
                vec![(c.accent, c.header_active_bg, 3.0)],
            ]
            .concat();
            let failing: Vec<_> = pairs
                .iter()
                .filter(|(fg, bg, min)| contrast(*fg, *bg) < *min)
                .map(|(fg, bg, min)| {
                    format!(
                        "{} on {}: {:.2} < {min}",
                        to_hex(*fg),
                        to_hex(*bg),
                        contrast(*fg, *bg)
                    )
                })
                .collect();
            assert!(
                failing.is_empty(),
                "{id}: too little contrast: {failing:#?}"
            );
        }
    }

    #[test]
    fn every_builtin_parses_with_its_name() {
        let names: Vec<_> = ids_and_names();
        assert_eq!(
            names,
            vec![
                ("tokyo-night", "Tokyo Night".to_owned()),
                ("gruvbox-dark", "Gruvbox Dark".to_owned()),
                ("everforest-dark", "Everforest Dark".to_owned()),
                ("catppuccin-latte", "Catppuccin Latte".to_owned()),
                ("classic", "Classic".to_owned()),
            ]
        );
        assert_eq!(Theme::builtin("classic").unwrap().mode, Mode::Light);
        assert_eq!(Theme::builtin("gruvbox-dark").unwrap().mode, Mode::Dark);
        assert_eq!(Theme::builtin("everforest-dark").unwrap().mode, Mode::Dark);
        assert_eq!(
            Theme::builtin("catppuccin-latte").unwrap().mode,
            Mode::Light
        );
    }

    #[test]
    fn named_falls_back_to_tokyo_night() {
        assert_eq!(Theme::named(None).0.name, "Tokyo Night");
        assert!(!Theme::named(None).1);
        assert_eq!(Theme::named(Some("classic")).0.name, "Classic");
        let (theme, unknown) = Theme::named(Some("solarized"));
        assert_eq!((theme.name.as_str(), unknown), ("Tokyo Night", true));
    }

    /// Dark dialogs share the popups' color, so every floating surface
    /// looks alike (owner, 2026-10-28); the light themes keep their own.
    #[test]
    fn dark_dialogs_are_raised_like_popups() {
        for (id, _) in BUILTIN {
            let theme = Theme::builtin(id).unwrap();
            let c = &theme.colors;
            if theme.mode == Mode::Dark {
                assert_eq!(c.dialog_bg, c.popup_bg, "{id}");
            }
            assert!(
                contrast(c.dialog_bg, c.panel_bg) >= 1.05,
                "{id}: lists and fields don't stand out on a dialog"
            );
        }
    }

    /// With `border` as their edge, fields and buttons vanished on Tokyo
    /// Night's and Gruvbox's raised dialogs (`border` was `popup_bg`).
    #[test]
    fn dialog_controls_have_visible_edges() {
        for (id, _) in BUILTIN {
            let theme = Theme::builtin(id).unwrap();
            let edge = color(&theme.component_config().colors.input.unwrap());
            assert!(
                contrast(edge, theme.colors.dialog_bg) >= 1.4,
                "{id}: control edges too faint on dialogs"
            );
        }
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
        assert_eq!(config.colors.border, Some(to_hex(c.raised_border)));
        assert_eq!(config.colors.primary, Some(to_hex(c.accent)));
        assert_eq!(
            config.colors.primary_foreground,
            Some(to_hex(c.text_on_accent))
        );
        assert_eq!(config.colors.danger, Some(to_hex(c.error)));
        assert_eq!(config.colors.list_active, Some("#7aa2f733".into()));
        assert_ne!(config.colors.primary_hover, config.colors.primary);
    }

    /// Popups were once drawn in the path header's color, close to the
    /// panels': the owner found them hard to see (2026-10-27).
    #[test]
    fn popups_stand_out_from_the_panels() {
        for (id, _) in BUILTIN {
            let c = Theme::builtin(id).unwrap().colors;
            assert!(
                contrast(c.popup_bg, c.panel_bg) >= 1.1,
                "{id}: popup_bg too close to panel_bg"
            );
            assert!(
                contrast(c.raised_border, c.popup_bg) >= 1.5,
                "{id}: raised_border too faint"
            );
        }
    }

    /// Everforest Dark once drew the menu cursor in the menu's own color.
    #[test]
    fn menu_cursor_stands_out_and_keeps_text_readable() {
        for (id, _) in BUILTIN {
            let c = Theme::builtin(id).unwrap().colors;
            let cursor = menu_cursor(&c);
            assert!(
                contrast(cursor, c.popup_bg) >= 1.25,
                "{id}: cursor too faint"
            );
            assert!(contrast(c.text, cursor) >= 4.5, "{id}: text unreadable");
        }
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
