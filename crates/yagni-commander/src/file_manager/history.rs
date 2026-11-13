//! Alt-Down: the active tab's folder history, newest first, in the mounts
//! popup's look, under the panel's path header. A pick goes there like
//! Alt-Left/Alt-Right (core `Command::HistoryTo`).

use gpui_kit::{AppContext, Context, Focusable, IntoElement, ParentElement, Styled, Window};
use gpui_kit::{anchored, deferred, div, px};
use yagni_commander_core::mounts::Place;
use yagni_commander_core::{Command, Side, label};

use super::{FileManager, OpenMounts, POPUP_INSET};
use crate::mounts_popup::{MountsEvent, MountsPopup};
use crate::zoom::rems_from_px;

impl FileManager {
    /// Opens the active tab's history, or closes it if it is open. Not
    /// while the panel loads.
    pub(super) fn toggle_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = self.history.is_some();
        self.close_history(window, cx);
        if was_open || self.active_loading(cx) {
            return;
        }
        let commander = self.commander.read(cx);
        let side = commander.active();
        let history = commander.panel(side).history();
        let count = history.places().len();
        // Newest first: row `r` is place `count - 1 - r`.
        let places: Vec<Place> = history
            .places()
            .iter()
            .rev()
            .map(|place| Place {
                label: label(&place.path),
                path: place.path.clone(),
            })
            .collect();
        if places.is_empty() {
            return;
        }
        let highlight = count - 1 - history.current();
        self.close_hotlist(window, cx);
        self.close_mounts(window, cx);
        self.end_search(cx);
        let popup = cx.new(|cx| {
            MountsPopup::titled(
                ("Folder history", "history", "history-title"),
                places,
                highlight,
                cx,
            )
        });
        let picked = cx.subscribe_in(
            &popup,
            window,
            move |this, _, event: &MountsEvent, window, cx| {
                this.close_history(window, cx);
                if let MountsEvent::Pick(row, _) = event {
                    this.execute(Command::HistoryTo(count - 1 - row), cx);
                }
            },
        );
        let focus = popup.focus_handle(cx);
        let blurred = cx.on_focus_out(&focus, window, |this, _, _, cx| {
            if this.history.take().is_some() {
                cx.notify();
            }
        });
        focus.focus(window, cx);
        self.history = Some(OpenMounts {
            side,
            popup,
            _subscriptions: vec![picked, blurred],
        });
        cx.notify();
    }

    /// Closes the history; focus goes back to the panels.
    pub(super) fn close_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// The history, where the hotlist popup opens.
    pub(super) fn history_overlay(&self, side: Side) -> Option<impl IntoElement + use<>> {
        let open = self.history.as_ref().filter(|open| open.side == side)?;
        Some(
            div()
                .absolute()
                .top(rems_from_px(crate::panel_view::BELOW_HEADER + POPUP_INSET))
                .left(rems_from_px(POPUP_INSET))
                .child(deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.0))
                        .child(open.popup.clone()),
                )),
        )
    }
}
