//! The update button in the status bar: a quiet check for a newer release
//! (at startup, every hour, and when the setting is turned on), and
//! installing it.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};

use gpui_kit::component::WindowExt;
use gpui_kit::{
    AppContext, Context, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Task, Window, div, relative,
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
/// Between quiet checks after the one at startup.
pub(crate) const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

type Relaunch = Rc<dyn Fn(&Path) -> Result<(), String>>;

pub(crate) struct Updates {
    /// The newer release the quiet check found: the button shows.
    pub(crate) available: Option<update::Version>,
    checking: bool,
    /// The hourly checks, started with the one at startup.
    hourly: Option<Task<()>>,
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
    /// The script's last step; `None` before the first.
    step: Option<update::Step>,
    cancelling: bool,
}

impl InstallingView {
    fn text(&self) -> String {
        let version = self.version;
        match (&self.step, self.cancelling) {
            (Some(update::Step::Copying(_)), _) => format!("Installing {version}..."),
            (_, true) => "Cancelling...".to_owned(),
            (Some(update::Step::Downloading(_)), _) | (None, _) => {
                format!("Downloading {version}...")
            }
        }
    }
}

/// What the install thread sends.
enum Installing {
    Step(update::Step),
    Done(Result<(), update::InstallError>),
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
            .child(
                div()
                    .debug_selector(|| "update-installing-text".into())
                    .child(self.text()),
            )
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
            hourly: None,
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
    /// At startup and then every `CHECK_EVERY`, each time if the setting is
    /// on then. Called by `main`, so app tests, whose `update_url` is set
    /// after `new`, can never ask GitHub.
    pub(crate) fn check_at_start(&mut self, cx: &mut Context<Self>) {
        self.check_if_on(cx);
        self.updates.hourly = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_EVERY).await;
                if this.update(cx, |this, cx| this.check_if_on(cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn check_if_on(&mut self, cx: &mut Context<Self>) {
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
                .border_color(colors.raised_border)
                .text_color(colors.accent)
                .cursor_pointer()
                .font_family(icons::FONT_FAMILY)
                .text_size(rems_from_px(10.0))
                .child(icons::UPDATE.to_string())
                // Checks again first: a newer release may be out by now.
                .on_click(cx.listener(|this, _, window, cx| this.check_for_updates(window, cx))),
        )
    }

    pub(super) fn offer_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Installs on a thread behind a box that only its Cancel (or Escape)
    /// closes, and only until the copying begins: the restart must not catch
    /// a file operation started meanwhile.
    fn install_update(
        &mut self,
        version: update::Version,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let log = self.commander.read(cx).log().cloned();
        if let Some(log) = &log {
            log.write(format_args!(
                "update started {} -> {version}",
                update::Version::current()
            ));
        }
        let (tx, rx) = mpsc::channel();
        let releases = self.updates.releases.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                let result = update::install(&releases, version, &cancel, |step| {
                    let _ = tx.send(Installing::Step(step));
                });
                let _ = tx.send(Installing::Done(result));
            }
        });
        let view = cx.new(|_| InstallingView {
            version,
            frame: 0,
            step: None,
            cancelling: false,
        });
        let progress = view.clone();
        let press_cancel: Rc<dyn Fn(&mut gpui_kit::App)> = Rc::new({
            let view = view.clone();
            move |cx| {
                view.update(cx, |view, cx| {
                    // Too late once the copying has begun.
                    if !matches!(view.step, Some(update::Step::Copying(_))) {
                        cancel.store(true, Ordering::Relaxed);
                        view.cancelling = true;
                        cx.notify();
                    }
                })
            }
        });
        let on_press: OnPress = Rc::new({
            let press_cancel = press_cancel.clone();
            move |_, cx| press_cancel(cx)
        });
        let buttons = ButtonRow::build([("Cancel", on_press)], 0, cx);
        let row = buttons.clone();
        // After the question's close has run.
        window.defer(cx, move |window, cx| {
            focus_when_open(buttons.focus_handle(cx), window, cx);
            window.open_dialog(cx, move |dialog, _, cx| {
                let press_cancel = press_cancel.clone();
                themed_dialog(dialog, cx)
                    .title("Update yagni-commander")
                    .w(crate::zoom::dialog_width(420.0, cx))
                    .close_button(false)
                    .overlay_closable(false)
                    .child(view.clone())
                    .footer(buttons.clone())
                    // Escape cancels too, and Enter (the dialog's own OK,
                    // which would close it), whatever has focus; the box
                    // closes when the thread ends.
                    .on_ok({
                        let press_cancel = press_cancel.clone();
                        move |_, _, cx| {
                            press_cancel(cx);
                            false
                        }
                    })
                    .on_cancel(move |_, _, cx| {
                        press_cancel(cx);
                        false
                    })
            });
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = 'run: loop {
                cx.background_executor().timer(FRAME).await;
                progress.update(cx, |view, cx| {
                    view.frame += 1;
                    cx.notify();
                });
                loop {
                    match rx.try_recv() {
                        Ok(Installing::Step(step)) => {
                            if let Some(log) = &log {
                                log.write(step_log_line(&step));
                            }
                            let copying = matches!(step, update::Step::Copying(_));
                            progress.update(cx, |view, cx| {
                                view.step = Some(step);
                                cx.notify();
                            });
                            if copying {
                                row.update(cx, |row, cx| {
                                    row.set_enabled(0, false);
                                    cx.notify();
                                });
                            }
                        }
                        Ok(Installing::Done(result)) => break 'run result,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            break 'run Err(update::InstallError::Failed(
                                "The update stopped unexpectedly.".to_owned(),
                            ));
                        }
                    }
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_installed(version, result, window, cx)
            });
        })
        .detach();
    }

    fn update_installed(
        &mut self,
        version: update::Version,
        result: Result<(), update::InstallError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_dialog(cx);
        let log = self.commander.read(cx).log().cloned();
        if let Some(log) = &log {
            log.write(install_log_line(
                update::Version::current(),
                version,
                &result,
            ));
        }
        let refocus = Some(self.focus.clone());
        let failure = match result {
            Err(update::InstallError::Cancelled) => {
                self.focus.focus(window, cx);
                return;
            }
            Err(update::InstallError::Failed(e)) => ("Update failed", e),
            Ok(()) => {
                let app = self.updates.app.clone().unwrap_or_default();
                match (self.updates.relaunch)(&app) {
                    Ok(()) => return cx.quit(),
                    Err(e) => {
                        if let Some(log) = &log {
                            log.write(format_args!("update restart failed: {}", one_line(&e)));
                        }
                        ("Updated, but not restarted", e)
                    }
                }
            }
        };
        // After the close above, which would close this box instead.
        window.defer(cx, move |window, cx| {
            show_error(failure.0, failure.1, refocus, window, cx)
        });
    }
}

/// The operation log's line for an install attempt's end.
fn install_log_line(
    current: update::Version,
    version: update::Version,
    result: &Result<(), update::InstallError>,
) -> String {
    match result {
        Ok(()) => format!("update installed {current} -> {version}"),
        Err(update::InstallError::Cancelled) => {
            format!("update cancelled {current} -> {version}")
        }
        Err(update::InstallError::Failed(e)) => {
            format!("update failed {current} -> {version}: {}", one_line(e))
        }
    }
}

/// The operation log's line for a step the installer reported.
fn step_log_line(step: &update::Step) -> String {
    match step {
        update::Step::Downloading(file) => format!("update downloading {}", one_line(file)),
        update::Step::Copying(place) => format!("update copying to {}", one_line(place)),
    }
}

/// The log has one line per entry; curl and the script may write several.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn update_question(latest: update::Version, current: update::Version) -> String {
    format!("yagni-commander {latest} is available (you have {current}). Install it and restart?")
}
