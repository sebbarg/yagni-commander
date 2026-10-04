//! F5 copy, F6 move and F8 trash: the dialogs around a background
//! [`Job`]. One job runs at a time. While it runs, a timer polls its events:
//! progress updates the progress dialog, a conflict opens the Overwrite/Skip
//! prompt, and the end reloads both panels and reports any failures.

use std::cell::Cell;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::InputState;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, WeakEntity, Window, div, px,
};
use yagni_commander_core::archive::inner_parts;
use yagni_commander_core::file_ops::{
    Answer, Comparison, Conflict, Destination, Difference, Event, Incoming, Job, LinkAnswer,
    LinkChoice, LinkPlace, LinkQuestion, Operation, PasswordAnswer, PasswordQuestion, Progress,
    Report, Settings, is_archive,
};
use yagni_commander_core::{
    Command, Commander, Entry, EntryKind, Panel, PanelId, Side, format_modified, format_size,
};

use super::FileManager;
use super::commands::{
    Prompt, dialog_password_field, focus_when_open, show_error, show_message, show_message_box,
    stem_range,
};
use crate::button_row::{ButtonRow, OnPress};
use crate::theme::Theme;

/// How often a running job's events are read.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Polls before the progress dialog opens, so quick jobs don't flash one.
const POLLS_BEFORE_PROGRESS: u32 = 6;
/// Failures listed in the summary before "and N more".
const MAX_LISTED_FAILURES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Copy,
    Move,
    Trash,
    Delete,
    Pack,
    Extract,
    Compare,
    /// F3 inside an archive: a private copy for the viewer.
    View,
}

impl Kind {
    fn verb(self) -> &'static str {
        match self {
            Kind::Copy => "Copy",
            Kind::Move => "Move",
            Kind::Trash => "Move to trash",
            Kind::Delete => "Delete",
            Kind::Pack => "Pack",
            Kind::Extract => "Extract",
            Kind::Compare => "Compare",
            Kind::View => "View",
        }
    }

    fn progress_title(self) -> &'static str {
        match self {
            Kind::Copy => "Copying",
            Kind::Move => "Moving",
            Kind::Trash => "Moving to trash",
            Kind::Delete => "Deleting",
            Kind::Pack => "Packing",
            Kind::Extract => "Extracting",
            Kind::Compare => "Comparing",
            Kind::View => "Extracting",
        }
    }
}

/// F5 inside an archive: the entries to copy out.
struct FromArchive {
    archive: PathBuf,
    inner: Vec<std::ffi::OsString>,
    names: Vec<std::ffi::OsString>,
}

/// The job in progress and its dialog.
pub(super) struct RunningJob {
    job: Job,
    kind: Kind,
    /// The tab the sources came from.
    source: PanelId,
    /// The folder the sources came from.
    source_dir: PathBuf,
    view: Entity<ProgressView>,
    polls: u32,
    progress_open: bool,
    /// What a compare runs on, for its result.
    compare: Option<ComparePair>,
    /// F3 inside an archive: where the copy goes.
    viewing: Option<Viewing>,
}

/// A private copy of an archive entry for the viewer. Dropping it (Cancel,
/// a failure, the viewer window closing) deletes the folder.
struct Viewing {
    dir: tempfile::TempDir,
    file: PathBuf,
}

#[cfg(test)]
impl RunningJob {
    pub(super) fn progress_open(&self) -> bool {
        self.progress_open
    }

    pub(super) fn cancelling(&self, cx: &App) -> bool {
        self.view.read(cx).cancelling
    }
}

impl FileManager {
    /// F5 and F6: ask for the destination (the other panel's directory by
    /// default), then run the job.
    pub(super) fn copy_or_move(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        if kind == Kind::Move && self.refuse_in_archive(window, cx) {
            return;
        }
        // The destination is the other panel's folder: wait for it too.
        let other = self.commander.read(cx).active().other();
        if self.commander.read(cx).panel(other).loading().is_some() {
            return;
        }
        self.end_search(cx);
        let (sources, dir, other_dir, one_is_dir, from_archive) = {
            let commander = self.commander.read(cx);
            let panel = commander.panel(commander.active());
            let other = commander.panel(commander.active().other());
            let targets = panel.targets();
            let one_is_dir = matches!(targets.as_slice(), [one] if one.kind == EntryKind::Dir);
            // Inside an archive the sources are entries of it.
            let from_archive = panel.archive().map(|index| FromArchive {
                archive: index.file().to_path_buf(),
                inner: inner_parts(index.file(), panel.path()),
                names: targets.iter().map(|e| e.name.clone()).collect(),
            });
            (
                source_paths(panel),
                // Typed relative paths start from a real folder.
                panel.real_dir().to_path_buf(),
                other.path().to_path_buf(),
                one_is_dir,
                from_archive,
            )
        };
        if sources.is_empty() || self.refuse_second_job(window, cx) {
            return;
        }
        let title = format!("{} {} to", kind.verb(), describe(&sources));
        let error_title = match kind {
            Kind::Copy => "Cannot copy",
            _ => "Cannot move",
        };
        // One entry: its full target path, so it can be renamed on the way.
        // More: the target folder.
        let (initial, selection) = match sources.as_slice() {
            [one] => single_target(&other_dir, one, one_is_dir),
            _ => {
                let text = other_dir.display().to_string();
                let end = text.len();
                (text, end..end)
            }
        };
        self.prompt_name(
            Prompt {
                title: &title,
                error_title,
                initial: &initial,
                selection,
                width: COPY_PROMPT_WIDTH,
            },
            Rc::new(move |this, typed, window, cx| {
                let to = destination(&dir, typed, &sources)?;
                let sources = sources.clone();
                let operation = match (&from_archive, kind) {
                    (Some(from), _) => Operation::ExtractEntries {
                        archive: from.archive.clone(),
                        inner: from.inner.clone(),
                        names: from.names.clone(),
                        to,
                    },
                    (None, Kind::Copy) => Operation::Copy { sources, to },
                    (None, _) => Operation::Move { sources, to },
                };
                this.start_job(kind, operation, window, cx)
            }),
            window,
            cx,
        );
    }

    /// Alt-F5: ask for the zip's path (in the other panel by default), then
    /// pack the selection or the entry under the cursor into it. An
    /// existing zip is replaced only after a confirm box.
    pub(super) fn pack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        if self.refuse_in_archive(window, cx) {
            return;
        }
        let other = self.commander.read(cx).active().other();
        if self.commander.read(cx).panel(other).loading().is_some() {
            return;
        }
        self.end_search(cx);
        let (sources, dir, other_dir) = {
            let commander = self.commander.read(cx);
            let panel = commander.panel(commander.active());
            let other = commander.panel(commander.active().other());
            (
                source_paths(panel),
                panel.path().to_path_buf(),
                other.path().to_path_buf(),
            )
        };
        if sources.is_empty() || self.refuse_second_job(window, cx) {
            return;
        }
        let title = format!("Pack {} to", describe(&sources));
        let (initial, selection) = zip_target(&other_dir, &sources, &dir);
        self.prompt_name(
            Prompt {
                title: &title,
                error_title: "Cannot pack",
                initial: &initial,
                selection,
                width: COPY_PROMPT_WIDTH,
            },
            Rc::new(move |this, typed, window, cx| {
                let to = zip_path(&dir, typed)?;
                let operation = Operation::Pack {
                    sources: sources.clone(),
                    base: dir.clone(),
                    to: to.clone(),
                };
                if to.exists() {
                    // After the prompt has closed: closing pops the top dialog.
                    let this = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        confirm_overwrite(this, to, operation, window, cx)
                    });
                    return Ok(());
                }
                this.start_job(Kind::Pack, operation, window, cx)
            }),
            window,
            cx,
        );
    }

    /// Alt-F6 / Alt-F9: ask for the folder (the other panel's by default),
    /// then extract the selected archives or the one under the cursor.
    pub(super) fn extract(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        if self.refuse_in_archive(window, cx) {
            return;
        }
        let other = self.commander.read(cx).active().other();
        if self.commander.read(cx).panel(other).loading().is_some() {
            return;
        }
        self.end_search(cx);
        let (archives, dir, other_dir) = {
            let commander = self.commander.read(cx);
            let panel = commander.panel(commander.active());
            let other = commander.panel(commander.active().other());
            (
                source_paths(panel),
                panel.path().to_path_buf(),
                other.path().to_path_buf(),
            )
        };
        if archives.is_empty() || self.refuse_second_job(window, cx) {
            return;
        }
        if !archives.iter().any(|path| is_archive(path)) {
            let message = match archives.as_slice() {
                [one] => format!(
                    "“{}” is not an archive.",
                    one.file_name().unwrap_or_default().to_string_lossy()
                ),
                _ => "None of the selected entries is an archive.".to_owned(),
            };
            show_error(
                "Cannot extract",
                message,
                Some(self.focus.clone()),
                window,
                cx,
            );
            return;
        }
        let title = extract_title(&archives);
        let initial = other_dir.display().to_string();
        let end = initial.len();
        self.prompt_name(
            Prompt {
                title: &title,
                error_title: "Cannot extract",
                initial: &initial,
                selection: end..end,
                width: COPY_PROMPT_WIDTH,
            },
            Rc::new(move |this, typed, window, cx| {
                let into = extract_folder(&dir, typed)?;
                let operation = Operation::Extract {
                    archives: archives.clone(),
                    into,
                };
                this.start_job(Kind::Extract, operation, window, cx)
            }),
            window,
            cx,
        );
    }

    /// F8 and Del: confirm, then move to the trash. Shift-F8 and Shift-Del
    /// (`Kind::Delete`): confirm, then delete permanently.
    /// Files menu, Compare by content: two selected entries in the active
    /// panel, else one from each panel; the result in a message box.
    pub(super) fn compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        let other = self.commander.read(cx).active().other();
        if self.commander.read(cx).panel(other).loading().is_some() {
            return;
        }
        self.end_search(cx);
        if self.refuse_second_job(window, cx) {
            return;
        }
        let pair = match compare_pair(self.commander.read(cx)) {
            Ok(pair) => pair,
            Err(message) => {
                show_error("Cannot compare", message, None, window, cx);
                return;
            }
        };
        if inside_archive(&pair.first) || inside_archive(&pair.second) {
            show_error(
                "Inside an archive",
                yagni_commander_core::archive::IN_ARCHIVE,
                Some(self.focus.clone()),
                window,
                cx,
            );
            return;
        }
        let operation = Operation::Compare {
            first: pair.first.clone(),
            second: pair.second.clone(),
        };
        match self.start_job(Kind::Compare, operation, window, cx) {
            Ok(()) => {
                if let Some(running) = &mut self.job {
                    running.compare = Some(pair);
                }
            }
            Err(e) => show_error("Cannot compare", e.to_string(), None, window, cx),
        }
    }

    /// F3 inside an archive: extracts the entry under the cursor into a
    /// fresh private folder; `finish_job` opens the viewer on it.
    pub(super) fn view_in_archive(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.refuse_second_job(window, cx) {
            return Ok(());
        }
        let root = self
            .temp_dir
            .clone()
            .ok_or_else(|| io::Error::other("no folder for temporary files"))?;
        yagni_commander_core::storage::private_dir(&root)?;
        let dir = tempfile::Builder::new().prefix("view-").tempdir_in(&root)?;
        let (archive, inner, name) = {
            let panel = self.active_panel(cx);
            let index = panel.archive().expect("view_file checked");
            let entry = panel.cursor_entry().expect("view_file checked");
            (
                index.file().to_path_buf(),
                inner_parts(index.file(), panel.path()),
                entry.name.clone(),
            )
        };
        let file = dir.path().join(&name);
        let operation = Operation::ExtractEntries {
            archive,
            inner,
            names: vec![name],
            to: Destination::As(file.clone()),
        };
        self.start_job(Kind::View, operation, window, cx)?;
        if let Some(running) = &mut self.job {
            running.viewing = Some(Viewing { dir, file });
        }
        Ok(())
    }

    pub(super) fn trash(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        if self.refuse_in_archive(window, cx) {
            return;
        }
        self.end_search(cx);
        let sources = {
            let commander = self.commander.read(cx);
            source_paths(commander.panel(commander.active()))
        };
        if sources.is_empty() || self.refuse_second_job(window, cx) {
            return;
        }
        let delete = kind == Kind::Delete;
        let (title, question, error_title) = if delete {
            (
                "Delete permanently",
                format!(
                    "Delete {} permanently? This cannot be undone.",
                    describe(&sources)
                ),
                "Cannot delete",
            )
        } else {
            (
                "Move to trash",
                format!("Move {} to the trash?", describe(&sources)),
                "Cannot move to trash",
            )
        };
        let question = SharedString::from(question);
        let this = cx.entity().downgrade();
        let focus = self.focus.clone();
        let cancel: OnPress = Rc::new({
            let focus = focus.clone();
            move |window, cx| {
                window.close_dialog(cx);
                focus.focus(window, cx);
            }
        });
        let confirm: OnPress = Rc::new(move |window, cx| {
            window.close_dialog(cx);
            let sources = sources.clone();
            let operation = if delete {
                Operation::Delete { sources }
            } else {
                Operation::Trash { sources }
            };
            let result = this
                .update(cx, |this, cx| this.start_job(kind, operation, window, cx))
                .unwrap_or(Ok(()));
            if let Err(e) = result {
                show_error(error_title, e.to_string(), None, window, cx);
            }
        });
        // Enter confirms, like TC's F8 and Shift-F8.
        let label = if delete { "Delete" } else { "Move to trash" };
        let buttons = ButtonRow::build([("Cancel", cancel), (label, confirm)], 1, cx);
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let focus_cancel = focus.clone();
            dialog
                .title(title)
                .w(px(420.0))
                .close_button(false)
                .child(question.clone())
                .footer(buttons.clone())
                .on_cancel(move |_, window, cx| {
                    focus_cancel.focus(window, cx);
                    true
                })
        });
    }

    fn refuse_second_job(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.job.is_none() {
            return false;
        }
        show_error(
            "Operation in progress",
            "Wait for the current operation to finish.",
            None,
            window,
            cx,
        );
        true
    }

    fn start_job(
        &mut self,
        kind: Kind,
        operation: Operation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let settings = Settings {
            trash: self.trash,
            // A viewer's private copy is not a change worth logging.
            log: (kind != Kind::View)
                .then(|| self.commander.read(cx).log().cloned())
                .flatten(),
        };
        let job = Job::spawn(operation, settings)?;
        let commander = self.commander.read(cx);
        let panel = commander.panel(commander.active());
        let source = panel.id();
        let source_dir = panel.path().to_path_buf();
        self.job = Some(RunningJob {
            job,
            kind,
            source,
            source_dir,
            view: cx.new(|_| ProgressView::new()),
            polls: 0,
            progress_open: false,
            compare: None,
            viewing: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let running = this.update_in(cx, |this, window, cx| this.poll_job(window, cx));
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        Ok(())
    }

    /// Handles the job's pending events. Returns whether it is still running.
    fn poll_job(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(running) = &mut self.job else {
            return false;
        };
        running.polls += 1;
        let mut conflict = None;
        let mut link = None;
        let mut password = None;
        while let Some(event) = running.job.try_event() {
            match event {
                Event::Progress(progress) => running.view.update(cx, |view, cx| {
                    view.progress = progress;
                    cx.notify();
                }),
                // The worker waits for the answer, so nothing follows yet.
                Event::Conflict(c) => {
                    conflict = Some(c);
                    break;
                }
                Event::Link(question) => {
                    link = Some(question);
                    break;
                }
                Event::Password(question) => {
                    password = Some(question);
                    break;
                }
                Event::Finished(report) => {
                    self.finish_job(report, window, cx);
                    return false;
                }
            }
        }
        if !running.progress_open
            && (conflict.is_some()
                || link.is_some()
                || password.is_some()
                || running.polls >= POLLS_BEFORE_PROGRESS)
        {
            running.progress_open = true;
            let (kind, view) = (running.kind, running.view.clone());
            self.open_progress(kind, view, window, cx);
        }
        if let Some(conflict) = conflict {
            self.ask_conflict(&conflict, window, cx);
        }
        if let Some(question) = link {
            self.ask_link(&question, window, cx);
        }
        if let Some(question) = password {
            self.ask_password(&question, window, cx);
        }
        true
    }

    fn open_progress(
        &mut self,
        kind: Kind,
        view: Entity<ProgressView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let cancel: OnPress = Rc::new({
            let this = this.clone();
            move |_, cx| cancel_job(&this, cx)
        });
        let buttons = ButtonRow::build([("Cancel", cancel)], 0, cx);
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let this = this.clone();
            dialog
                .title(kind.progress_title())
                .w(px(480.0))
                .close_button(false)
                .overlay_closable(false)
                .child(view.clone())
                .footer(buttons.clone())
                // Escape cancels too; the dialog closes when the job ends.
                .on_cancel(move |_, _, cx| {
                    cancel_job(&this, cx);
                    false
                })
        });
    }

    fn ask_conflict(&mut self, conflict: &Conflict, window: &mut Window, cx: &mut Context<Self>) {
        let name = conflict
            .target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let place = conflict
            .target
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let question = SharedString::from(format!("“{name}” already exists in {place}."));
        let new_text = match conflict.incoming {
            Some(incoming) => describe_incoming(incoming),
            None => describe_file(&conflict.source),
        };
        let new = SharedString::from(format!("New:       {new_text}"));
        let old = SharedString::from(format!("Existing: {}", describe_file(&conflict.target)));
        let this = cx.entity().downgrade();
        let button = |label: &'static str, answer: Answer| -> (&'static str, OnPress) {
            let this = this.clone();
            let on_press: OnPress = Rc::new(move |window, cx| {
                answer_job(&this, answer, cx);
                window.close_dialog(cx);
            });
            (label, on_press)
        };
        // Overwrite is preselected, like TC: Enter replaces the target.
        let buttons = ButtonRow::build(
            [
                button("Overwrite", Answer::Overwrite),
                button("Overwrite all", Answer::OverwriteAll),
                button("Skip", Answer::Skip),
                button("Skip all", Answer::SkipAll),
                button("Cancel", Answer::Cancel),
            ],
            0,
            cx,
        );
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let secondary = Theme::get(cx).colors.text_secondary;
            let this_cancel = this.clone();
            dialog
                .title("File exists")
                .w(px(560.0))
                .close_button(false)
                .overlay_closable(false)
                .child(question.clone())
                .child(
                    div()
                        .text_color(secondary)
                        .text_size(px(13.0))
                        .child(
                            div()
                                .debug_selector(|| "conflict-new".into())
                                .child(new.clone()),
                        )
                        .child(old.clone()),
                )
                .footer(buttons.clone())
                .on_cancel(move |_, _, cx| {
                    answer_job(&this_cancel, Answer::Cancel, cx);
                    true
                })
        });
    }

    fn ask_link(&mut self, question: &LinkQuestion, window: &mut Window, cx: &mut Context<Self>) {
        let place = match question.place {
            LinkPlace::Inside => "inside what you are packing",
            LinkPlace::Outside => "outside what you are packing",
            LinkPlace::Missing => "its target does not exist",
        };
        let text = SharedString::from(format!(
            "{}  ->  {}",
            question.shown.display(),
            question.target.display()
        ));
        let place = SharedString::from(format!("({place})"));
        let for_all = Rc::new(Cell::new(false));
        let this = cx.entity().downgrade();
        let button = |label: &'static str, choice: LinkChoice| -> (&'static str, OnPress) {
            let (this, for_all) = (this.clone(), for_all.clone());
            let on_press: OnPress = Rc::new(move |window, cx| {
                let answer = LinkAnswer {
                    choice,
                    for_all: for_all.get(),
                };
                answer_link_job(&this, answer, cx);
                window.close_dialog(cx);
            });
            (label, on_press)
        };
        // Follow is preselected: Enter packs what the link points to.
        let buttons = ButtonRow::build(
            [
                button("Follow", LinkChoice::Follow),
                button("Store as link", LinkChoice::Store),
                button("Leave out", LinkChoice::LeaveOut),
                button("Cancel", LinkChoice::Cancel),
            ],
            0,
            cx,
        );
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let secondary = Theme::get(cx).colors.text_secondary;
            let this_cancel = this.clone();
            let checked = for_all.get();
            let for_all = for_all.clone();
            dialog
                .title("Symbolic link")
                .w(px(560.0))
                .close_button(false)
                .overlay_closable(false)
                .child(
                    div()
                        .debug_selector(|| "link-prompt".into())
                        .child(text.clone()),
                )
                .child(
                    div()
                        .text_color(secondary)
                        .text_size(px(13.0))
                        .child(place.clone()),
                )
                .child(
                    div()
                        .debug_selector(|| "link-all".into())
                        .pt(px(8.0))
                        .child(
                            Checkbox::new("link-all")
                                .label("Same for the remaining links")
                                .checked(checked)
                                .on_click(move |on, window, _| {
                                    for_all.set(*on);
                                    window.refresh();
                                }),
                        ),
                )
                .footer(buttons.clone())
                .on_cancel(move |_, _, cx| {
                    let answer = LinkAnswer {
                        choice: LinkChoice::Cancel,
                        for_all: false,
                    };
                    answer_link_job(&this_cancel, answer, cx);
                    true
                })
        });
    }

    fn ask_password(
        &mut self,
        question: &PasswordQuestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = question
            .archive
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let text = SharedString::from(format!("“{name}” is password-protected."));
        let retry = question.retry;
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let this = cx.entity().downgrade();
        // Sends the typed password; the caller closes the dialog.
        let submit: Rc<dyn Fn(&mut App)> = Rc::new({
            let (this, input) = (this.clone(), input.clone());
            move |cx| {
                let password = input.read(cx).value().to_string();
                answer_password_job(&this, PasswordAnswer::Password(password), cx);
            }
        });
        let ok: OnPress = Rc::new({
            let submit = submit.clone();
            move |window, cx| {
                submit(cx);
                window.close_dialog(cx);
            }
        });
        let button = |answer: PasswordAnswer| -> OnPress {
            let this = this.clone();
            Rc::new(move |window, cx| {
                answer_password_job(&this, answer.clone(), cx);
                window.close_dialog(cx);
            })
        };
        let buttons = ButtonRow::build(
            [
                ("OK", ok),
                ("Skip archive", button(PasswordAnswer::Skip)),
                ("Cancel", button(PasswordAnswer::Cancel)),
            ],
            0,
            cx,
        );
        // Opening the dialog moves focus to it, so focus the field afterwards.
        window.defer(cx, {
            let input = input.clone();
            move |window, cx| input.update(cx, |state, cx| state.focus(window, cx))
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let secondary = Theme::get(cx).colors.text_secondary;
            let this_cancel = this.clone();
            let submit = submit.clone();
            dialog
                .title("Password")
                .w(px(480.0))
                .close_button(false)
                .overlay_closable(false)
                .child(
                    div()
                        .debug_selector(|| "password-prompt".into())
                        .child(text.clone()),
                )
                .children(retry.then(|| {
                    div()
                        .debug_selector(|| "password-wrong".into())
                        .text_color(secondary)
                        .text_size(px(13.0))
                        .child("Wrong password.")
                }))
                .child(dialog_password_field(&input))
                .footer(buttons.clone())
                // Enter in the field.
                .on_ok(move |_, _, cx| {
                    submit(cx);
                    true
                })
                .on_cancel(move |_, _, cx| {
                    answer_password_job(&this_cancel, PasswordAnswer::Cancel, cx);
                    true
                })
        });
    }

    fn finish_job(&mut self, report: Report, window: &mut Window, cx: &mut Context<Self>) {
        let Some(running) = self.job.take() else {
            return;
        };
        if running.progress_open {
            window.close_dialog(cx);
        }
        if running.kind == Kind::View {
            // Reads only: no reload, the selection stays.
            if !report.failures.is_empty() {
                show_error(
                    "Cannot view file",
                    failure_summary(&report),
                    Some(self.focus.clone()),
                    window,
                    cx,
                );
            } else if let (Some(viewing), false) = (running.viewing, report.cancelled) {
                let main = window.window_bounds();
                if let Err(e) =
                    crate::viewer_view::open(viewing.file, main, Some(viewing.dir), None, cx)
                {
                    show_error(
                        "Cannot view file",
                        e.to_string(),
                        Some(self.focus.clone()),
                        window,
                        cx,
                    );
                }
            }
            return;
        }
        if running.kind == Kind::Compare {
            // Reads only: no reload, the selection stays.
            if let (Some(pair), Some(comparison), false) =
                (&running.compare, &report.comparison, report.cancelled)
            {
                let shown = comparison_text(comparison, pair, Some(MAX_LISTED_DIFFERENCES));
                let copied = format!("Compare\n{}", comparison_text(comparison, pair, None));
                show_message_box(
                    "Compare",
                    shown.into(),
                    copied,
                    COMPARE_BOX_WIDTH,
                    "OK",
                    None,
                    window,
                    cx,
                );
            }
            return;
        }
        let complete = report.failures.is_empty() && !report.cancelled;
        if matches!(running.kind, Kind::Copy | Kind::Pack) && complete {
            self.commander.update(cx, |commander, _| {
                commander.clear_selection(running.source, &running.source_dir)
            });
        }
        self.execute(Command::Reload, cx);
        let left_out = left_out_summary(&report);
        if !report.failures.is_empty() {
            let title = match running.kind {
                Kind::Copy => "Some entries were not copied",
                Kind::Move => "Some entries were not moved",
                Kind::Trash => "Some entries were not moved to the trash",
                Kind::Delete => "Some entries were not deleted",
                Kind::Pack => "Some entries were not packed",
                Kind::Extract => "Some entries were not extracted",
                Kind::Compare | Kind::View => unreachable!("reported above"),
            };
            let mut text = failure_summary(&report);
            if let Some(left_out) = &left_out {
                text.push_str(
                    "

",
                );
                text.push_str(left_out);
            }
            show_error(title, text, None, window, cx);
        } else if let Some(text) = left_out {
            show_message("Links left out", text, "OK", None, window, cx);
        }
    }
}

fn cancel_job(this: &WeakEntity<FileManager>, cx: &mut App) {
    let _ = this.update(cx, |this, cx| {
        if let Some(running) = &this.job {
            running.job.cancel();
            running.view.update(cx, |view, cx| {
                view.cancelling = true;
                cx.notify();
            });
        }
    });
}

fn answer_job(this: &WeakEntity<FileManager>, answer: Answer, cx: &mut App) {
    let _ = this.update(cx, |this, _| {
        if let Some(running) = &this.job {
            running.job.answer(answer);
        }
    });
}

/// Differences listed in the compare result before "and N more".
const MAX_LISTED_DIFFERENCES: usize = 20;
/// Width of the compare result: room for paths.
const COMPARE_BOX_WIDTH: f32 = 640.0;

/// The two entries a compare runs on, and how the result names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ComparePair {
    first: PathBuf,
    second: PathBuf,
    /// "left"/"right", or the two names when both come from one panel.
    labels: [String; 2],
    folders: bool,
}

/// What Compare by content runs on: two selected entries in the active
/// panel, else one entry from each panel (its one selected entry, or the
/// cursor entry when nothing is selected), left panel first.
fn compare_pair(commander: &Commander) -> Result<ComparePair, &'static str> {
    const SELECT: &str = "Select two files or two folders to compare.";
    let at = |panel: &Panel, entry: &Entry| (panel.path().join(&entry.name), entry.kind);
    let active = commander.panel(commander.active());
    let selected: Vec<&Entry> = active.selection().collect();
    let ((first, x), (second, y), labels) = if let [a, b] = selected[..] {
        let labels = [a.label.clone(), b.label.clone()];
        (at(active, a), at(active, b), labels)
    } else {
        let pick = |side: Side| {
            let panel = commander.panel(side);
            let selected: Vec<&Entry> = panel.selection().collect();
            let entry = match selected[..] {
                [] => panel
                    .cursor_entry()
                    .filter(|e| e.kind != EntryKind::Parent)?,
                [one] => one,
                _ => return None,
            };
            Some(at(panel, entry))
        };
        let (Some(left), Some(right)) = (pick(Side::Left), pick(Side::Right)) else {
            return Err(SELECT);
        };
        (left, right, ["left".to_owned(), "right".to_owned()])
    };
    if x != y {
        return Err("Cannot compare a file with a folder.");
    }
    Ok(ComparePair {
        first,
        second,
        labels,
        folders: x == EntryKind::Dir,
    })
}

/// The compare result: identical, or the differences (at most `limit`
/// lines, then "and N more").
fn comparison_text(comparison: &Comparison, pair: &ComparePair, limit: Option<usize>) -> String {
    let unreadable = |d: &Difference| matches!(d, Difference::Unreadable { .. });
    if !pair.folders {
        return if comparison.identical() {
            "The files are identical.".to_owned()
        } else if comparison.differences.iter().any(unreadable) {
            let lines: Vec<String> = comparison
                .differences
                .iter()
                .map(difference_line(pair))
                .collect();
            format!("Could not compare the files:\n{}", lines.join("\n"))
        } else {
            "The files differ.".to_owned()
        };
    }
    if comparison.identical() {
        let files = match comparison.files {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        return format!("The folders are identical ({files}).");
    }
    let heading = if comparison.differences.iter().all(unreadable) {
        "The folders could not be fully compared:"
    } else {
        "The folders differ:"
    };
    let total = comparison.differences.len();
    let shown = limit.unwrap_or(total).min(total);
    let mut lines = vec![heading.to_owned()];
    lines.extend(
        comparison.differences[..shown]
            .iter()
            .map(difference_line(pair)),
    );
    if shown < total {
        lines.push(format!("and {} more", total - shown));
    }
    lines.join("\n")
}

fn difference_line(pair: &ComparePair) -> impl Fn(&Difference) -> String + '_ {
    move |difference| {
        let only = |label: &str, path: &Path, is_dir: bool| {
            let slash = if is_dir { "/" } else { "" };
            format!("only in {label}: {}{slash}", path.display())
        };
        match difference {
            Difference::OnlyFirst { path, is_dir } => only(&pair.labels[0], path, *is_dir),
            Difference::OnlySecond { path, is_dir } => only(&pair.labels[1], path, *is_dir),
            Difference::Content(path) => format!("different: {}", path.display()),
            Difference::Type(path) => format!("different type: {}", path.display()),
            Difference::Unreadable { path, message } => {
                format!("could not read: {}: {message}", path.display())
            }
        }
    }
}

/// Paths of the panel's targets (selection, else the entry under the cursor).
fn source_paths(panel: &yagni_commander_core::Panel) -> Vec<PathBuf> {
    panel
        .targets()
        .iter()
        .map(|entry| panel.path().join(&entry.name))
        .collect()
}

/// "“name”" for one source, "3 entries" for more.
fn describe(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!(
            "“{}”",
            one.file_name().unwrap_or_default().to_string_lossy()
        ),
        many => format!("{} entries", many.len()),
    }
}

/// Size and modification time, for the conflict prompt.
fn describe_file(path: &Path) -> String {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_symlink() => "symlink".to_owned(),
        Ok(meta) => {
            let modified = meta.modified().map(format_modified).unwrap_or_default();
            format!("{}, {modified}", format_size(meta.len()))
        }
        Err(e) => e.to_string(),
    }
}

/// The Alt-F5 field: a zip in `other_dir` named after the one source (a
/// file without its extension, like F2) or after the current folder; the
/// name before `.zip` is preselected.
fn zip_target(other_dir: &Path, sources: &[PathBuf], current: &Path) -> (String, Range<usize>) {
    let stem = match sources {
        [one] => {
            let name = one
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let is_dir = one.symlink_metadata().is_ok_and(|m| m.is_dir());
            name[stem_range(&name, is_dir)].to_owned()
        }
        _ => current.file_name().map_or_else(
            || "archive".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        ),
    };
    let text = other_dir.join(format!("{stem}.zip")).display().to_string();
    let end = text.len() - ".zip".len();
    (text, end - stem.len()..end)
}

/// The zip the Alt-F5 prompt names: relative to `dir`, `.zip` added when
/// the name doesn't end in it.
fn zip_path(dir: &Path, typed: &str) -> io::Result<PathBuf> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(invalid("give a name for the archive".into()));
    }
    let mut path = dir.join(typed);
    if inside_archive(&path) {
        return Err(invalid("Can't pack into an archive".into()));
    }
    if typed.ends_with('/') || path.is_dir() {
        return Err(invalid(format!("“{typed}” is a folder")));
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".zip");
        path.set_file_name(name);
    }
    Ok(path)
}

/// "Links left out:" and their paths, at most [`MAX_LISTED_FAILURES`].
fn left_out_summary(report: &Report) -> Option<String> {
    if report.left_out.is_empty() {
        return None;
    }
    let mut lines = vec!["Links left out:".to_owned()];
    lines.extend(
        report
            .left_out
            .iter()
            .take(MAX_LISTED_FAILURES)
            .map(|path| path.display().to_string()),
    );
    let more = report.left_out.len().saturating_sub(MAX_LISTED_FAILURES);
    if more > 0 {
        lines.push(format!("and {more} more"));
    }
    Some(lines.join("\n"))
}

/// Size and time of a file coming from an archive, for the conflict prompt.
fn describe_incoming(incoming: Incoming) -> String {
    let modified = incoming.modified.map(format_modified).unwrap_or_default();
    format!("{}, {modified}", format_size(incoming.size))
}

/// The zip exists: replace it only after a confirm (Overwrite preselected).
fn confirm_overwrite(
    this: WeakEntity<FileManager>,
    to: PathBuf,
    operation: Operation,
    window: &mut Window,
    cx: &mut App,
) {
    let name = to
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let question = SharedString::from(format!("“{name}” exists. Overwrite it?"));
    let focus = this.upgrade().map(|fm| fm.read(cx).focus.clone());
    let cancel: OnPress = Rc::new({
        let focus = focus.clone();
        move |window, cx| {
            window.close_dialog(cx);
            if let Some(focus) = &focus {
                focus.focus(window, cx);
            }
        }
    });
    let overwrite: OnPress = Rc::new(move |window, cx| {
        window.close_dialog(cx);
        let operation = operation.clone();
        let result = this
            .update(cx, |this, cx| {
                this.start_job(Kind::Pack, operation, window, cx)
            })
            .unwrap_or(Ok(()));
        if let Err(e) = result {
            show_error("Cannot pack", e.to_string(), None, window, cx);
        }
    });
    let buttons = ButtonRow::build([("Cancel", cancel), ("Overwrite", overwrite)], 1, cx);
    focus_when_open(buttons.focus_handle(cx), window, cx);
    window.open_dialog(cx, move |dialog, _, _| {
        let focus = focus.clone();
        dialog
            .title("Archive exists")
            .w(px(420.0))
            .close_button(false)
            .child(question.clone())
            .footer(buttons.clone())
            .on_cancel(move |_, window, cx| {
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                true
            })
    });
}

fn answer_password_job(this: &WeakEntity<FileManager>, answer: PasswordAnswer, cx: &mut App) {
    let _ = this.update(cx, |this, _| {
        if let Some(running) = &this.job {
            running.job.answer_password(answer);
        }
    });
}

fn answer_link_job(this: &WeakEntity<FileManager>, answer: LinkAnswer, cx: &mut App) {
    let _ = this.update(cx, |this, _| {
        if let Some(running) = &this.job {
            running.job.answer_link(answer);
        }
    });
}

/// "Extract “name” to", "Extract 3 archives to", or "3 entries" when some
/// selected entries are not archives (they fail in the summary).
fn extract_title(archives: &[PathBuf]) -> String {
    if archives.len() > 1 && archives.iter().all(|path| is_archive(path)) {
        format!("Extract {} archives to", archives.len())
    } else {
        format!("Extract {} to", describe(archives))
    }
}

/// The folder the Alt-F6 prompt names, relative to `dir`; created by the job.
fn extract_folder(dir: &Path, typed: &str) -> io::Result<PathBuf> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(invalid("give a folder".into()));
    }
    let into = dir.join(typed);
    if is_or_inside_archive(&into) {
        return Err(invalid("Can't extract into an archive".into()));
    }
    if into.exists() && !into.is_dir() {
        return Err(invalid(format!("“{typed}” is not a folder")));
    }
    Ok(into)
}

/// Width of the F5/F6 prompt: room for long paths.
const COPY_PROMPT_WIDTH: f32 = 720.0;

/// Where the text typed into the F5/F6 prompt sends `sources`, relative to
/// the active panel's directory `dir`. One source: an existing folder (or
/// text ending in "/") means into it, anything else is its new full path.
/// More: into the typed folder. Missing folders are created by the job.
fn destination(dir: &Path, typed: &str, sources: &[PathBuf]) -> io::Result<Destination> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(invalid("give a destination".into()));
    }
    let to = dir.join(typed);
    if inside_archive(&to) {
        return Err(invalid("Can't copy into an archive".into()));
    }
    let same = |a: &Path, b: &Path| {
        a.canonicalize()
            .ok()
            .is_some_and(|a| Some(a) == b.canonicalize().ok())
    };
    if let [one] = sources {
        let into = typed.ends_with('/') || to.is_dir();
        let target = match (into, one.file_name()) {
            (true, Some(name)) => to.join(name),
            _ => to.clone(),
        };
        if same(one, &target) {
            return Err(invalid("source and target are the same".into()));
        }
        return Ok(if into {
            Destination::Into(to)
        } else {
            Destination::As(to)
        });
    }
    if is_or_inside_archive(&to) {
        return Err(invalid("Can't copy into an archive".into()));
    }
    if to.exists() && !to.is_dir() {
        return Err(invalid(format!("“{typed}” is not a directory")));
    }
    // The sources' own folder: inside an archive it can't exist, so
    // extracting into the archive's folder is allowed.
    let sources_dir = sources.first().and_then(|s| s.parent()).unwrap_or(dir);
    if same(&to, sources_dir) {
        return Err(invalid(
            "source and destination are the same directory".into(),
        ));
    }
    Ok(Destination::Into(to))
}

/// The F5/F6 field for one source: its path in `other_dir`, with the name
/// (up to the extension, like F2) preselected.
fn single_target(
    other_dir: &Path,
    source: &Path,
    is_dir: bool,
) -> (String, std::ops::Range<usize>) {
    let name = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let text = other_dir.join(&name).display().to_string();
    let stem = stem_range(&name, is_dir);
    let start = text.len() - name.len();
    (text, start + stem.start..start + stem.end)
}

/// Whether `path` is below an archive file (a panel inside an archive
/// shows such paths): nothing can be written there.
pub(super) fn inside_archive(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|p| is_archive(p) && p.is_file())
}

/// Whether `path` is an archive file or below one: the target of a panel
/// at an archive's root, or inside it.
fn is_or_inside_archive(path: &Path) -> bool {
    inside_archive(path) || (is_archive(path) && path.is_file())
}

/// One line per failure, at most [`MAX_LISTED_FAILURES`].
fn failure_summary(report: &Report) -> String {
    let mut lines: Vec<String> = report
        .failures
        .iter()
        .take(MAX_LISTED_FAILURES)
        .map(|f| format!("{}: {}", f.path.display(), f.message))
        .collect();
    let more = report.failures.len().saturating_sub(MAX_LISTED_FAILURES);
    if more > 0 {
        lines.push(format!("and {more} more"));
    }
    lines.join("\n")
}

/// The progress dialog's contents.
pub(super) struct ProgressView {
    progress: Progress,
    cancelling: bool,
}

impl ProgressView {
    fn new() -> Self {
        Self {
            progress: Progress::default(),
            cancelling: false,
        }
    }
}

impl Render for ProgressView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let status = if self.cancelling {
            "Cancelling...".to_owned()
        } else {
            progress_text(&self.progress)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_start()
                    .child(self.progress.current.display().to_string()),
            )
            .child(ProgressBar::new("progress").value(fraction(&self.progress) * 100.0))
            .child(
                div()
                    .text_color(colors.text_secondary)
                    .text_size(px(13.0))
                    .child(status),
            )
    }
}

/// Share of the work done. Each file counts as its bytes plus a fixed cost,
/// so both one huge file and many small ones move the bar. Without files
/// (a rename-only move, trash) it counts top-level entries.
fn fraction(progress: &Progress) -> f32 {
    const FILE_COST: u64 = 16 * 1024;
    let (done, total) = if progress.files_total > 0 {
        (
            progress.bytes_done + progress.files_done as u64 * FILE_COST,
            progress.bytes_total + progress.files_total as u64 * FILE_COST,
        )
    } else {
        (progress.items_done as u64, progress.items_total as u64)
    };
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64).min(1.0) as f32
    }
}

fn progress_text(progress: &Progress) -> String {
    if progress.files_total > 0 && progress.bytes_total == 0 {
        format!(
            "{} of {} files",
            progress.files_done.min(progress.files_total),
            progress.files_total
        )
    } else if progress.files_total > 0 {
        format!(
            "{} of {} files, {} of {}",
            progress.files_done.min(progress.files_total),
            progress.files_total,
            format_size(progress.bytes_done.min(progress.bytes_total)),
            format_size(progress.bytes_total)
        )
    } else {
        format!("Entry {} of {}", progress.items_done, progress.items_total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yagni_commander_core::file_ops::Failure;

    fn progress(items: (usize, usize), files: (usize, usize), bytes: (u64, u64)) -> Progress {
        Progress {
            items_done: items.0,
            items_total: items.1,
            files_done: files.0,
            files_total: files.1,
            bytes_done: bytes.0,
            bytes_total: bytes.1,
            current: PathBuf::new(),
        }
    }

    #[test]
    fn fraction_weighs_files_and_bytes_else_counts_entries() {
        assert_eq!(fraction(&progress((1, 4), (0, 0), (0, 0))), 0.25);
        assert_eq!(fraction(&progress((0, 1), (0, 0), (0, 0))), 0.0);
        assert_eq!(fraction(&progress((0, 0), (0, 0), (0, 0))), 0.0);
        // Many empty files still move the bar.
        assert_eq!(fraction(&progress((0, 1), (500, 1000), (0, 0))), 0.5);
        // One huge file is mostly its bytes.
        let half = fraction(&progress((0, 1), (0, 1), (1 << 30, 1 << 31)));
        assert!((0.49..0.5).contains(&half), "{half}");
        assert_eq!(fraction(&progress((0, 1), (2, 1), (2048, 1024))), 1.0);
    }

    #[test]
    fn progress_text_counts_files_and_bytes_or_entries() {
        assert_eq!(
            progress_text(&progress((0, 1), (3, 60), (512, 2048))),
            "3 of 60 files, 512 B of 2.0 KiB"
        );
        assert_eq!(
            progress_text(&progress((2, 3), (0, 0), (0, 0))),
            "Entry 2 of 3"
        );
        assert_eq!(
            progress_text(&progress((0, 1), (3, 60), (0, 0))),
            "3 of 60 files"
        );
    }

    #[test]
    fn describe_names_one_source_or_counts_many() {
        assert_eq!(describe(&[PathBuf::from("/a/notes.txt")]), "“notes.txt”");
        assert_eq!(
            describe(&[PathBuf::from("/a/x"), PathBuf::from("/a/y")]),
            "2 entries"
        );
    }

    #[test]
    fn one_source_goes_to_the_typed_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("f.txt"), b"").unwrap();
        let one = [dir.join("f.txt")];
        let to = |typed: &str| destination(dir, typed, &one);
        // A new name, here or anywhere (missing folders are made later).
        assert_eq!(to(" g.txt ").unwrap(), Destination::As(dir.join("g.txt")));
        assert_eq!(
            to("new/sub/g.txt").unwrap(),
            Destination::As(dir.join("new/sub/g.txt"))
        );
        // An existing folder, or a trailing slash: into it, keeping the name.
        assert_eq!(to("sub").unwrap(), Destination::Into(dir.join("sub")));
        assert_eq!(to("new/").unwrap(), Destination::Into(dir.join("new/")));
        let absolute = dir.join("sub").display().to_string();
        assert_eq!(to(&absolute).unwrap(), Destination::Into(dir.join("sub")));
        // Not onto itself, not empty.
        assert!(to("f.txt").is_err());
        assert!(to(".").is_err(), "into its own folder is onto itself");
        assert!(to("").is_err());
    }

    #[test]
    fn many_sources_go_into_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("file"), b"").unwrap();
        let two = [dir.join("x"), dir.join("y")];
        let to = |typed: &str| destination(dir, typed, &two);
        assert_eq!(to(" sub ").unwrap(), Destination::Into(dir.join("sub")));
        assert_eq!(to("new").unwrap(), Destination::Into(dir.join("new")));
        assert!(to("file").is_err(), "not a directory");
        assert!(to(".").is_err(), "the same directory");
        assert!(to("").is_err());
        assert!(
            destination(
                &dir.join("sub"),
                "..",
                &[dir.join("sub/x"), dir.join("sub/y")]
            )
            .is_ok()
        );
    }

    #[test]
    fn a_single_source_starts_with_its_full_target_path() {
        let (text, selection) =
            single_target(Path::new("/other"), Path::new("/here/notes.txt"), false);
        assert_eq!(text, "/other/notes.txt");
        assert_eq!(&text[selection], "notes");
    }

    #[test]
    fn failure_summary_lists_the_first_failures() {
        let failures = (0..12)
            .map(|i| Failure {
                path: PathBuf::from(format!("/f{i}")),
                message: "denied".into(),
            })
            .collect();
        let report = Report {
            failures,
            ..Report::default()
        };
        let summary = failure_summary(&report);
        assert!(summary.starts_with("/f0: denied\n/f1: denied"));
        assert!(summary.ends_with("/f9: denied\nand 2 more"));
    }

    #[test]
    fn describe_file_gives_size_and_time_or_the_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        std::fs::write(&file, b"12345").unwrap();
        assert!(describe_file(&file).starts_with("5 B, "));
        assert!(!describe_file(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn the_zip_is_named_after_the_entry_or_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("photos")).unwrap();
        let other = Path::new("/o");
        let (text, sel) = zip_target(other, &[tmp.path().join("notes.txt")], tmp.path());
        assert_eq!(text, "/o/notes.zip");
        assert_eq!(&text[sel], "notes");
        let (text, sel) = zip_target(other, &[tmp.path().join("photos")], tmp.path());
        assert_eq!(text, "/o/photos.zip");
        assert_eq!(&text[sel], "photos");
        let (text, sel) = zip_target(
            other,
            &[tmp.path().join("a"), tmp.path().join("b")],
            Path::new("/home/me/work"),
        );
        assert_eq!(text, "/o/work.zip");
        assert_eq!(&text[sel], "work");
        let (text, _) = zip_target(
            other,
            &[PathBuf::from("/a"), PathBuf::from("/b")],
            Path::new("/"),
        );
        assert_eq!(text, "/o/archive.zip");
    }

    #[test]
    fn zip_is_added_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(zip_path(dir, "out").unwrap(), dir.join("out.zip"));
        assert_eq!(zip_path(dir, " out.zip ").unwrap(), dir.join("out.zip"));
        assert_eq!(zip_path(dir, "OUT.ZIP").unwrap(), dir.join("OUT.ZIP"));
        assert_eq!(zip_path(dir, "sub/x").unwrap(), dir.join("sub/x.zip"));
        assert_eq!(
            zip_path(dir, "/abs/y.zip").unwrap(),
            PathBuf::from("/abs/y.zip")
        );
        assert!(zip_path(dir, "  ").is_err());
        assert!(zip_path(dir, "folder/").is_err());
        std::fs::create_dir(dir.join("existing")).unwrap();
        assert!(zip_path(dir, "existing").is_err());
    }

    #[test]
    fn left_out_links_are_listed() {
        let mut report = Report::default();
        assert_eq!(left_out_summary(&report), None);
        report.left_out = (0..12).map(|i| PathBuf::from(format!("/l{i}"))).collect();
        let text = left_out_summary(&report).unwrap();
        assert!(text.starts_with("Links left out:\n/l0\n"), "{text}");
        assert!(text.ends_with("/l9\nand 2 more"), "{text}");
    }

    #[test]
    fn incoming_entries_show_size_and_time() {
        let incoming = Incoming {
            size: 2048,
            modified: None,
        };
        assert_eq!(
            describe_incoming(incoming),
            format!("{}, ", format_size(2048))
        );
    }

    #[test]
    fn the_extract_folder_is_relative_and_must_be_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert_eq!(
            extract_folder(dir, " out/here ").unwrap(),
            dir.join("out/here")
        );
        assert_eq!(extract_folder(dir, "/abs").unwrap(), PathBuf::from("/abs"));
        assert!(extract_folder(dir, "").is_err());
        std::fs::write(dir.join("file"), "").unwrap();
        assert!(extract_folder(dir, "file").is_err());
    }

    fn pair(folders: bool) -> ComparePair {
        ComparePair {
            first: PathBuf::from("/l/x"),
            second: PathBuf::from("/r/x"),
            labels: ["left".into(), "right".into()],
            folders,
        }
    }

    fn unreadable(path: &str) -> Difference {
        Difference::Unreadable {
            path: PathBuf::from(path),
            message: "denied".into(),
        }
    }

    #[test]
    fn compare_text_for_files() {
        let mut comparison = Comparison {
            files: 1,
            differences: vec![],
        };
        let text = |c: &Comparison| comparison_text(c, &pair(false), Some(20));
        assert_eq!(text(&comparison), "The files are identical.");
        comparison.differences = vec![Difference::Content(PathBuf::new())];
        assert_eq!(text(&comparison), "The files differ.");
        comparison.differences = vec![unreadable("/r/x")];
        assert_eq!(
            text(&comparison),
            "Could not compare the files:\ncould not read: /r/x: denied"
        );
    }

    #[test]
    fn compare_text_for_folders_lists_up_to_the_limit() {
        let mut comparison = Comparison {
            files: 1,
            differences: vec![],
        };
        let text = |c: &Comparison, limit| comparison_text(c, &pair(true), limit);
        assert_eq!(
            text(&comparison, Some(20)),
            "The folders are identical (1 file)."
        );
        comparison.files = 0;
        assert_eq!(
            text(&comparison, Some(20)),
            "The folders are identical (0 files)."
        );
        comparison.differences = vec![unreadable("/l/x/s")];
        assert_eq!(
            text(&comparison, Some(20)),
            "The folders could not be fully compared:\ncould not read: /l/x/s: denied"
        );
        comparison.differences = vec![
            Difference::OnlyFirst {
                path: PathBuf::from("d"),
                is_dir: true,
            },
            Difference::OnlySecond {
                path: PathBuf::from("e/f"),
                is_dir: false,
            },
            Difference::Type(PathBuf::from("t")),
            Difference::Content(PathBuf::from("u")),
            unreadable("/l/x/v"),
        ];
        assert_eq!(
            text(&comparison, Some(3)),
            "The folders differ:\nonly in left: d/\nonly in right: e/f\ndifferent type: t\nand 2 more"
        );
        assert!(
            text(&comparison, None).ends_with("different: u\ncould not read: /l/x/v: denied"),
            "no limit lists all"
        );
    }

    #[test]
    fn nothing_is_packed_extracted_or_copied_into_an_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let zip = dir.join("a.zip");
        std::fs::write(&zip, b"").unwrap();
        let message = |r: io::Result<PathBuf>| r.unwrap_err().to_string();
        assert_eq!(
            message(zip_path(dir, "a.zip/out.zip")),
            "Can't pack into an archive"
        );
        assert_eq!(
            message(extract_folder(dir, "a.zip/sub")),
            "Can't extract into an archive"
        );
        assert_eq!(
            message(extract_folder(dir, "a.zip")),
            "Can't extract into an archive"
        );
        let two = [dir.join("x"), dir.join("y")];
        let err = destination(dir, "a.zip", &two).unwrap_err();
        assert_eq!(err.to_string(), "Can't copy into an archive");
        // One entry onto an existing zip replaces it (after Overwrite?).
        let one = [dir.join("x")];
        assert!(matches!(
            destination(dir, "a.zip", &one),
            Ok(Destination::As(_))
        ));
    }

    #[test]
    fn inside_archive_means_below_an_archive_file() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("a.zip");
        std::fs::write(&zip, b"").unwrap();
        std::fs::create_dir(tmp.path().join("dir.zip")).unwrap();
        assert!(inside_archive(&zip.join("x")));
        assert!(inside_archive(&zip.join("x/y")));
        assert!(!inside_archive(&zip), "the archive itself is a file");
        assert!(!inside_archive(&tmp.path().join("dir.zip/x")));
        assert!(!inside_archive(&tmp.path().join("plain/x")));
    }

    #[test]
    fn the_extract_title_counts_archives() {
        assert_eq!(
            extract_title(&[PathBuf::from("/a/p.zip")]),
            "Extract “p.zip” to"
        );
        let two = [PathBuf::from("/a/p.zip"), PathBuf::from("/a/q.tgz")];
        assert_eq!(extract_title(&two), "Extract 2 archives to");
        let mixed = [PathBuf::from("/a/p.zip"), PathBuf::from("/a/n.txt")];
        assert_eq!(extract_title(&mixed), "Extract 2 entries to");
    }
}
