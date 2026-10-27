//! F4 and Shift-F4 on a remote folder, and F2/F7 there: the file is
//! fetched into a private folder (a `Kind::Edit` job), the editor opens the
//! copy, and every save is uploaded (core `file_ops::upload_edit`) while
//! the remote file is still the one fetched. A remote file changed in the
//! meantime asks first. The copies are watched for the rest of the session.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use gpui_kit::component::WindowExt;
use gpui_kit::{Context, Focusable, InteractiveElement, ParentElement, Window, div};
use yagni_commander_core::file_ops::{
    Destination, Operation, RemoteStamp, Report, Uploaded, upload_edit,
};
use yagni_commander_core::launch;

use super::FileManager;
use super::commands::{configured_editor, focus_when_open, show_error, themed_dialog};
use super::file_ops::{Kind, RunningJob};
use crate::button_row::{ButtonRow, OnPress};

/// How often the edited copies are checked for a save.
pub(crate) const EDIT_POLL: Duration = Duration::from_secs(1);

/// `time` as SFTP seconds, if it fits.
pub(crate) fn seconds(time: SystemTime) -> Option<u32> {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
    u32::try_from(secs).ok()
}

/// A remote file being fetched for the editor.
pub(crate) struct Fetching {
    dir: tempfile::TempDir,
    file: PathBuf,
    remote: PathBuf,
    stamp: Option<RemoteStamp>,
    editor: String,
}

/// A fetched copy open in the editor.
pub(crate) struct Edited {
    pub(crate) local: PathBuf,
    pub(crate) remote: PathBuf,
    /// The remote file as last fetched or uploaded; `None` uploads over
    /// whatever is there.
    stamp: Option<RemoteStamp>,
    /// The copy's time and size when last uploaded (or fetched).
    seen: Option<(SystemTime, u64)>,
    uploading: Option<Receiver<io::Result<Uploaded>>>,
    /// Holds the copy's folder; dropped with it.
    _dir: tempfile::TempDir,
}

/// The copy's time and size now.
fn state(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl FileManager {
    /// Starts an F2/F7/Shift-F4 job on a remote folder; once it is done the
    /// panel is re-read with the cursor on `select`.
    pub(super) fn start_remote_job(
        &mut self,
        kind: Kind,
        operation: Operation,
        select: OsString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        if self.refuse_second_job(window, cx) {
            return Ok(());
        }
        self.start_job(kind, operation, window, cx)?;
        if let Some(running) = &mut self.job {
            running.select = Some(select);
        }
        Ok(())
    }

    /// F4 on remote file `remote` (`stamp`: the remote file as listed).
    pub(super) fn edit_remote(
        &mut self,
        remote: PathBuf,
        stamp: Option<RemoteStamp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = configured_editor(Some(self.focus.clone()), window, cx) else {
            return;
        };
        if self.refuse_second_job(window, cx) {
            return;
        }
        let started = (|| {
            let root = self
                .temp_dir
                .clone()
                .ok_or_else(|| io::Error::other("no folder for temporary files"))?;
            yagni_commander_core::storage::private_dir(&root)?;
            let dir = tempfile::Builder::new().prefix("edit-").tempdir_in(&root)?;
            let name = yagni_commander_core::remote::file_name(&remote)
                .ok_or_else(|| io::Error::other("not a file"))?;
            let file = dir.path().join(name);
            let operation = Operation::Copy {
                sources: vec![remote.clone()],
                to: Destination::As(file.clone()),
            };
            self.start_job(Kind::Edit, operation, window, cx)?;
            if let Some(running) = &mut self.job {
                running.editing = Some(Fetching {
                    dir,
                    file,
                    remote,
                    stamp,
                    editor,
                });
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(e) = started {
            show_error(
                "Cannot edit file",
                e.to_string(),
                Some(self.focus.clone()),
                window,
                cx,
            );
        }
    }

    /// The `Kind::Edit` job ended: open the editor on the copy and watch it.
    pub(super) fn fetched_for_edit(
        &mut self,
        running: RunningJob,
        report: Report,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(fetching) = running.editing else {
            return;
        };
        if !report.failures.is_empty() {
            let text = report
                .failures
                .iter()
                .map(|f| format!("{}: {}", f.path.display(), f.message))
                .collect::<Vec<_>>()
                .join("\n");
            show_error(
                "Cannot edit file",
                text,
                Some(self.focus.clone()),
                window,
                cx,
            );
            return;
        }
        if report.cancelled {
            return;
        }
        if let Err(e) = launch::open_in_editor(&fetching.editor, &fetching.file) {
            show_error(
                "Cannot open editor",
                e.to_string(),
                Some(self.focus.clone()),
                window,
                cx,
            );
            return;
        }
        // A file edited again replaces its earlier copy.
        self.edited.retain(|e| e.remote != fetching.remote);
        self.edited.push(Edited {
            seen: state(&fetching.file),
            local: fetching.file,
            remote: fetching.remote,
            stamp: fetching.stamp,
            uploading: None,
            _dir: fetching.dir,
        });
        if !self.edit_watch {
            self.edit_watch = true;
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(EDIT_POLL).await;
                    let watching = this.update_in(cx, |this, window, cx| {
                        this.check_edits(window, cx);
                        !this.edited.is_empty()
                    });
                    if !matches!(watching, Ok(true)) {
                        break;
                    }
                }
                let _ = this.update(cx, |this, _| this.edit_watch = false);
            })
            .detach();
        }
    }

    /// Uploads each copy saved since its last upload, and takes in the
    /// uploads that ended.
    fn check_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.edited.len() {
            let edited = &mut self.edited[ix];
            if let Some(result) = &edited.uploading {
                let result = match result.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Empty) => continue,
                    Err(TryRecvError::Disconnected) => Err(io::Error::other("the upload stopped")),
                };
                edited.uploading = None;
                match result {
                    Ok(Uploaded::Done(stamp)) => edited.stamp = Some(stamp),
                    Ok(Uploaded::Changed) => {
                        let remote = edited.remote.clone();
                        self.ask_overwrite(remote, window, cx);
                    }
                    Err(e) => {
                        let text = format!("{}: {e}", edited.remote.display());
                        show_error("Cannot upload", text, None, window, cx);
                    }
                }
                continue;
            }
            let now = state(&edited.local);
            if now.is_some() && now != edited.seen {
                edited.seen = now;
                let stamp = edited.stamp;
                self.upload(ix, stamp);
            }
        }
    }

    /// Uploads copy `ix` on a thread, if the remote file is still `stamp`.
    fn upload(&mut self, ix: usize, stamp: Option<RemoteStamp>) {
        let edited = &mut self.edited[ix];
        let (local, remote) = (edited.local.clone(), edited.remote.clone());
        let (send, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(upload_edit(&local, &remote, stamp));
        });
        edited.uploading = Some(result);
    }

    /// The remote file changed since it was fetched: upload over it, or
    /// leave it (the next save asks again).
    fn ask_overwrite(&mut self, remote: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = yagni_commander_core::remote::file_name(&remote)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let text = format!(
            "“{name}” changed on the host since it was opened in the editor. Upload your version over it?"
        );
        let this = cx.entity().downgrade();
        let focus = self.focus.clone();
        let upload: OnPress = Rc::new({
            let (this, focus) = (this.clone(), focus.clone());
            move |window, cx| {
                let _ = this.update(cx, |fm, _| {
                    if let Some(ix) = fm.edited.iter().position(|e| e.remote == remote) {
                        fm.upload(ix, None);
                    }
                });
                window.close_dialog(cx);
                focus.focus(window, cx);
            }
        });
        let keep: OnPress = Rc::new(move |window, cx| {
            window.close_dialog(cx);
            focus.focus(window, cx);
        });
        let buttons = ButtonRow::build([("Upload", upload), ("Don't upload", keep)], 1, cx);
        focus_when_open(buttons.focus_handle(cx), window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            themed_dialog(dialog, cx)
                .title("Remote file changed")
                .w(crate::zoom::dialog_width(520.0, cx))
                .close_button(false)
                .child(
                    div()
                        .debug_selector(|| "remote-changed".into())
                        .child(text.clone()),
                )
                .footer(buttons.clone())
        });
    }
}
