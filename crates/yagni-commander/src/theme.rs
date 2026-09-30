//! Colors, stored as a gpui global so any view can read them.
//! Hardcoded Tokyo Night (Omarchy's default) until themes become configurable.

use gpui::{App, Global, Rgba, rgb};

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
            border: rgb(0x292e42),
            error: rgb(0xf7768e),
        }
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}
