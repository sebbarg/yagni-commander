//! `L` (or a click on the language in the status line), in source mode:
//! the grammar picked by hand. A popup lists Plain text and every grammar, searched by typing;
//! the pick shows at once and is remembered for the session by the file's extension, else its
//! whole name (core `viewer::pick_key`), so the next viewer of such a file
//! opens with it (owner, 2026-11-17: never saved).

use std::collections::HashMap;

use gpui_kit::{
    AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, ScrollHandle,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::viewer::{LANGUAGES, pick_key};

use super::{ViewerView, padding, status_height};
use crate::actions::{LANGUAGE_CONTEXT, language as act};
use crate::file_manager::commands::show_message;
use crate::theme::{Theme, popup_frame, popup_title};
use crate::zoom::rems_from_px;

/// Rows shown before the list scrolls.
const MAX_ROWS: f32 = 12.0;
const ROW_HEIGHT: f32 = 26.0;
/// The first row: no grammar.
pub(super) const PLAIN: &str = "Plain text";

/// The grammars picked by hand this session, by `pick_key`; `None` is
/// Plain text.
#[derive(Default)]
pub struct Picks(HashMap<String, Option<&'static str>>);

impl Global for Picks {}

impl Picks {
    /// The pick for a file named `name`, if one was made: `Some(None)` is
    /// Plain text.
    pub(super) fn of(name: &str, cx: &gpui_kit::App) -> Option<Option<&'static str>> {
        cx.try_global::<Self>()
            .and_then(|picks| picks.0.get(&pick_key(name)).copied())
    }

    fn remember(name: &str, language: Option<&'static str>, cx: &mut gpui_kit::App) {
        cx.default_global::<Self>()
            .0
            .insert(pick_key(name), language);
    }
}

pub(super) enum LanguageEvent {
    /// `None`: Plain text.
    Pick(Option<&'static str>),
    Dismiss,
}

/// The first row whose shown name starts with `typed`, ignoring case
/// (empty: none).
fn first_match(typed: &str) -> Option<usize> {
    if typed.is_empty() {
        return None;
    }
    let typed = typed.to_lowercase();
    rows().position(|(_, shown)| shown.to_lowercase().starts_with(&typed))
}

/// The rows: Plain text, then every grammar by its shown name.
fn rows() -> impl Iterator<Item = (Option<&'static str>, &'static str)> {
    std::iter::once((None, PLAIN)).chain(LANGUAGES.iter().map(|&(name, shown)| (Some(name), shown)))
}

pub(super) struct LanguagePopup {
    highlight: usize,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The search typed so far.
    pub(super) typed: String,
    /// Drawn once. gpui's first frame of the list drops a scroll asked for
    /// (the handle doesn't know its bounds yet), so the scroll to the
    /// highlight is asked for on the next one.
    drawn: bool,
}

impl EventEmitter<LanguageEvent> for LanguagePopup {}

impl Focusable for LanguagePopup {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LanguagePopup {
    /// The highlight starts on `current` (`None`: Plain text).
    fn new(current: Option<&'static str>, cx: &mut Context<Self>) -> Self {
        let highlight = rows().position(|(name, _)| name == current).unwrap_or(0);
        Self {
            highlight,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            typed: String::new(),
            drawn: false,
        }
    }

    #[cfg(test)]
    pub(super) fn highlight(&self) -> usize {
        self.highlight
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        self.typed.clear();
        let count = rows().count() as isize;
        self.highlight = (self.highlight as isize + by).rem_euclid(count) as usize;
        self.scroll.scroll_to_item(self.highlight);
        cx.notify();
    }

    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some((language, _)) = rows().nth(row) {
            cx.emit(LanguageEvent::Pick(language));
        }
    }

    /// Typing searches (owner, 2026-11-17): the highlight goes to the first
    /// row starting with the typed text, which never picks. A character that
    /// would match nothing is ignored; Backspace takes the last one back.
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        if keystroke.key == "backspace" {
            if self.typed.pop().is_some() {
                if let Some(ix) = first_match(&self.typed) {
                    self.go_to(ix);
                }
                cx.notify();
            }
            return;
        }
        let Some(text) = keystroke.key_char.as_deref() else {
            return;
        };
        if text.chars().count() != 1 {
            return;
        }
        let typed = format!("{}{text}", self.typed);
        if let Some(ix) = first_match(&typed) {
            self.typed = typed;
            self.go_to(ix);
            cx.notify();
        }
    }

    fn go_to(&mut self, row: usize) {
        self.highlight = row;
        self.scroll.scroll_to_item(row);
    }
}

impl Render for LanguagePopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.drawn {
            self.drawn = true;
            cx.on_next_frame(window, |this, _, cx| {
                this.scroll.scroll_to_item(this.highlight);
                cx.notify();
            });
        }
        let theme = Theme::get(cx).clone();
        let colors = theme.colors.clone();
        let items = rows().enumerate().map(|(ix, (_, shown))| {
            let highlighted = ix == self.highlight;
            div()
                .id(("language-row", ix))
                .debug_selector(move || format!("language-row-{ix}"))
                .h(rems_from_px(ROW_HEIGHT))
                .flex()
                .items_center()
                .px(rems_from_px(10.0))
                .whitespace_nowrap()
                .when(highlighted, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.pick(ix, cx);
                    }),
                )
                .child(shown)
        });
        div()
            .key_context(LANGUAGE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Up, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Down, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::Pick, _, cx| this.pick(this.highlight, cx)))
            .on_action(cx.listener(|_, _: &act::Close, _, cx| cx.emit(LanguageEvent::Dismiss)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(LanguageEvent::Dismiss)))
            .debug_selector(|| "language-popup".into())
            .occlude()
            .min_w(rems_from_px(200.0))
            .pb(rems_from_px(4.0))
            .map(|d| popup_frame(d, &theme))
            .text_sm()
            .text_color(colors.text)
            .child(popup_title(
                if self.typed.is_empty() {
                    "Language".to_owned()
                } else {
                    format!("Language: {}", self.typed)
                },
                "language-title",
                &theme,
            ))
            .child(
                div()
                    .id("language-list")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .max_h(rems_from_px(ROW_HEIGHT * MAX_ROWS))
                    .flex()
                    .flex_col()
                    .children(items),
            )
    }
}

/// The open popup and the subscription to its events.
pub(super) type Open = (Entity<LanguagePopup>, Subscription);

impl ViewerView {
    /// The file's name, as the grammar and the pick go by it.
    pub(super) fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// `L` or a click on the language: opens the popup, or closes it.
    /// Only in source mode (owner, 2026-11-17; `S` first, also for a file
    /// without a grammar), not over hex, Markdown or image mode.
    pub(super) fn toggle_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.language.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
            return;
        }
        let Some(current) = self.source_shown().map(|mode| mode.language) else {
            return;
        };
        let popup = cx.new(|cx| LanguagePopup::new(current, cx));
        let subscription = cx.subscribe_in(&popup, window, |this, _, event, window, cx| {
            this.language = None;
            this.focus.focus(window, cx);
            if let LanguageEvent::Pick(language) = event {
                this.pick_language(*language, window, cx);
            }
            cx.notify();
        });
        popup.focus_handle(cx).focus(window, cx);
        self.language = Some((popup, subscription));
        cx.notify();
    }

    /// Source mode, when it is what shows.
    fn source_shown(&self) -> Option<&super::source::SourceMode> {
        self.source
            .as_ref()
            .filter(|_| !self.hex && self.markdown.is_none() && self.picture.is_none())
    }

    /// Source mode in `language` (`None`: Plain text), remembered for such
    /// files. If the file can't be shown (it changed past the limits), a box
    /// says why, the mode stays and nothing is remembered.
    pub(super) fn pick_language(
        &mut self,
        language: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.start_source(language, window, cx) {
            Ok(()) => Picks::remember(&self.file_name(), language, cx),
            Err(message) => {
                let back = Some(self.focus.clone());
                show_message("Source", message, "OK", back, window, cx);
            }
        }
        cx.notify();
    }

    /// Source mode's language name at the right of the status line, left of
    /// the reload button: a click opens the popup.
    pub(super) fn language_button(
        &self,
        language: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        div()
            .id("viewer-language")
            .debug_selector(|| "viewer-language".into())
            .flex_none()
            .ml(rems_from_px(8.0))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|d| d.text_color(colors.text))
            .child(language)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Stopped here, the click can't reach the view's own
                    // focus handling.
                    cx.stop_propagation();
                    this.toggle_language(window, cx);
                }),
            )
    }

    /// The popup, over the text above the status line's right end.
    pub(super) fn render_language(&self, cx: &gpui_kit::App) -> Option<impl IntoElement> {
        let (popup, _) = self.language.as_ref()?;
        Some(
            div()
                .absolute()
                .right(px(padding(cx)))
                .bottom(px(status_height(cx) + padding(cx) / 2.0))
                .child(popup.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(row: Option<usize>) -> Option<&'static str> {
        row.and_then(|ix| rows().nth(ix)).map(|(_, shown)| shown)
    }

    #[test]
    fn typing_finds_the_first_name_starting_with_it() {
        assert_eq!(shown(first_match("m")), Some("Make"));
        assert_eq!(shown(first_match("mar")), Some("Markdown"));
        assert_eq!(shown(first_match("MAR")), Some("Markdown"));
        assert_eq!(shown(first_match("c")), Some("C"));
        assert_eq!(shown(first_match("c+")), Some("C++"));
        assert_eq!(shown(first_match("c#")), Some("C#"));
        assert_eq!(shown(first_match("plain t")), Some("Plain text"));
        assert_eq!(shown(first_match("jso")), Some("JSON"));
        assert_eq!(first_match("x"), None);
        assert_eq!(first_match(""), None);
    }
}
