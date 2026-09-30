//! Colors shared by all frontends so they render the same design.
//! Hardcoded Tokyo Night (Omarchy's default) until themes become configurable.

/// 0xRRGGBB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u32);

impl Rgb {
    pub fn channels(self) -> (u8, u8, u8) {
        ((self.0 >> 16) as u8, (self.0 >> 8) as u8, self.0 as u8)
    }
}

pub struct Palette {
    pub window_bg: Rgb,
    pub panel_bg: Rgb,
    pub header_bg: Rgb,
    pub header_active_bg: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub text_on_accent: Rgb,
    pub dir: Rgb,
    pub symlink: Rgb,
    pub accent: Rgb,
    pub cursor_inactive_bg: Rgb,
    pub border: Rgb,
    pub error: Rgb,
}

pub const TOKYO_NIGHT: Palette = Palette {
    window_bg: Rgb(0x16161e),
    panel_bg: Rgb(0x1a1b26),
    header_bg: Rgb(0x1f2335),
    header_active_bg: Rgb(0x2f3549),
    text: Rgb(0xc0caf5),
    text_dim: Rgb(0x565f89),
    text_on_accent: Rgb(0x1a1b26),
    dir: Rgb(0x7aa2f7),
    symlink: Rgb(0x7dcfff),
    accent: Rgb(0x7aa2f7),
    cursor_inactive_bg: Rgb(0x292e42),
    border: Rgb(0x292e42),
    error: Rgb(0xf7768e),
};
