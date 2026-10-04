//! The bundled fonts: the icon font (Symbols Nerd Font Mono, Nerd Fonts
//! v3.5.1) and the monospace font of the F3 viewer (JetBrains Mono NL v2.304,
//! the variant without ligatures, so `!=` or `->` show as the characters they
//! are).

use std::borrow::Cow;

use gpui_kit::App;

const ICONS: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNL-Regular.ttf");

/// The family name inside `JetBrainsMonoNL-Regular.ttf`, set as the theme's
/// monospace font.
pub const MONO_FAMILY: &str = "JetBrains Mono NL";

/// Makes `yagni_commander_core::icons::FONT_FAMILY` and `MONO_FAMILY`
/// available to the views. A failure only costs the look (icons fall back to
/// a generic glyph, text to a system font), so it is logged.
pub fn register(cx: &mut App) {
    if let Err(e) = cx
        .text_system()
        .add_fonts(vec![Cow::Borrowed(ICONS), Cow::Borrowed(MONO)])
    {
        log::warn!("bundled fonts not loaded: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(font: &'static [u8]) -> Option<String> {
        ttf_parser::Face::parse(font, 0)
            .unwrap()
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
            .and_then(|n| n.to_string())
    }

    #[test]
    fn the_mono_font_family_is_the_one_the_theme_asks_for() {
        assert_eq!(family(MONO).as_deref(), Some(MONO_FAMILY));
    }

    #[test]
    fn the_mono_font_has_an_m_so_gpui_keeps_it() {
        // gpui's Linux text system drops any face without a glyph for 'm'.
        let face = ttf_parser::Face::parse(MONO, 0).unwrap();
        assert!(face.glyph_index('m').is_some());
    }
}
