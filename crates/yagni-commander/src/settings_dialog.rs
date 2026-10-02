//! The Settings dialog (Ctrl-,). Every change applies at once and is saved
//! key by key (`FileManager::change_setting`). The text fields apply when
//! they lose focus and when the dialog closes (Close, Enter or Escape).

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, WindowExt};
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use yagni_commander_core::Setting;
use yagni_commander_core::config::parse_keep_days;

use crate::button_row::{ButtonRow, OnPress};
use crate::config_state::CurrentConfig;
use crate::file_manager::FileManager;
use crate::file_manager::commands::{focus_when_open, text_field};
use crate::theme::Theme;

pub struct SettingsView {
    file_manager: WeakEntity<FileManager>,
    editor: Entity<InputState>,
    days: Entity<InputState>,
    days_error: Option<&'static str>,
    /// Where the operation log is written, if there is a state folder.
    log_dir: Option<PathBuf>,
    /// The log folder's path was copied by clicking it.
    log_dir_copied: bool,
    /// Typed into since the last commit. Only an edited field is saved, so
    /// the text it opened with can't undo a hand edit of the file.
    editor_edited: bool,
    days_edited: bool,
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
        let subscriptions = vec![
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
            days_error: None,
            log_dir,
            log_dir_copied: false,
            editor_edited: false,
            days_edited: false,
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
    pub fn editor_text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn days_text(&self, cx: &App) -> String {
        self.days.read(cx).value().to_string()
    }

    fn editor_focus(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }

    fn change(&self, setting: Setting, window: &mut Window, cx: &mut App) {
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

    /// Before the dialog closes: saves the edited fields. False while the
    /// days field holds an invalid value.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.commit_editor(window, cx);
        self.commit_days(window, cx);
        self.days_error.is_none()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::get(cx).colors.clone();
        let current = cx.global::<CurrentConfig>();
        let (config, problem) = (current.config.clone(), current.problem.clone());
        let disabled = problem.is_some();
        let this = cx.entity().downgrade();
        let switch =
            |id: &'static str, label: &'static str, on: bool, make: fn(bool) -> Setting| {
                let this = this.clone();
                div().debug_selector(move || id.into()).child(
                    Switch::new(id)
                        .label(label)
                        .checked(on)
                        .disabled(disabled)
                        .on_click(move |checked, window, cx| {
                            let _ =
                                this.update(cx, |this, cx| this.change(make(*checked), window, cx));
                        }),
                )
            };
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .when_some(problem, |d, problem| {
                d.child(
                    div()
                        .debug_selector(|| "settings-problem".into())
                        .text_color(colors.error)
                        .child(format!("{problem}. Fix the file, then press Ctrl-R.")),
                )
            })
            .child(
                div().flex().flex_col().gap(px(4.0)).child("Editor").child(
                    div()
                        .debug_selector(|| "settings-editor".into())
                        .child(text_field(&self.editor)),
                ),
            )
            .child(switch(
                "settings-sort",
                "Sort names case-sensitively",
                config.case_sensitive_sort,
                Setting::CaseSensitiveSort,
            ))
            .child(switch(
                "settings-log",
                "Log file operations",
                config.log,
                Setting::Log,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child("Keep logs for (days)")
                    .child(
                        div()
                            .debug_selector(|| "settings-days".into())
                            .w(px(120.0))
                            .child(text_field(&self.days)),
                    )
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .text_size(px(12.0))
                            .child("Older log files are deleted at startup."),
                    )
                    .when_some(self.log_dir_text(), |d, text| {
                        d.child(
                            div()
                                .id("settings-log-dir")
                                .debug_selector(|| "settings-log-dir".into())
                                .text_color(colors.text_dim)
                                .text_size(px(12.0))
                                .cursor_pointer()
                                .hover(|style| style.text_color(colors.text))
                                .on_click(cx.listener(|this, _, _, cx| this.copy_log_dir(cx)))
                                .child(text),
                        )
                    })
                    .when_some(self.days_error, |d, message| {
                        d.child(
                            div()
                                .debug_selector(|| "settings-days-error".into())
                                .text_color(colors.error)
                                .child(message),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child("Columns")
                    .child(switch(
                        "settings-icons",
                        "Icons",
                        config.icons,
                        Setting::Icons,
                    ))
                    .child(switch(
                        "settings-modified",
                        "Modified",
                        config.show_modified,
                        Setting::ShowModified,
                    ))
                    .child(switch(
                        "settings-owner",
                        "Owner",
                        config.show_owner,
                        Setting::ShowOwner,
                    ))
                    .child(switch(
                        "settings-permissions",
                        "Permissions",
                        config.show_permissions,
                        Setting::ShowPermissions,
                    ))
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .text_size(px(12.0))
                            .child("Name and Size are always shown."),
                    ),
            )
    }
}

impl FileManager {
    /// Ctrl-, or the menu: the settings dialog.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_search(cx);
        let me = cx.entity().downgrade();
        let log_dir = self.log_dir.clone();
        let view = cx.new(|cx| SettingsView::new(me.clone(), log_dir, window, cx));
        self.settings = Some(view.clone());
        let focus = self.focus.clone();
        // Every way out saves the edited fields first, then gives focus
        // back. Enter and Close stay in the dialog while the days field is
        // invalid (its error shows); Escape leaves anyway.
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
        let on_close: OnPress = Rc::new({
            let (commit, closed) = (commit.clone(), closed.clone());
            move |window, cx| {
                if commit(window, cx) {
                    closed(window, cx);
                    window.close_dialog(cx);
                }
            }
        });
        let buttons = ButtonRow::build([("Close", on_close)], 0, cx);
        focus_when_open(view.read(cx).editor_focus(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let (commit_ok, closed_ok) = (commit.clone(), closed.clone());
            let (commit_cancel, closed_cancel) = (commit.clone(), closed.clone());
            dialog
                .title("Settings")
                .w(px(480.0))
                .close_button(false)
                .child(view.clone())
                .footer(buttons.clone())
                .on_ok(move |_, window, cx| {
                    let valid = commit_ok(window, cx);
                    if valid {
                        closed_ok(window, cx);
                    }
                    valid
                })
                .on_cancel(move |_, window, cx| {
                    commit_cancel(window, cx);
                    closed_cancel(window, cx);
                    true
                })
        });
    }
}
