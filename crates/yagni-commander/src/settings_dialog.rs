//! The Settings dialog (Ctrl-,). Every change applies at once and is saved
//! key by key (`FileManager::change_setting`), so it shows while choosing.
//! The text fields apply when they lose focus and on OK (or Enter). Cancel
//! (or Escape) puts back the file and the settings the dialog opened with
//! (`FileManager::revert_settings`).

use crate::file_manager::commands::themed_dialog;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, WindowExt};
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div, prelude::FluentBuilder,
};
use yagni_commander_core::config::Config;
use yagni_commander_core::config::parse_keep_days;
use yagni_commander_core::{Setting, hotlist};

use crate::button_row::{ButtonRow, OnLetter, OnPress, dialog_keys, marked_label};
use crate::config_state::CurrentConfig;
use crate::dropdown::{Dropdown, DropdownEvent};
use crate::file_manager::FileManager;
use crate::file_manager::commands::{focus_when_open, select_field, text_field};
use crate::theme::{self, Theme};

pub struct SettingsView {
    file_manager: WeakEntity<FileManager>,
    editor: Entity<InputState>,
    days: Entity<InputState>,
    /// The Theme dropdown: the built-ins' display names.
    theme: Entity<Dropdown>,
    days_error: Option<&'static str>,
    /// Where the operation log is written, if there is a state folder.
    log_dir: Option<PathBuf>,
    /// The log folder's path was copied by clicking it.
    log_dir_copied: bool,
    /// Typed into since the last commit. Only an edited field is saved, so
    /// the text it opened with can't undo a hand edit of the file.
    editor_edited: bool,
    days_edited: bool,
    /// Cancelled: nothing more is saved (a field's blur as the dialog
    /// closes would undo the revert).
    cancelled: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(
        file_manager: WeakEntity<FileManager>,
        log_dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = cx.global::<CurrentConfig>();
        let config = current.config.clone();
        let disabled = current.problem.is_some();
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder("e.g. code --wait")
                .default_value(config.editor.clone().unwrap_or_default());
            state.set_disabled(disabled, cx);
            state
        });
        let days = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).default_value(config.log_keep_days.to_string());
            state.set_disabled(disabled, cx);
            state
        });
        let themes = theme::ids_and_names();
        let current_name = Theme::named(config.theme.as_deref()).0.name;
        let selected = themes
            .iter()
            .position(|(_, name)| *name == current_name)
            .unwrap_or(0);
        let names: Vec<SharedString> = themes.iter().map(|(_, n)| n.clone().into()).collect();
        let theme = cx.new(|cx| {
            let mut dropdown = Dropdown::new("theme", names, selected, 200.0, cx);
            dropdown.set_disabled(disabled);
            dropdown
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &theme,
                window,
                move |this, _, DropdownEvent::Picked(ix), window, cx| {
                    let id = themes[*ix].0;
                    this.change(Setting::Theme(id.into()), window, cx);
                },
            ),
            cx.subscribe_in(
                &editor,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.editor_edited = true,
                    InputEvent::Blur => this.commit_editor(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &days,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.days_edited = true,
                    InputEvent::Blur => this.commit_days(window, cx),
                    _ => {}
                },
            ),
        ];
        Self {
            file_manager,
            editor,
            days,
            theme,
            days_error: None,
            log_dir,
            log_dir_copied: false,
            editor_edited: false,
            days_edited: false,
            cancelled: false,
            _subscriptions: subscriptions,
        }
    }

    /// The line under the log settings that says where the logs are.
    /// Clicking it copies the path.
    pub fn log_dir_text(&self) -> Option<String> {
        let dir = self.log_dir.as_ref()?;
        let hint = if self.log_dir_copied {
            "copied"
        } else {
            "click to copy"
        };
        Some(format!(
            "Log files are stored in {} ({hint})",
            dir.display()
        ))
    }

    fn copy_log_dir(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = &self.log_dir {
            cx.write_to_clipboard(ClipboardItem::new_string(dir.display().to_string()));
            self.log_dir_copied = true;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn theme_open(&self, cx: &App) -> bool {
        self.theme.read(cx).is_open()
    }

    #[cfg(test)]
    pub fn theme_value(&self, cx: &App) -> Option<String> {
        let ix = self.theme.read(cx).selected();
        theme::ids_and_names().get(ix).map(|(_, name)| name.clone())
    }

    /// The text fields by name, for tab-order tests (the switches have no
    /// handle of ours).
    #[cfg(test)]
    pub fn tab_stops(&self, cx: &App) -> Vec<(&'static str, gpui_kit::FocusHandle)> {
        vec![
            ("editor", self.editor.focus_handle(cx)),
            ("days", self.days.focus_handle(cx)),
            ("theme", self.theme.focus_handle(cx)),
        ]
    }

    #[cfg(test)]
    pub fn editor_text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn days_text(&self, cx: &App) -> String {
        self.days.read(cx).value().to_string()
    }

    fn change(&self, setting: Setting, window: &mut Window, cx: &mut App) {
        if self.cancelled {
            return;
        }
        let _ = self
            .file_manager
            .update(cx, |fm, cx| fm.change_setting(setting, window, cx));
    }

    fn commit_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.editor_edited) {
            return;
        }
        let text = self.editor.read(cx).value().to_string();
        self.change(Setting::editor(&text), window, cx);
    }

    fn commit_days(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.days_edited {
            return;
        }
        let text = self.days.read(cx).value().to_string();
        match parse_keep_days(&text) {
            Ok(days) => {
                self.days_edited = false;
                self.days_error = None;
                self.change(Setting::LogKeepDays(days), window, cx);
            }
            Err(message) => self.days_error = Some(message),
        }
        cx.notify();
    }

    /// Alt and a label's letter: Theme opens, a text field gets focus with
    /// its text selected, a switch toggles (the focus stays). Nothing while
    /// the file has a problem (the controls are disabled). Whether the
    /// letter is the body's.
    pub fn alt(&mut self, letter: char, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let current = cx.global::<CurrentConfig>();
        let (config, disabled) = (current.config.clone(), current.problem.is_some());
        let switch = GENERAL_SWITCHES
            .iter()
            .chain(&PANEL_SWITCHES)
            .find(|s| hotlist::label(s.1).letter == Some(letter));
        if !(switch.is_some() || "ted".contains(letter)) {
            return false;
        }
        if disabled {
            return true;
        }
        match (letter, switch) {
            (_, Some(&(_, _, value, make))) => self.change(make(!value(&config)), window, cx),
            ('t', _) => self.theme.update(cx, |d, cx| d.open(window, cx)),
            ('e', _) => select_field(&self.editor, window, cx),
            _ => select_field(&self.days, window, cx),
        }
        cx.notify();
        true
    }

    /// Before the dialog closes: saves the edited fields. False while the
    /// days field holds an invalid value.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.commit_editor(window, cx);
        self.commit_days(window, cx);
        self.days_error.is_none()
    }
}

/// A switch: debug selector (also its id), label with its Alt letter, its
/// value in the config, the setting it saves.
type SwitchDef = (
    &'static str,
    &'static str,
    fn(&Config) -> bool,
    fn(bool) -> Setting,
);

/// The general column's switches, in order (Theme, Editor and the days
/// field take T, E and D).
const GENERAL_SWITCHES: [SwitchDef; 3] = [
    (
        "settings-sort",
        "&Sort names case-sensitively",
        |c| c.case_sensitive_sort,
        Setting::CaseSensitiveSort,
    ),
    (
        "settings-updates",
        "Check for &updates at startup",
        |c| c.check_updates,
        Setting::CheckUpdates,
    ),
    (
        "settings-log",
        "&Log file operations",
        |c| c.log,
        Setting::Log,
    ),
];

/// The Panels column's switches, in order.
const PANEL_SWITCHES: [SwitchDef; 5] = [
    ("settings-icons", "&Icons", |c| c.icons, Setting::Icons),
    ("settings-git", "&Git", |c| c.git, Setting::Git),
    (
        "settings-modified",
        "&Modified",
        |c| c.show_modified,
        Setting::ShowModified,
    ),
    (
        "settings-owner",
        "&Owner",
        |c| c.show_owner,
        Setting::ShowOwner,
    ),
    (
        "settings-permissions",
        "&Permissions",
        |c| c.show_permissions,
        Setting::ShowPermissions,
    ),
];

/// The dialog's width at 100%: two columns, or one in a narrow window.
const TWO_COLUMNS_WIDTH: f32 = 640.0;
const ONE_COLUMN_WIDTH: f32 = 480.0;
/// Room the window keeps beside the two-column dialog (px, not zoomed).
const WINDOW_MARGIN: f32 = 32.0;

/// Two columns (general settings left, Panels right) while the window has
/// room for them at the current zoom, else one; checked on every render.
fn two_columns(window: &Window, cx: &App) -> bool {
    window.viewport_size().width
        >= crate::zoom::dialog_width(TWO_COLUMNS_WIDTH, cx) + gpui_kit::px(WINDOW_MARGIN)
}

fn dialog_width(window: &Window, cx: &App) -> gpui_kit::Pixels {
    let width = if two_columns(window, cx) {
        TWO_COLUMNS_WIDTH
    } else {
        ONE_COLUMN_WIDTH
    };
    crate::zoom::dialog_width(width, cx)
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let current = cx.global::<CurrentConfig>();
        let (config, problem) = (current.config.clone(), current.problem.clone());
        let disabled = problem.is_some();
        let two = two_columns(window, cx);
        let this = cx.entity().downgrade();
        // Our own label beside the switch: gpui-component's can't underline
        // a letter. Clicking it, or the row's empty end, toggles too.
        let switch = |&(id, label, value, make): &SwitchDef| {
            let on = value(&config);
            let this = this.clone();
            let toggle = move |checked: bool, window: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |this, cx| this.change(make(checked), window, cx));
            };
            let by_label = toggle.clone();
            div()
                .debug_selector(move || id.into())
                .flex()
                .items_start()
                .gap(crate::zoom::rems_from_px(8.0))
                .child(
                    Switch::new(id)
                        .accessibility_label(hotlist::label(label).text)
                        .checked(on)
                        .disabled(disabled)
                        .on_click(move |checked, window, cx| toggle(*checked, window, cx)),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-label")))
                        .flex_1()
                        .min_w_0()
                        .line_height(crate::zoom::rems_from_px(20.0))
                        .when(disabled, |d| d.text_color(colors.text_dim))
                        .when(!disabled, |d| {
                            d.cursor_pointer()
                                .on_click(move |_, window, cx| by_label(!on, window, cx))
                        })
                        .child(marked_label(label)),
                )
        };
        let column = || div().flex().flex_col().gap(crate::zoom::rems_from_px(12.0));
        let hint = |text: SharedString| {
            div()
                .text_color(colors.text_secondary)
                .text_size(crate::zoom::rems_from_px(12.0))
                .child(text)
        };
        // Under both columns when there are two, so the long path wraps less.
        let log_dir = self.log_dir_text().map(|text| {
            div()
                .id("settings-log-dir")
                .debug_selector(|| "settings-log-dir".into())
                .text_color(colors.text_secondary)
                .text_size(crate::zoom::rems_from_px(12.0))
                .cursor_pointer()
                .hover(|style| style.text_color(colors.text))
                .on_click(cx.listener(|this, _, _, cx| this.copy_log_dir(cx)))
                .child(text)
        });
        let (log_dir_inside, log_dir_below) = if two {
            (None, log_dir)
        } else {
            (log_dir, None)
        };

        let general = column()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(crate::zoom::rems_from_px(4.0))
                    .child(marked_label("&Theme"))
                    .child(
                        div()
                            .debug_selector(|| "settings-theme".into())
                            .w(crate::zoom::rems_from_px(200.0))
                            .child(self.theme.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(crate::zoom::rems_from_px(4.0))
                    .child(marked_label("&Editor"))
                    .child(
                        div()
                            .debug_selector(|| "settings-editor".into())
                            .child(text_field(&self.editor)),
                    ),
            )
            .children(GENERAL_SWITCHES.iter().map(switch))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(crate::zoom::rems_from_px(4.0))
                    .child(marked_label("Keep logs for (&days)"))
                    .child(
                        div()
                            .debug_selector(|| "settings-days".into())
                            .w(crate::zoom::rems_from_px(120.0))
                            .child(text_field(&self.days)),
                    )
                    .child(hint("Older log files are deleted at startup.".into()))
                    .children(log_dir_inside)
                    .when_some(self.days_error, |d, message| {
                        d.child(
                            div()
                                .debug_selector(|| "settings-days-error".into())
                                .text_color(colors.error)
                                .child(message),
                        )
                    }),
            );
        let panels = div()
            .flex()
            .flex_col()
            .gap(crate::zoom::rems_from_px(8.0))
            .child("Panels")
            .children(PANEL_SWITCHES.iter().map(switch))
            .child(hint("Name and Size are always shown.".into()));

        column()
            // A dialog body clips at its bottom edge.
            .pb(crate::zoom::rems_from_px(8.0))
            .when_some(problem, |d, problem| {
                d.child(
                    div()
                        .debug_selector(|| "settings-problem".into())
                        .text_color(colors.error)
                        .child(format!("{problem}. Fix the file, then press Ctrl-R.")),
                )
            })
            .map(|d| {
                if two {
                    d.child(
                        div()
                            .flex()
                            .gap(crate::zoom::rems_from_px(24.0))
                            .child(general.flex_1().min_w_0())
                            .child(panels.flex_1().min_w_0()),
                    )
                } else {
                    d.child(general).child(panels)
                }
            })
            .children(log_dir_below)
    }
}

/// The settings and the file's text as the dialog found them, for Cancel.
#[derive(Clone)]
pub(crate) struct Snapshot {
    config: Config,
    /// `None` without a readable file (then nothing was saved either).
    text: Option<String>,
}

impl FileManager {
    /// Cancel or Escape in the settings dialog: writes the file back as it
    /// was (only if it changed) and applies its settings. A failed write
    /// shows an error box.
    pub(crate) fn revert_settings(
        &mut self,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = cx.global::<CurrentConfig>();
        if current.problem.is_some() {
            return;
        }
        if let (Some(path), Some(text)) = (current.path.clone(), &snapshot.text)
            && std::fs::read_to_string(&path).ok().as_ref() != Some(text)
            && let Err(e) = yagni_commander_core::storage::write_atomic(&path, text)
        {
            // Deferred: the settings dialog is closing, which would close
            // this box instead.
            let message = e.to_string();
            window.defer(cx, move |window, cx| {
                crate::file_manager::commands::show_error(
                    "Settings not restored",
                    message,
                    None,
                    window,
                    cx,
                )
            });
        }
        self.apply_config(snapshot.config.clone(), cx);
    }

    /// Ctrl-, or the menu: the settings dialog.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let me = cx.entity().downgrade();
        let log_dir = self.log_dir.clone();
        let view = cx.new(|cx| SettingsView::new(me.clone(), log_dir, window, cx));
        self.settings = Some(view.clone());
        let focus = self.focus.clone();
        let current = cx.global::<CurrentConfig>();
        let snapshot = Snapshot {
            config: current.config.clone(),
            text: current
                .path
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok()),
        };
        // OK saves the edited fields, then gives focus back; while the days
        // field is invalid it stays in the dialog (its error shows).
        let commit = {
            let view = view.clone();
            move |window: &mut Window, cx: &mut App| view.update(cx, |v, cx| v.commit(window, cx))
        };
        let closed = {
            let me = me.clone();
            move |window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |fm, _| fm.settings = None);
                let focus = focus.clone();
                window.defer(cx, move |window, cx| focus.focus(window, cx));
            }
        };
        // Cancel puts everything back, the typed fields dropped.
        let revert = {
            let (view, me, closed) = (view.clone(), me.clone(), closed.clone());
            move |window: &mut Window, cx: &mut App| {
                view.update(cx, |v, _| v.cancelled = true);
                let _ = me.update(cx, |fm, cx| fm.revert_settings(&snapshot, window, cx));
                closed(window, cx);
            }
        };
        let on_ok: OnPress = Rc::new({
            let (commit, closed) = (commit.clone(), closed.clone());
            move |window, cx| {
                if commit(window, cx) {
                    closed(window, cx);
                    window.close_dialog(cx);
                }
            }
        });
        let on_cancel: OnPress = Rc::new({
            let revert = revert.clone();
            move |window, cx| {
                revert(window, cx);
                window.close_dialog(cx);
            }
        });
        let buttons = ButtonRow::build([("Cancel", on_cancel), ("OK", on_ok)], 1, cx);
        let alt: OnLetter = Rc::new({
            let view = view.clone();
            move |letter, window, cx| view.update(cx, |v, cx| v.alt(letter, window, cx))
        });
        // The buttons have focus, OK selected, so Enter or Space closes at
        // once (owner, 2026-10-04); Tab reaches the controls.
        focus_when_open(buttons.read(cx).focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, window, cx| {
            let (commit_ok, closed_ok) = (commit.clone(), closed.clone());
            let revert = revert.clone();
            themed_dialog(dialog, cx)
                .title("Settings")
                .w(dialog_width(window, cx))
                .close_button(false)
                .child(dialog_keys(
                    div().child(view.clone()),
                    &buttons,
                    Some(alt.clone()),
                ))
                .footer(dialog_keys(
                    div().child(buttons.clone()),
                    &buttons,
                    Some(alt.clone()),
                ))
                .on_ok(move |_, window, cx| {
                    let valid = commit_ok(window, cx);
                    if valid {
                        closed_ok(window, cx);
                    }
                    valid
                })
                .on_cancel(move |_, window, cx| {
                    revert(window, cx);
                    true
                })
        });
    }
}
