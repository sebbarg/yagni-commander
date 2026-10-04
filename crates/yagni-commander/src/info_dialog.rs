//! The Properties box's body (Alt-Enter): label and value rows. The box
//! and its updates live in `file_manager/properties.rs`.

use gpui_kit::{Context, IntoElement, ParentElement, Render, Styled, Window, div, px};

use crate::theme::Theme;

/// Width of the label column.
const LABEL_WIDTH: f32 = 120.0;

pub(crate) struct InfoView {
    lines: Vec<(&'static str, String)>,
}

impl InfoView {
    pub(crate) fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Shows `lines`; returns whether anything changed (so a box waiting
    /// on a hung `stat` doesn't redraw every tick).
    pub(crate) fn set_lines(&mut self, lines: Vec<(&'static str, String)>) -> bool {
        if lines == self.lines {
            return false;
        }
        self.lines = lines;
        true
    }
}

impl Render for InfoView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(self.lines.iter().map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(LABEL_WIDTH))
                            .flex_none()
                            .text_color(colors.text_secondary)
                            .child(*label),
                    )
                    .child(div().flex_1().min_w_0().child(value.clone()))
            }))
    }
}

/// The box as text for Ctrl-C: the title, then `Label: value` lines.
pub(crate) fn copy_text(lines: &[(&'static str, String)]) -> String {
    let mut text = String::from("Properties");
    for (label, value) in lines {
        text.push_str(&format!("\n{label}: {value}"));
    }
    text
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_lines_reports_changes_only() {
        let mut view = super::InfoView::new();
        let lines = vec![("Name", "f".to_owned())];
        assert!(view.set_lines(lines.clone()));
        assert!(!view.set_lines(lines));
        assert!(view.set_lines(vec![("Name", "g".to_owned())]));
    }

    #[test]
    fn copy_text_is_the_title_then_one_line_each() {
        let lines = vec![("Name", "f".to_owned()), ("Type", "File".to_owned())];
        assert_eq!(super::copy_text(&lines), "Properties\nName: f\nType: File");
    }
}
