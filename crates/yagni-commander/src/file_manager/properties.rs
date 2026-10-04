//! Alt-Enter: the Properties box. The details are read, and a folder is
//! counted, on a thread of its own (a `stat` can hang on a dead mount);
//! a timer moves the results in, like the opener's errors.

use crate::file_manager::commands::themed_dialog;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::component::WindowExt;
use gpui_kit::{AppContext, Context, Entity, Focusable, ParentElement, Window};
use yagni_commander_core::archive::inner_parts;
use yagni_commander_core::info::{self, Count, Info, Kind, Progress, Totals};
use yagni_commander_core::{EntryKind, Panel};

use super::FileManager;
use super::commands::{OPENER_POLL, focus_when_open};
use crate::button_row::{ButtonRow, OnPress};
use crate::info_dialog::{InfoView, copy_text};

/// Width of the Properties box.
const WIDTH: f32 = 520.0;

/// What the worker thread sends.
enum Update {
    Read(io::Result<Info>),
    /// The count's result; `None` when cancelled.
    Counted(Option<Totals>),
}

/// The open Properties box.
pub(crate) struct RunningInfo {
    /// Tells a late timer tick that its box was closed or replaced.
    id: u64,
    name: String,
    folder: String,
    info: Option<io::Result<Info>>,
    count: Count,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    /// `None` inside an archive: everything is known at once.
    updates: Option<Receiver<Update>>,
    view: Entity<InfoView>,
    buttons: Entity<ButtonRow>,
}

impl RunningInfo {
    /// The details arrived and no count is running.
    pub(crate) fn done(&self) -> bool {
        self.info.is_some() && !matches!(self.count, Count::Counting(_))
    }

    #[cfg(test)]
    pub(crate) fn cancel(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn lines(&self) -> Vec<(&'static str, String)> {
        match &self.info {
            Some(info) => info::lines(&self.name, &self.folder, info, &self.count),
            None => vec![
                ("Name", self.name.clone()),
                ("Folder", self.folder.clone()),
                ("Type", "Reading...".to_owned()),
            ],
        }
    }
}

/// What Alt-Enter describes.
enum Subject {
    /// On disk: read on a thread.
    Disk(PathBuf),
    /// Inside an archive: from the index.
    Archive(Info, Count),
}

impl FileManager {
    /// Alt-Enter: the Properties box for the entry under the cursor, or
    /// the panel's own folder on "..".
    pub(super) fn show_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        let Some((name, folder, subject)) = subject(self.active_panel(cx)) else {
            return;
        };
        self.close_info();
        let (info, count, updates) = match subject {
            Subject::Archive(info, count) => (Some(Ok(info)), count, None),
            Subject::Disk(path) => (None, Count::NotAFolder, Some(path)),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let updates = updates.map(|path| read_on_thread(path, &progress, &cancel));
        self.next_info += 1;
        let id = self.next_info;
        let this = cx.entity().downgrade();
        let ok: OnPress = Rc::new(move |window, cx| {
            window.close_dialog(cx);
            let _ = this.update(cx, |this, cx| this.closed_info(id, window, cx));
        });
        let buttons = ButtonRow::build([("OK", ok)], 0, cx);
        let view = cx.new(|_| InfoView::new());
        let running = RunningInfo {
            id,
            name,
            folder,
            info,
            count,
            cancel,
            progress,
            updates,
            view: view.clone(),
            buttons: buttons.clone(),
        };
        refresh(&running, cx);
        let waiting = !running.done();
        self.info = Some(running);
        focus_when_open(buttons.focus_handle(cx), window, cx);
        let this = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            themed_dialog(dialog, cx)
                .title("Properties")
                .w(crate::zoom::dialog_width(WIDTH, cx))
                .close_button(false)
                .child(view.clone())
                .footer(buttons.clone())
                .on_cancel(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.closed_info(id, window, cx));
                    true
                })
        });
        if waiting {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(OPENER_POLL).await;
                    let open = this.update(cx, |this, cx| this.poll_info(id, cx));
                    if !matches!(open, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Takes in what the thread sent. Returns whether box `id` still waits.
    fn poll_info(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(running) = self.info.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        if let Some(updates) = &running.updates {
            loop {
                match updates.try_recv() {
                    Ok(Update::Read(read)) => {
                        let folder = matches!(&read, Ok(i) if i.kind == Kind::Folder);
                        if folder {
                            running.count = Count::Counting(Totals::default());
                        }
                        running.info = Some(read);
                    }
                    Ok(Update::Counted(Some(totals))) => running.count = Count::Done(totals),
                    Ok(Update::Counted(None)) | Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        if let Count::Counting(_) = running.count {
            running.count = Count::Counting(running.progress.so_far());
        }
        refresh(running, cx);
        !running.done()
    }

    /// Box `id` closed (OK, Escape): stop its count and forget it.
    fn closed_info(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.info.as_ref().is_some_and(|r| r.id == id) {
            self.close_info();
        }
        // After the dialog stack has finished restoring focus.
        let focus = self.focus.clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }

    fn close_info(&mut self) {
        if let Some(running) = self.info.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Shows `running`'s current lines and makes Ctrl-C copy them.
fn refresh(running: &RunningInfo, cx: &mut gpui_kit::App) {
    let lines = running.lines();
    let text = copy_text(&lines);
    let changed = running.view.update(cx, |view, cx| {
        let changed = view.set_lines(lines);
        if changed {
            cx.notify();
        }
        changed
    });
    if changed {
        running.buttons.update(cx, |row, _| row.set_copy_text(text));
    }
}

/// Reads `path` (and counts it, if it is a folder) on a thread of its own.
fn read_on_thread(
    path: PathBuf,
    progress: &Arc<Progress>,
    cancel: &Arc<AtomicBool>,
) -> Receiver<Update> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (progress, cancel) = (progress.clone(), cancel.clone());
    std::thread::spawn(move || {
        let read = info::read(&path);
        let folder = matches!(&read, Ok(i) if i.kind == Kind::Folder);
        if tx.send(Update::Read(read)).is_err() || !folder {
            return;
        }
        let _ = tx.send(Update::Counted(info::count(&path, &progress, &cancel)));
    });
    rx
}

/// The name, folder and source of what Alt-Enter describes in `panel`.
fn subject(panel: &Panel) -> Option<(String, String, Subject)> {
    let entry = panel.cursor_entry()?;
    let path = if entry.kind == EntryKind::Parent {
        panel.path().to_path_buf()
    } else {
        panel.path().join(&entry.name)
    };
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let folder = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let Some(index) = panel.archive() else {
        return Some((name, folder, Subject::Disk(path)));
    };
    let inner = inner_parts(index.file(), &path);
    let subject = match index.entry(&inner) {
        Some(entry) if entry.kind != EntryKind::Dir => {
            Subject::Archive(info::from_entry(&entry), Count::NotAFolder)
        }
        // A folder inside, or the archive's root (no entry of its own).
        found => {
            let info = match found {
                Some(entry) => info::from_entry(&entry),
                None => folder_info(),
            };
            Subject::Archive(info, Count::Done(index.totals(&inner)))
        }
    };
    Some((name, folder, subject))
}

/// An archive's root: a folder with nothing else known.
fn folder_info() -> Info {
    Info {
        kind: Kind::Folder,
        size: None,
        modified: None,
        accessed: None,
        created: None,
        mode: None,
        owner: None,
        link: None,
    }
}
