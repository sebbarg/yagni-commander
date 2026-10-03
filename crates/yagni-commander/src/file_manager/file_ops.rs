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
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, WeakEntity, Window, div, px,
};
use yagni_commander_core::file_ops::{
    Answer, Conflict, Destination, Event, Incoming, Job, LinkAnswer, LinkChoice, LinkPlace,
    LinkQuestion, Operation, Progress, Report, Settings, is_archive,
};
use yagni_commander_core::{Command, Side, format_modified, format_size};

use super::FileManager;
use super::commands::{Prompt, focus_when_open, show_error, show_message, stem_range};
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
        }
    }
}

/// The job in progress and its dialog.
pub(super) struct RunningJob {
    job: Job,
    kind: Kind,
    /// The panel the sources came from.
    source: Side,
    /// The folder the sources came from.
    source_dir: PathBuf,
    view: Entity<ProgressView>,
    polls: u32,
    progress_open: bool,
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
        // The destination is the other panel's folder: wait for it too.
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
        let title = format!("{} {} to", kind.verb(), describe(&sources));
        let error_title = match kind {
            Kind::Copy => "Cannot copy",
            _ => "Cannot move",
        };
        // One entry: its full target path, so it can be renamed on the way.
        // More: the target folder.
        let (initial, selection) = match sources.as_slice() {
            [one] => single_target(&other_dir, one),
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
                let operation = match kind {
                    Kind::Copy => Operation::Copy { sources, to },
                    _ => Operation::Move { sources, to },
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
    pub(super) fn trash(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
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
            log: self.commander.read(cx).log().cloned(),
        };
        let job = Job::spawn(operation, settings)?;
        let commander = self.commander.read(cx);
        let source = commander.active();
        let source_dir = commander.panel(source).path().to_path_buf();
        self.job = Some(RunningJob {
            job,
            kind,
            source,
            source_dir,
            view: cx.new(|_| ProgressView::new()),
            polls: 0,
            progress_open: false,
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
                Event::Finished(report) => {
                    self.finish_job(report, window, cx);
                    return false;
                }
            }
        }
        if !running.progress_open
            && (conflict.is_some() || link.is_some() || running.polls >= POLLS_BEFORE_PROGRESS)
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
            let dim = Theme::get(cx).colors.text_dim;
            let this_cancel = this.clone();
            dialog
                .title("File exists")
                .w(px(560.0))
                .close_button(false)
                .overlay_closable(false)
                .child(question.clone())
                .child(
                    div()
                        .text_color(dim)
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
            let dim = Theme::get(cx).colors.text_dim;
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
                        .text_color(dim)
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

    fn finish_job(&mut self, report: Report, window: &mut Window, cx: &mut Context<Self>) {
        let Some(running) = self.job.take() else {
            return;
        };
        if running.progress_open {
            window.close_dialog(cx);
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
    if to.exists() && !to.is_dir() {
        return Err(invalid(format!("“{typed}” is not a directory")));
    }
    if same(&to, dir) {
        return Err(invalid(
            "source and destination are the same directory".into(),
        ));
    }
    Ok(Destination::Into(to))
}

/// The F5/F6 field for one source: its path in `other_dir`, with the name
/// (up to the extension, like F2) preselected.
fn single_target(other_dir: &Path, source: &Path) -> (String, std::ops::Range<usize>) {
    let name = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let text = other_dir.join(&name).display().to_string();
    let is_dir = source.symlink_metadata().is_ok_and(|m| m.is_dir());
    let stem = stem_range(&name, is_dir);
    let start = text.len() - name.len();
    (text, start + stem.start..start + stem.end)
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
                    .text_color(colors.text_dim)
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
        let (text, selection) = single_target(Path::new("/other"), Path::new("/here/notes.txt"));
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
