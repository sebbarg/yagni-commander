//! F5 copy, F6 move and F8 trash: the dialogs around a background
//! [`Job`]. One job runs at a time. While it runs, a timer polls its events:
//! progress updates the progress dialog, a conflict opens the Overwrite/Skip
//! prompt, and the end reloads both panels and reports any failures.

use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, IntoElement, ParentElement, Render, SharedString,
    Styled, WeakEntity, Window, div, px,
};
use yagni_commander_core::file_ops::{
    Answer, Conflict, Event, Job, Operation, Progress, Report, Settings,
};
use yagni_commander_core::{Command, Side, format_modified, format_size};

use super::FileManager;
use super::commands::{Prompt, focus_when_open, show_error};
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
}

impl Kind {
    fn verb(self) -> &'static str {
        match self {
            Kind::Copy => "Copy",
            Kind::Move => "Move",
            Kind::Trash => "Move to trash",
            Kind::Delete => "Delete",
        }
    }

    fn progress_title(self) -> &'static str {
        match self {
            Kind::Copy => "Copying",
            Kind::Move => "Moving",
            Kind::Trash => "Moving to trash",
            Kind::Delete => "Deleting",
        }
    }
}

/// The job in progress and its dialog.
pub(super) struct RunningJob {
    job: Job,
    kind: Kind,
    /// The panel the sources came from.
    source: Side,
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
        self.end_search(cx);
        let (sources, dir, other_dir) = {
            let commander = self.commander.read(cx);
            let panel = commander.panel(commander.active());
            let other = commander.panel(commander.active().other());
            (
                source_paths(panel),
                panel.path().to_path_buf(),
                other.path().display().to_string(),
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
        let end = other_dir.len();
        self.prompt_name(
            Prompt {
                title: &title,
                error_title,
                initial: &other_dir,
                selection: end..end,
            },
            Rc::new(move |this, typed, window, cx| {
                let to = destination(&dir, typed)?;
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

    /// F8 and Del: confirm, then move to the trash. Shift-F8 and Shift-Del
    /// (`Kind::Delete`): confirm, then delete permanently.
    pub(super) fn trash(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
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
        let source = self.commander.read(cx).active();
        self.job = Some(RunningJob {
            job,
            kind,
            source,
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
                Event::Finished(report) => {
                    self.finish_job(report, window, cx);
                    return false;
                }
            }
        }
        if !running.progress_open && (conflict.is_some() || running.polls >= POLLS_BEFORE_PROGRESS)
        {
            running.progress_open = true;
            let (kind, view) = (running.kind, running.view.clone());
            self.open_progress(kind, view, window, cx);
        }
        if let Some(conflict) = conflict {
            self.ask_conflict(&conflict, window, cx);
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
        let new = SharedString::from(format!("New:       {}", describe_file(&conflict.source)));
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
                        .child(new.clone())
                        .child(old.clone()),
                )
                .footer(buttons.clone())
                .on_cancel(move |_, _, cx| {
                    answer_job(&this_cancel, Answer::Cancel, cx);
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
        if running.kind == Kind::Copy && complete {
            self.commander
                .update(cx, |commander, _| commander.clear_selection(running.source));
        }
        self.execute(Command::Reload, cx);
        if !report.failures.is_empty() {
            let title = match running.kind {
                Kind::Copy => "Some entries were not copied",
                Kind::Move => "Some entries were not moved",
                Kind::Trash => "Some entries were not moved to the trash",
                Kind::Delete => "Some entries were not deleted",
            };
            show_error(title, failure_summary(&report), None, window, cx);
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

/// The directory typed into the F5/F6 prompt, relative to the active panel's
/// directory `dir`. It must exist and differ from `dir`.
fn destination(dir: &Path, typed: &str) -> io::Result<PathBuf> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "give a destination directory",
        ));
    }
    let to = dir.join(typed);
    if !to.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("“{typed}” is not a directory"),
        ));
    }
    if to.canonicalize().ok() == dir.canonicalize().ok() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and destination are the same directory",
        ));
    }
    Ok(to)
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
    fn destination_must_be_another_existing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("file"), b"").unwrap();
        let dir = tmp.path();
        assert_eq!(destination(dir, " sub ").unwrap(), dir.join("sub"));
        let absolute = dir.join("sub").display().to_string();
        assert_eq!(destination(dir, &absolute).unwrap(), dir.join("sub"));
        assert!(destination(dir, "").is_err());
        assert!(destination(dir, "file").is_err());
        assert!(destination(dir, "missing").is_err());
        assert!(destination(dir, ".").is_err());
        assert!(destination(&dir.join("sub"), "..").is_ok());
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
}
