//! Ctrl-C, Ctrl-X and Ctrl-V on files: the files are kept here, in the
//! app, and pasted as an F5 or F6 job into the active panel's folder,
//! without F5's prompt. Ctrl-C and Ctrl-X also put the paths on the system
//! clipboard as text, one per line; that text is the marker: Ctrl-V pastes
//! only while the clipboard still holds it, so copying anything else
//! anywhere forgets the files. Text on the clipboard is never read as
//! paths. Files another app put there as files (Finder; gpui reads them on
//! macOS only) are copied. On macOS local files also go onto the pasteboard
//! as files (core `pasteboard`), so Finder pastes them; the marker is then
//! the pasteboard's change count, never the items read back (see
//! `pasteboard::change_count`). Once ours are forgotten they are put back
//! as text alone, so nothing pastes them.

use std::path::{Path, PathBuf};

use gpui_kit::{ClipboardEntry, ClipboardItem, Context, SharedString, Window};
use yagni_commander_core::PanelId;
use yagni_commander_core::file_ops::Destination;
use yagni_commander_core::remote;

use super::FileManager;
use super::commands::show_error;
use super::file_ops::{FromArchive, Kind, copy_operation, describe, from_archive, source_paths};

/// The files Ctrl-C or Ctrl-X took.
pub(crate) struct FileClipboard {
    /// Counts the clipboards of the session, so a paste's end forgets only
    /// the cut it pasted.
    id: u64,
    cut: bool,
    sources: Vec<PathBuf>,
    /// Inside an archive: the sources as its entries.
    from_archive: Option<FromArchive>,
    /// What went onto the system clipboard.
    text: String,
    /// macOS: the files went onto the pasteboard as files too, which left
    /// it at this change count.
    as_files: Option<isize>,
    /// The tab and folder they came from: a complete copy clears the
    /// selection there, as F5 does.
    source: PanelId,
    source_dir: PathBuf,
}

/// The status line while files are on the clipboard.
fn status_text(cut: bool, sources: &[PathBuf]) -> String {
    let paste = if cfg!(target_os = "macos") {
        "Cmd-V"
    } else {
        "Ctrl-V"
    };
    let (done, verb) = if cut {
        ("Cut", "moves")
    } else {
        ("Copied", "copies")
    };
    let them = if sources.len() == 1 { "it" } else { "them" };
    format!(
        "{done} {} · {paste} {verb} {them} here · Escape forgets",
        describe(sources)
    )
}

/// The paths as the clipboard's text, one per line.
fn clipboard_text(sources: &[PathBuf]) -> String {
    sources
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Puts the paths on the clipboard as `text`; on macOS, when they are
/// `local` files, as files too. Returns the pasteboard's change count when
/// they went on as files.
fn write_clipboard(
    sources: &[PathBuf],
    local: bool,
    text: &str,
    cx: &mut gpui_kit::App,
) -> Option<isize> {
    // Never in tests: they would overwrite the real pasteboard, which the
    // test clipboard doesn't see.
    #[cfg(all(target_os = "macos", not(test)))]
    if local {
        let paths: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
        if let Some(count) = yagni_commander_core::pasteboard::write_files(&paths, text) {
            return Some(count);
        }
    }
    #[cfg(any(not(target_os = "macos"), test))]
    let _ = (sources, local);
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
    None
}

/// Whether the system clipboard still holds `clip`'s files: the change
/// count when they went onto the pasteboard as files, else the text.
fn still_holds(clip: &FileClipboard, cx: &gpui_kit::App) -> bool {
    #[cfg(all(target_os = "macos", not(test)))]
    if let Some(count) = clip.as_files {
        return yagni_commander_core::pasteboard::change_count() == count;
    }
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .as_deref()
        == Some(clip.text.as_str())
}

/// The files another app put on the clipboard as files.
fn external_paths(item: &ClipboardItem) -> Option<Vec<PathBuf>> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::ExternalPaths(paths) if !paths.paths().is_empty() => {
            Some(paths.paths().to_vec())
        }
        _ => None,
    })
}

/// Whether `a` and `b` are the same folder: canonical paths locally, the
/// paths themselves on a host (nothing there is asked without waiting).
fn same_dir(a: &Path, b: &Path) -> bool {
    if remote::is_remote(a) || remote::is_remote(b) {
        return a == b;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl FileManager {
    /// Ctrl-C (`cut` false) and Ctrl-X: take the targets (selection, else
    /// the cursor entry) and put their paths on the clipboard. A cut inside
    /// an archive is refused, like F6.
    pub(super) fn take_files(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) || (cut && self.refuse_in_archive(window, cx)) {
            return;
        }
        self.end_search(cx);
        let panel = self.active_panel(cx);
        let sources = source_paths(panel);
        if sources.is_empty() {
            return;
        }
        let text = clipboard_text(&sources);
        // Archive entries and remote files are no files Finder could take.
        let local = !panel.in_archive() && !panel.is_remote();
        let as_files = write_clipboard(&sources, local, &text, cx);
        let panel = self.active_panel(cx);
        self.next_clipboard += 1;
        self.clipboard = Some(FileClipboard {
            id: self.next_clipboard,
            cut,
            from_archive: from_archive(panel),
            source: panel.id(),
            source_dir: panel.path().to_path_buf(),
            sources,
            text,
            as_files,
        });
        self.notice = None;
        cx.notify();
    }

    /// Escape (last) and the end of a pasted cut: forget the files. The
    /// text stays on the system clipboard. Returns whether there were any.
    pub(super) fn forget_files(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(clip) = self.clipboard.take() else {
            return false;
        };
        // Still ours on the pasteboard as files: text alone, so neither
        // Finder nor our Ctrl-V pastes them.
        if clip.as_files.is_some() && still_holds(&clip, cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(clip.text));
        }
        cx.notify();
        true
    }

    /// A complete paste of cut `id` forgets it (a newer Ctrl-C or Ctrl-X
    /// stays).
    pub(super) fn forget_cut(&mut self, id: Option<u64>, cx: &mut Context<Self>) {
        if self.clipboard.as_ref().map(|clip| clip.id) == id {
            self.forget_files(cx);
        }
    }

    /// Forgets the files once the system clipboard no longer holds their
    /// text: something else was copied (window activation, Ctrl-V).
    pub(super) fn check_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clip) = &self.clipboard else {
            return;
        };
        if !still_holds(clip, cx) {
            self.clipboard = None;
            cx.notify();
        }
    }

    /// The status line while files are on the clipboard.
    pub(super) fn clipboard_status(&self) -> Option<SharedString> {
        let clip = self.clipboard.as_ref()?;
        Some(status_text(clip.cut, &clip.sources).into())
    }

    /// Ctrl-V: copy or move the files into the active panel's folder, as
    /// F5 and F6 would. A copy of one entry into its own folder asks for a
    /// name, like Shift-F5.
    pub(super) fn paste_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_loading(cx) {
            return;
        }
        self.end_search(cx);
        self.check_clipboard(cx);
        let external = match &self.clipboard {
            Some(_) => None,
            None => cx.read_from_clipboard().as_ref().and_then(external_paths),
        };
        if self.clipboard.is_none() && external.is_none() {
            self.notice = Some("No files on the clipboard".into());
            cx.notify();
            return;
        }
        if self.refuse_read_only_target(window, cx) {
            return;
        }
        let dir = self.active_panel(cx).path().to_path_buf();
        let (kind, sources, archive, origin, cut) = match (&self.clipboard, external) {
            (Some(clip), _) => (
                if clip.cut { Kind::Move } else { Kind::Copy },
                clip.sources.clone(),
                clip.from_archive.clone(),
                Some((clip.source, clip.source_dir.clone())),
                clip.cut.then_some(clip.id),
            ),
            (None, Some(paths)) => (Kind::Copy, paths, None, None, None),
            (None, None) => return,
        };
        // Inside an archive the sources' folder is the archive's, never a
        // real one, so it can't be the panel's.
        let here = archive.is_none()
            && sources
                .iter()
                .any(|source| source.parent().is_some_and(|p| same_dir(p, &dir)));
        if here {
            match (kind, sources.as_slice()) {
                (Kind::Copy, [one]) => {
                    let is_dir = !remote::is_remote(one) && one.is_dir();
                    self.copy_beside(one.clone(), is_dir, window, cx);
                }
                _ => show_error(
                    "Cannot paste",
                    "The entries are already in this folder.",
                    Some(self.focus.clone()),
                    window,
                    cx,
                ),
            }
            return;
        }
        if self.refuse_second_job(window, cx) {
            return;
        }
        let started = copy_operation(archive.as_ref(), kind, sources, Destination::Into(dir))
            .and_then(|operation| self.start_job(kind, operation, window, cx));
        if let Err(e) = started {
            let title = match kind {
                Kind::Move => "Cannot move",
                _ => "Cannot copy",
            };
            show_error(title, e.to_string(), Some(self.focus.clone()), window, cx);
            return;
        }
        if let Some(running) = &mut self.job {
            if let Some((source, source_dir)) = origin {
                running.source = source;
                running.source_dir = source_dir;
            }
            running.pasted_cut = cut;
        }
    }

    /// Nothing is pasted into an archive or search results.
    fn refuse_read_only_target(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.refuse_in_archive(window, cx) || self.refuse_in_results(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_says_what_ctrl_v_will_do() {
        let one = [PathBuf::from("/a/notes.txt")];
        let two = [PathBuf::from("/a/x"), PathBuf::from("/a/y")];
        let paste = if cfg!(target_os = "macos") {
            "Cmd-V"
        } else {
            "Ctrl-V"
        };
        assert_eq!(
            status_text(false, &one),
            format!("Copied “notes.txt” · {paste} copies it here · Escape forgets")
        );
        assert_eq!(
            status_text(true, &two),
            format!("Cut 2 entries · {paste} moves them here · Escape forgets")
        );
    }

    #[test]
    fn the_text_is_one_path_per_line() {
        let paths = [PathBuf::from("/a/x"), PathBuf::from("/b/y z")];
        assert_eq!(clipboard_text(&paths), "/a/x\n/b/y z");
    }

    #[test]
    fn files_come_from_a_files_entry_never_from_text() {
        let text = ClipboardItem::new_string("/etc/passwd".into());
        assert_eq!(external_paths(&text), None);
        let files = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(gpui_kit::ExternalPaths(
                vec![PathBuf::from("/a/x")].into(),
            ))],
        };
        assert_eq!(external_paths(&files), Some(vec![PathBuf::from("/a/x")]));
    }

    #[test]
    fn a_folder_is_the_same_through_a_dot_but_hosts_compare_as_written() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(same_dir(tmp.path(), &tmp.path().join(".")));
        assert!(!same_dir(tmp.path(), &tmp.path().join("..")));
        assert!(same_dir(Path::new("sftp://h/a"), Path::new("sftp://h/a")));
        assert!(!same_dir(Path::new("sftp://h/a"), Path::new("sftp://g/a")));
    }
}
