//! The Connect dialog (Ctrl-F, Commands > Connect...): the hosts of
//! `~/.ssh/config`, by name, under a field that filters them. Enter opens the
//! highlighted host's login folder (`sftp://host`) in the active panel; when
//! the typed text is not exactly an alias it is offered last as a host of
//! its own (`user@host`). Up/Down and Page Up/Page Down move in the list
//! while the field keeps the focus; Home/End stay the field's.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder,
};
use yagni_commander_core::ssh_config::SshHost;

use crate::file_manager::commands::dialog_field;
use crate::theme::Theme;
use crate::zoom::rems_from_px;

/// Rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 10.0;
/// Rows Page Up/Page Down move: a screenful less one, so a row stays.
const PAGE: isize = VISIBLE_ROWS as isize - 1;
const ROW_HEIGHT: f32 = 24.0;

/// A row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A host from the config, by index.
    Host(usize),
    /// The typed text, offered as a host of its own.
    Typed(String),
}

pub struct ConnectDialog {
    hosts: Vec<SshHost>,
    rows: Vec<Row>,
    cursor: usize,
    pub(crate) field: Entity<InputState>,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl ConnectDialog {
    /// `hosts` are listed by alias, ignoring case.
    pub(crate) fn new(
        mut hosts: Vec<SshHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        hosts.sort_by_cached_key(|h| (h.alias.to_lowercase(), h.alias.clone()));
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Filter, or user@host"));
        let subscription = cx.subscribe_in(&field, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                this.refilter(cx);
            }
        });
        let mut this = Self {
            hosts,
            rows: Vec::new(),
            cursor: 0,
            field,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        };
        this.refilter(cx);
        this
    }

    /// The rows for the field's text: matching hosts, then the text itself
    /// unless it is an alias. The cursor goes back to the first row.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let typed = self.field.read(cx).value().trim().to_owned();
        self.rows = rows_for(&self.hosts, &typed);
        self.cursor = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        self.cursor = (self.cursor as isize + by).clamp(0, last) as usize;
        self.scroll.scroll_to_item(self.cursor);
        cx.notify();
    }

    /// The host the highlighted row names.
    pub fn target(&self) -> Option<String> {
        match self.rows.get(self.cursor)? {
            Row::Host(ix) => Some(self.hosts[*ix].alias.clone()),
            Row::Typed(text) => Some(text.clone()),
        }
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
}

/// The list for `typed`: the hosts it matches (alias or detail, ignoring
/// case), then `typed` itself if it is not exactly one of the aliases.
fn rows_for(hosts: &[SshHost], typed: &str) -> Vec<Row> {
    let filter = typed.to_lowercase();
    let mut rows: Vec<Row> = hosts
        .iter()
        .enumerate()
        .filter(|(_, h)| h.matches(&filter))
        .map(|(ix, _)| Row::Host(ix))
        .collect();
    if !typed.is_empty() && !hosts.iter().any(|h| h.alias == typed) {
        rows.push(Row::Typed(typed.to_owned()));
    }
    rows
}

impl Render for ConnectDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let at_cursor = ix == self.cursor;
            let (name, detail) = match row {
                Row::Host(h) => (self.hosts[*h].alias.clone(), self.hosts[*h].detail()),
                Row::Typed(text) => (text.clone(), "not in ~/.ssh/config".to_owned()),
            };
            div()
                .id(("connect-row", ix))
                .debug_selector(move || format!("connect-row-{ix}"))
                .flex()
                .items_center()
                .h(rems_from_px(ROW_HEIGHT))
                .gap(rems_from_px(16.0))
                .px(rems_from_px(8.0))
                .when(at_cursor, |d| {
                    d.bg(colors.accent).text_color(colors.text_on_accent)
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.cursor = ix;
                    cx.notify();
                    if event.click_count() >= 2 {
                        window.dispatch_action(
                            Box::new(gpui_kit::base::actions::Confirm { secondary: false }),
                            cx,
                        );
                    }
                }))
                .child(div().w(rems_from_px(160.0)).truncate().child(name))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .when(!at_cursor, |d| d.text_color(colors.text_secondary))
                        .child(detail),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(rems_from_px(8.0))
            .capture_action(
                cx.listener(|this, _: &gpui_kit::base::input::MoveUp, _, cx| {
                    this.step(-1, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_kit::base::input::MoveDown, _, cx| {
                    this.step(1, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_kit::base::input::MovePageUp, _, cx| {
                    this.step(-PAGE, cx);
                    cx.stop_propagation();
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui_kit::base::input::MovePageDown, _, cx| {
                    this.step(PAGE, cx);
                    cx.stop_propagation();
                }),
            )
            .child(dialog_field(&self.field))
            .child(
                div()
                    .id("connect-list")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .h(rems_from_px(ROW_HEIGHT * VISIBLE_ROWS))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.panel_bg)
                    .when(self.rows.is_empty(), |d| {
                        d.child(
                            div()
                                .debug_selector(|| "connect-empty".into())
                                .p(rems_from_px(8.0))
                                .text_color(colors.text_dim)
                                .child("No hosts in ~/.ssh/config. Type user@host."),
                        )
                    })
                    .children(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str, host_name: Option<&str>) -> SshHost {
        SshHost {
            alias: alias.into(),
            host_name: host_name.map(Into::into),
            ..SshHost::default()
        }
    }

    #[test]
    fn rows_filter_hosts_and_offer_the_typed_text_last() {
        let hosts = [host("web", Some("10.0.0.5")), host("db", Some("db.lan"))];
        assert_eq!(rows_for(&hosts, ""), [Row::Host(0), Row::Host(1)]);
        assert_eq!(
            rows_for(&hosts, "LAN"),
            [Row::Host(1), Row::Typed("LAN".into())]
        );
        assert_eq!(rows_for(&hosts, "web"), [Row::Host(0)], "an alias exactly");
        assert_eq!(
            rows_for(&hosts, "me@other"),
            [Row::Typed("me@other".into())]
        );
    }
}
