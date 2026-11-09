//! The update button in the status bar: a quiet check for a newer release
//! (at startup, and when the setting is turned on), and installing it.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};

use gpui_kit::component::WindowExt;
use gpui_kit::{
    AppContext, Context, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, relative,
};
use yagni_commander_core::{icons, storage, update};

use super::FileManager;
use super::commands::{OPENER_POLL, focus_when_open, show_error, show_message, themed_dialog};
use crate::button_row::{ButtonRow, OnPress};
use crate::config_state::CurrentConfig;
use crate::theme::Theme;
use crate::zoom::rems_from_px;

/// One step of the install box's bar.
const FRAME: std::time::Duration = std::time::Duration::from_millis(33);
/// Steps for the bar to cross its track.
const SWEEP_FRAMES: u32 = 45;
/// The moving part's share of the track.
const BAR: f32 = 0.3;

type Relaunch = Rc<dyn Fn(&Path) -> Result<(), String>>;

pub(crate) struct Updates {
    /// The newer release the quiet check found: the button shows.
    pub(crate) available: Option<update::Version>,
    checking: bool,
    /// What to start after an update; `None` if this copy isn't where the
    /// install script puts it, so the button only says how to install.
    pub(crate) app: Option<PathBuf>,
    /// Where the releases are. Tests point it at a local folder.
    pub(crate) releases: String,
    /// Starts the new copy. Tests replace it.
    pub(crate) relaunch: Relaunch,
}

/// The install box's text and a bar that shows activity, not an amount: the
/// download happens inside `install.sh`, out of our sight. Stepped by its
/// own loop, since `set_reduce_motion` freezes gpui's repeating animations.
struct InstallingView {
    version: update::Version,
    frame: u32,
}

impl Render for InstallingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let across = (self.frame % SWEEP_FRAMES) as f32 / SWEEP_FRAMES as f32;
        div()
            .debug_selector(|| "update-installing".into())
            .flex()
            .flex_col()
            .gap(rems_from_px(10.0))
            .pb(rems_from_px(4.0))
            .child(format!("Installing {}...", self.version))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(rems_from_px(4.0))
                    .rounded(rems_from_px(2.0))
                    .overflow_hidden()
                    .bg(colors.raised_border)
                    .child(
                        div()
                            .debug_selector(|| "update-progress-bar".into())
                            .absolute()
                            .top_0()
                            .h_full()
                            // Enters past the left edge, leaves past the right.
                            .left(relative(across * (1.0 + BAR) - BAR))
                            .w(relative(BAR))
                            .rounded(rems_from_px(2.0))
                            .bg(colors.accent),
                    ),
            )
    }
}

impl Updates {
    pub(super) fn new() -> Self {
        Self {
            available: None,
            checking: false,
            // Now: once the file is replaced, Linux reports it as deleted.
            app: std::env::current_exe()
                .ok()
                .and_then(|exe| update::installed_app(&exe, &storage::home_dir())),
            releases: update::RELEASES_URL.to_owned(),
            relaunch: Rc::new(update::relaunch),
        }
    }
}

#[cfg(test)]
impl Updates {
    pub(crate) fn is_checking(&self) -> bool {
        self.checking
    }
}

impl FileManager {
    /// At startup, if the setting is on. Called by `main`, so app tests,
    /// whose `update_url` is set after `new`, can never ask GitHub.
    pub(crate) fn check_at_start(&mut self, cx: &mut Context<Self>) {
        if cx.global::<CurrentConfig>().config.check_updates {
            self.check_quietly(cx);
        }
    }

    pub(super) fn check_updates_changed(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            self.check_quietly(cx);
        } else {
            self.updates.available = None;
        }
    }

    /// Asks GitHub on a thread of its own. A failure says nothing: nobody
    /// asked, and being offline is no news.
    fn check_quietly(&mut self, cx: &mut Context<Self>) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        let (tx, rx) = mpsc::channel();
        let url = self.update_url.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check(&url));
        });
        // Polled: a wake-up from another thread panics gpui's test scheduler.
        cx.spawn(async move |this, cx| {
            let result = loop {
                cx.background_executor().timer(OPENER_POLL).await;
                match rx.try_recv() {
                    Ok(result) => break result,
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => return,
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.updates.checking = false;
                // Turned off while the check ran.
                if !cx.global::<CurrentConfig>().config.check_updates {
                    return;
                }
                match result {
                    Ok(update::Check::Available { latest, .. }) => {
                        this.updates.available = Some(latest)
                    }
                    Ok(update::Check::UpToDate(_)) => this.updates.available = None,
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The button at the right of the status bar, while a release is waiting.
    pub(super) fn update_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.updates.available?;
        let colors = &Theme::get(cx).colors;
        Some(
            div()
                .id("update-button")
                .debug_selector(|| "update-button".into())
                .flex_none()
                .self_center()
                .ml(rems_from_px(8.0))
                .px(rems_from_px(6.0))
                .h(rems_from_px(16.0))
                .flex()
                .items_center()
                .rounded(rems_from_px(3.0))
                .border_1()
                .border_color(colors.border)
                .bg(colors.accent)
                .text_color(colors.text_on_accent)
                .cursor_pointer()
                .font_family(icons::FONT_FAMILY)
                .text_size(rems_from_px(10.0))
                .child(icons::UPDATE.to_string())
                .on_click(cx.listener(|this, _, window, cx| this.offer_update(window, cx))),
        )
    }

    fn offer_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let Some(latest) = self.updates.available else {
            return;
        };
        let current = update::Version::current();
        let refocus = Some(self.focus.clone());
        if self.updates.app.is_none() {
            // Not the copy the installer replaces: how to install it.
            let result = Ok(update::Check::Available { latest, current });
            self.show_update_check(result, window, cx);
            return;
        }
        if self.job.is_some() {
            let text = "A file operation is running. Update when it has finished.";
            show_message("Update", text, "OK", refocus, window, cx);
            return;
        }
        let text = update_question(latest, current);
        let this = cx.entity().downgrade();
        let yes: OnPress = Rc::new(move |window, cx| {
            window.close_dialog(cx);
            let _ = this.update(cx, |this, cx| this.install_update(latest, window, cx));
        });
        let no: OnPress = Rc::new({
            let focus = self.focus.clone();
            move |window, cx| {
                window.close_dialog(cx);
                focus.focus(window, cx);
            }
        });
        let buttons = ButtonRow::build([("Update and restart", yes), ("Not now", no)], 0, cx);
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            themed_dialog(dialog, cx)
                .title("Update yagni-commander")
                .w(crate::zoom::dialog_width(520.0, cx))
                .close_button(false)
                .child(
                    div()
                        .debug_selector(|| "update-question".into())
                        .child(text.clone()),
                )
                .footer(buttons.clone())
        });
    }

    /// Installs on a thread behind a box that can't be closed: the restart
    /// must not catch a file operation started meanwhile.
    fn install_update(
        &mut self,
        version: update::Version,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (tx, rx) = mpsc::channel();
        let releases = self.updates.releases.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::install(&releases, version));
        });
        let view = cx.new(|_| InstallingView { version, frame: 0 });
        let progress = view.clone();
        // After the question's close has run.
        window.defer(cx, move |window, cx| {
            window.open_dialog(cx, move |dialog, _, cx| {
                themed_dialog(dialog, cx)
                    .title("Update yagni-commander")
                    .w(crate::zoom::dialog_width(420.0, cx))
                    .close_button(false)
                    .keyboard(false)
                    .overlay_closable(false)
                    .child(view.clone())
            });
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = loop {
                cx.background_executor().timer(FRAME).await;
                progress.update(cx, |view, cx| {
                    view.frame += 1;
                    cx.notify();
                });
                match rx.try_recv() {
                    Ok(result) => break result,
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        break Err("The update stopped unexpectedly.".to_owned());
                    }
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_installed(result, window, cx)
            });
        })
        .detach();
    }

    fn update_installed(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let refocus = Some(self.focus.clone());
        let failure = match result {
            Err(e) => ("Update failed", e),
            Ok(()) => {
                let app = self.updates.app.clone().unwrap_or_default();
                match (self.updates.relaunch)(&app) {
                    Ok(()) => return cx.quit(),
                    Err(e) => ("Updated, but not restarted", e),
                }
            }
        };
        // After the close above, which would close this box instead.
        window.defer(cx, move |window, cx| {
            show_error(failure.0, failure.1, refocus, window, cx)
        });
    }
}

pub(super) fn update_question(latest: update::Version, current: update::Version) -> String {
    format!("yagni-commander {latest} is available (you have {current}). Install it and restart?")
}
