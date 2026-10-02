//! The bundled icon font (Symbols Nerd Font Mono, Nerd Fonts v3.5.1).

use std::borrow::Cow;

use gpui_kit::App;

const FONT: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono-Regular.ttf");

/// Makes `yagni_commander_core::icons::FONT_FAMILY` available to the views.
/// A failure only costs the icons (a fallback glyph shows), so it is logged.
pub fn register(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(vec![Cow::Borrowed(FONT)]) {
        log::warn!("icon font not loaded: {e:#}");
    }
}
