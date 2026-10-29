//! Markdown mode (`M`): the whole file (at most `CAP`) rendered by
//! gpui-base's `TextView` (gpui-component's wrapper hides `image_source`),
//! which parses on a background task, lays out the blocks in a `gpui::list`
//! and scrolls, selects and draws highlights itself. This keeps its state and
//! maps the viewer's keys, search and copy onto it. Positions here are byte
//! offsets in the rendered text (what the view shows and copies), never in
//! the file. Images: core `Images` decides which are shown (local files) and
//! turns the rest into alt text before the view sees the source.

use std::collections::HashMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use gpui_kit::base::TextViewStyle;
use gpui_kit::base::text::TextView;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::text::{RangeHighlight, TextViewState};
use gpui_kit::{
    App, AppContext, Entity, HighlightStyle, ImageCacheError, ImageSource, ListOffset, Resource,
    SharedString, SharedUri, StyleRefinement, Styled, px, rems,
};
use yagni_commander_core::find::text::TextQuery;
use yagni_commander_core::format_size;
use yagni_commander_core::viewer::{Direction, Images, find, has_scheme};

use super::search::PREFILL_MAX;
use crate::zoom::{Zoom, scaled};

/// Larger files are refused: the whole file is read and laid out on the
/// UI thread.
pub(super) const CAP: u64 = 1 << 20;
/// Longer paragraphs are refused: laying one out costs more than linear in
/// its length (release build, CPU rendering: 64 KiB at once, 1 MiB ~3 s,
/// 4 MiB ~65 s).
pub(super) const PARAGRAPH_CAP: u64 = 64 << 10;

/// Why a file of `len` bytes can't be shown, before it is read.
pub(super) fn too_large(len: u64) -> Option<String> {
    (len > CAP).then(|| {
        format!(
            "The file is too large to show as Markdown ({}, at most {}).",
            format_size(len),
            format_size(CAP)
        )
    })
}

/// Why `bytes` can't be shown: a paragraph (lines without a blank line
/// between them, so also a table or a code block) over `PARAGRAPH_CAP`.
pub(super) fn too_long(bytes: &[u8]) -> Option<String> {
    let longest = longest_paragraph(bytes);
    (longest > PARAGRAPH_CAP).then(|| {
        format!(
            "A paragraph is too long to show as Markdown ({}, at most {}).",
            format_size(longest),
            format_size(PARAGRAPH_CAP)
        )
    })
}

/// The longest run of non-blank lines, in bytes with their line ends.
fn longest_paragraph(bytes: &[u8]) -> u64 {
    let (mut longest, mut run) = (0, 0);
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            run = 0;
        } else {
            run += line.len() as u64;
            longest = longest.max(run);
        }
    }
    longest
}

/// A line for Up/Down, in px at the 16 px base.
const LINE: f32 = 24.0;

pub(super) enum Scroll {
    /// Lines down (positive) or up.
    Lines(f32),
    /// Screens down (positive) or up, less a line each.
    Pages(f32),
    Start,
    End,
}

pub(super) struct Markdown {
    state: Entity<TextViewState>,
    /// The match last found.
    pub(super) current: Option<Range<usize>>,
    /// The file of each image shown, by its URL.
    images: Arc<HashMap<String, PathBuf>>,
}

impl Markdown {
    /// Starts parsing `bytes` (invalid UTF-8 becomes U+FFFD), a file in
    /// `dir`; checks its images' files first.
    pub(super) fn new(bytes: &[u8], dir: &Path, cx: &mut App) -> Self {
        let images = Images::prepare(&String::from_utf8_lossy(bytes), dir);
        let state = cx.new(|cx| TextViewState::markdown(&images.text, cx));
        Self {
            state,
            current: None,
            images: Arc::new(images.paths),
        }
    }

    pub(super) fn element(&self, cx: &App) -> TextView {
        let images = self.images.clone();
        TextView::new(&self.state)
            .style(text_style(cx))
            .image_source(move |url| image_source(&images, url))
            .selectable(true)
            .scrollable(true)
            .on_link_click(|url, _, _, cx| {
                if let Some(url) = followed(url) {
                    cx.open_url(url);
                }
            })
            .size_full()
    }

    pub(super) fn scroll(&self, scroll: Scroll, cx: &mut App) {
        let line = scaled(LINE, Zoom::get(cx).viewer);
        let list = self.state.read(cx).list_state().clone();
        match scroll {
            Scroll::Lines(n) => list.scroll_by(px(line * n)),
            Scroll::Pages(n) => {
                let screen = f32::from(list.viewport_bounds().size.height);
                list.scroll_by(px((screen - line).max(line) * n));
            }
            Scroll::Start => list.scroll_to(ListOffset::default()),
            Scroll::End => list.scroll_to_end(),
        }
        self.state.update(cx, |_, cx| cx.notify());
    }

    /// How far down the view is scrolled, 0 to 100 (100 when it all fits).
    pub(super) fn percent(&self, cx: &App) -> u64 {
        let list = self.state.read(cx).list_state();
        let max = f32::from(list.max_offset_for_scrollbar().y);
        let offset = -f32::from(list.scroll_px_offset_for_scrollbar().y);
        if max <= 0.0 {
            return 100;
        }
        (offset / max * 100.0).round().clamp(0.0, 100.0) as u64
    }

    /// The text as shown.
    pub(super) fn rendered(&self, cx: &App) -> String {
        self.state.read(cx).rendered_text().as_str().to_owned()
    }

    pub(super) fn select_all(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| state.select_all(cx));
    }

    /// The selection as shown (no Markdown marks), trimmed as the view's
    /// own copy does; empty without one.
    pub(super) fn selected_text(&self, cx: &App) -> String {
        self.state.read(cx).selected_text().trim().to_owned()
    }

    /// Tests: the images shown and their files.
    #[cfg(test)]
    pub(super) fn image_files(&self) -> Vec<(String, PathBuf)> {
        let mut files: Vec<_> = self
            .images
            .iter()
            .map(|(url, path)| (url.clone(), path.clone()))
            .collect();
        files.sort();
        files
    }

    /// What Ctrl-F starts with: the selection, under text mode's rules
    /// (one line, at most `PREFILL_MAX` bytes, not just the match last
    /// found).
    pub(super) fn prefill(&self, cx: &App) -> Option<String> {
        let selected = self.selected_text(cx);
        let rendered = self.rendered(cx);
        let found = self.current.as_ref().and_then(|m| rendered.get(m.clone()));
        let fits = !selected.is_empty() && selected.len() as u64 <= PREFILL_MAX;
        (fits && !selected.contains(['\n', '\r']) && found != Some(selected.as_str()))
            .then_some(selected)
    }

    /// The next match after the current one (or from the top), or the one
    /// before it (or from the end). A found match is highlighted and
    /// scrolled to; `None` leaves everything as it was.
    pub(super) fn find(
        &mut self,
        query: &TextQuery,
        direction: Direction,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        let text = self.rendered(cx).into_bytes();
        let from = match (direction, &self.current) {
            (Direction::Forward, Some(m)) => m.start + 1,
            (Direction::Backward, Some(m)) => m.start,
            (Direction::Forward, None) => 0,
            (Direction::Backward, None) => text.len(),
        };
        let found = find_in(text, query, from, direction)?;
        let color = cx.theme().selection;
        let shown = self.state.update(cx, |state, cx| {
            state
                .set_range_highlights([RangeHighlight::new(found.clone(), color)], cx)
                .and_then(|()| state.reveal_range(found.clone(), cx))
        });
        // A range the view can't take (not on character boundaries) counts
        // as no match rather than a match nobody sees.
        shown.ok()?;
        self.current = Some(found.clone());
        Some(found)
    }

    /// A new search: the next F3 starts at the top again.
    pub(super) fn restart(&mut self, cx: &mut App) {
        self.current = None;
        self.state
            .update(cx, |state, cx| state.clear_range_highlights(cx));
    }
}

/// Headings as multiples of the body text, which follows the viewer's zoom
/// (GitHub's sizes). gpui-base's own are fixed: the same multiples of 14 px.
const HEADING_SCALE: [f32; 6] = [2.0, 1.5, 1.25, 1.0, 0.875, 0.85];

fn heading_scale(level: u8) -> f32 {
    HEADING_SCALE[usize::from(level.clamp(1, 6)) - 1]
}

/// The view's style: gpui-component's (`base_text_view_style`, private
/// there; a style given to the view replaces it), plus heading sizes.
fn text_style(cx: &App) -> TextViewStyle {
    let theme = cx.theme();
    let radius = theme.semantic_tokens().radius.md;
    let mut table = StyleRefinement::default();
    table.corner_radii.top_left = Some(radius.into());
    table.corner_radii.top_right = Some(radius.into());
    table.corner_radii.bottom_left = Some(radius.into());
    table.corner_radii.bottom_right = Some(radius.into());
    let code_block = StyleRefinement {
        corner_radii: table.corner_radii.clone(),
        ..Default::default()
    };
    let table_head = StyleRefinement::default()
        .bg(theme.table_head)
        .text_color(theme.table_head_foreground);
    TextViewStyle::default()
        .with_foreground(theme.foreground)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(theme.link)
        .with_selection(theme.selection)
        .with_code_background(theme.muted)
        .with_border(theme.border)
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(table_head)
        .with_inline_code(HighlightStyle {
            background_color: Some(theme.accent),
            ..Default::default()
        })
        .with_dark(theme.is_dark())
        .with_heading(|level| StyleRefinement::default().text_size(rems(heading_scale(level))))
}

/// An image's file, checked by `Images`. Anything else (only an image the
/// view finds that we didn't) fails to load: never a network fetch, which
/// the view's default source would try for a URL.
fn image_source(images: &HashMap<String, PathBuf>, url: &SharedUri) -> ImageSource {
    match images.get(url.as_ref()) {
        Some(path) => ImageSource::Resource(Resource::Path(path.as_path().into())),
        None => ImageSource::Custom(Arc::new(|_, _| {
            let unsupported = io::Error::from(io::ErrorKind::Unsupported);
            Some(Err(ImageCacheError::Io(Arc::new(unsupported))))
        })),
    }
}

/// The viewer's matching (`find`, one line at most), on text in memory.
fn find_in(
    text: Vec<u8>,
    query: &TextQuery,
    from: usize,
    direction: Direction,
) -> Option<Range<usize>> {
    let (cancel, progress) = (AtomicBool::new(false), AtomicU64::new(0));
    let found = find(&text, query, from as u64, direction, &cancel, &progress).ok()??;
    Some(found.start as usize..found.end as usize)
}

/// The URL a click opens: only one with a scheme. Relative links would
/// resolve against the app's working folder, not the file's.
fn followed(url: &SharedString) -> Option<&str> {
    has_scheme(url).then_some(url.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yagni_commander_core::find::text::Text;

    fn query(pattern: &str) -> TextQuery {
        Text {
            pattern: pattern.into(),
            ..Default::default()
        }
        .compile()
        .unwrap()
    }

    #[test]
    fn files_up_to_1_mib_are_shown() {
        assert_eq!(too_large(0), None);
        assert_eq!(too_large(1024 * 1024), None);
        assert_eq!(
            too_large(5 * 1024 * 1024).as_deref(),
            Some("The file is too large to show as Markdown (5.0 MiB, at most 1.0 MiB).")
        );
    }

    #[test]
    fn a_paragraph_runs_until_a_blank_line() {
        assert_eq!(longest_paragraph(b""), 0);
        assert_eq!(longest_paragraph(b"abc"), 3);
        assert_eq!(longest_paragraph(b"ab\ncd\n\nefg\n"), 6);
        assert_eq!(longest_paragraph(b"a\n \t\r\nbcd\r\n"), 5);
        assert_eq!(longest_paragraph(b"\n\n\n"), 0);
    }

    #[test]
    fn paragraphs_up_to_64_kib_are_shown() {
        let text = |n: usize| [vec![b'a'; n], b"\n\nshort\n".to_vec()].concat();
        assert_eq!(too_long(&text(64 * 1024 - 1)), None, "with its line end");
        assert_eq!(
            too_long(&text(300 * 1024)).as_deref(),
            Some("A paragraph is too long to show as Markdown (300.0 KiB, at most 64.0 KiB).")
        );
        // Many paragraphs under the limit are fine.
        let many = [text(60 * 1024), text(60 * 1024)].concat();
        assert_eq!(too_long(&many), None);
    }

    #[test]
    fn only_checked_images_load_and_never_over_the_network() {
        let images = HashMap::from([("a.png".to_owned(), PathBuf::from("/d/a.png"))]);
        let source = |url: &str| image_source(&images, &SharedUri::from(url.to_owned()));
        assert!(matches!(
            source("a.png"),
            ImageSource::Resource(Resource::Path(p)) if p.as_ref() == Path::new("/d/a.png")
        ));
        for url in ["https://x/a.png", "b.png"] {
            let ImageSource::Custom(load) = source(url) else {
                panic!("{url}: not a failing source");
            };
            let cx = gpui_kit::TestAppContext::single();
            let failed = cx.update(|cx| {
                let window = cx
                    .open_window(Default::default(), |_, cx| cx.new(|_| gpui_kit::Empty))
                    .unwrap();
                window
                    .update(cx, |_, window, cx| matches!(load(window, cx), Some(Err(_))))
                    .unwrap()
            });
            assert!(failed, "{url}");
        }
    }

    #[test]
    fn headings_follow_the_body_text() {
        assert_eq!(heading_scale(1), 2.0);
        assert_eq!(heading_scale(2), 1.5);
        assert_eq!(heading_scale(4), 1.0);
        assert_eq!(heading_scale(6), 0.85);
        assert_eq!(heading_scale(0), 2.0, "clamped");
        assert_eq!(heading_scale(9), 0.85, "clamped");
    }

    #[test]
    fn only_links_with_a_scheme_are_followed() {
        let url = |s: &str| SharedString::from(s.to_owned());
        assert_eq!(followed(&url("https://x.org/a")), Some("https://x.org/a"));
        assert_eq!(followed(&url("mailto:a@b.c")), Some("mailto:a@b.c"));
        assert_eq!(followed(&url("git+ssh://h/r")), Some("git+ssh://h/r"));
        assert_eq!(followed(&url("docs/x.md")), None);
        assert_eq!(followed(&url("#features")), None);
        assert_eq!(followed(&url("../a:b.md")), None);
        assert_eq!(followed(&url("C:/x.md")), None);
        assert_eq!(followed(&url("1x:y")), None);
        assert_eq!(followed(&url("")), None);
    }

    #[test]
    fn finds_forward_and_backward_in_memory() {
        let text = || b"one two\none two".to_vec();
        let q = query("two");
        assert_eq!(find_in(text(), &q, 0, Direction::Forward), Some(4..7));
        assert_eq!(find_in(text(), &q, 5, Direction::Forward), Some(12..15));
        assert_eq!(find_in(text(), &q, 13, Direction::Forward), None);
        assert_eq!(find_in(text(), &q, 15, Direction::Backward), Some(12..15));
        assert_eq!(find_in(text(), &q, 12, Direction::Backward), Some(4..7));
        assert_eq!(find_in(text(), &q, 4, Direction::Backward), None);
    }
}
