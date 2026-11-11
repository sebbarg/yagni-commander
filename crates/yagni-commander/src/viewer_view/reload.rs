//! A viewer notices when its file changes on disk and lights the reload
//! button at the right of the status line; the text on screen stays until
//! the user asks (Ctrl-R or the button). One `PanelWatcher` per window, on
//! the file's folder: an editor's save usually renames a new file over the
//! old one, which a watch on the file itself would not survive. A folder
//! report only says something changed there, so its thread compares the
//! file's stamp, where a stat on a dead mount can hang only that thread.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use gpui_kit::{Context, Window, div, prelude::*};
use yagni_commander_core::icons;
use yagni_commander_core::viewer::{Document, FileSource, FileStamp, count_lines};
use yagni_commander_core::watch::PanelWatcher;

use super::ViewerView;
use crate::file_manager::commands::{show_error, show_message};
use crate::theme::Theme;
use crate::zoom::rems_from_px;

/// The status line's icon buttons (reload, `?`): their glyphs fill the em
/// box, so the status line's own size would look massive.
pub(super) const ICON_TEXT: f32 = 10.0;

/// Counts the lines of `source` in the background into `lines`; returns the
/// flag that stops it (and drops a result already on its way).
pub(super) fn start_count(source: &FileSource, cx: &mut Context<ViewerView>) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(counter) = source.try_clone() {
        let stop = cancel.clone();
        let counting = cx
            .background_executor()
            .spawn(async move { count_lines(&counter, &stop) });
        let stop = cancel.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Some(index)) = counting.await {
                let _ = this.update(cx, |this, cx| {
                    if !stop.load(Ordering::Relaxed) {
                        this.lines = Some(Arc::new(index));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    cancel
}

/// Starts watching `path`'s real folder (a symlink's target changes there).
/// None in app tests, where a report from another thread would panic gpui's
/// test scheduler: they call `file_seen`.
pub(super) fn watch_file(path: &Path, cx: &mut Context<ViewerView>) -> Option<PanelWatcher> {
    if cfg!(test) {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    let dir = real.parent()?.to_path_buf();
    let (seen, mut received) = futures::channel::mpsc::unbounded();
    let path = path.to_path_buf();
    // `Started` counts too: the file may have changed before the watch began.
    let mut watcher = PanelWatcher::spawn(move |_| {
        let _ = seen.unbounded_send(FileStamp::of(&path));
    });
    watcher.watch(dir);
    cx.spawn(async move |this, cx| {
        while let Some(stamp) = received.next().await {
            if this
                .update(cx, |this, cx| this.file_seen(stamp, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    Some(watcher)
}

impl ViewerView {
    /// The file on disk is at `seen` now. A file that is gone has nothing
    /// to reload, so it lights nothing.
    pub(super) fn file_seen(&mut self, seen: Option<FileStamp>, cx: &mut Context<Self>) {
        let changed = seen.is_some_and(|seen| seen != self.stamp);
        if changed != self.changed {
            self.changed = changed;
            cx.notify();
        }
    }

    /// Ctrl-R or the button: reads the file again, keeping the mode and the
    /// place in the file. Byte positions of the old version mean nothing in
    /// the new one, so the selection and the current match go; the search
    /// text stays for F3. A private copy has nothing newer to read.
    pub(super) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.temp.is_some() {
            return;
        }
        let back = Some(self.focus.clone());
        let source = match FileSource::open(&self.path) {
            Ok(source) => source,
            Err(e) => {
                show_error("Cannot reload", e.to_string(), back, window, cx);
                return;
            }
        };
        self.cancel_count.store(true, Ordering::Relaxed);
        self.cancel_count = start_count(&source, cx);
        self.stamp = source.stamp();
        self.changed = false;
        self.doc = Document::new(source);
        self.lines = None;
        self.top_line = None;
        self.selection = None;
        self.selecting = false;
        self.search.running = None;
        self.search.current = None;
        let layout = self.layout();
        let top = self.doc.row_start_at(self.top.min(self.doc.len()), layout);
        self.top = top.min(self.doc.last_top(self.screen_rows, layout));
        if let Some(old) = self.markdown.take() {
            let top = old.top(cx);
            match self.try_markdown(cx) {
                Ok(()) => {
                    if let Some(markdown) = &self.markdown {
                        markdown.restore_top(top, cx);
                    }
                }
                // It grew past the limits: the text view shows it.
                Err(message) => show_message("Markdown", message, "OK", back, window, cx),
            }
        }
        if self.picture.is_some() {
            self.read_picture(window, cx);
        }
        cx.notify();
    }

    /// The reload button beside `?`: quiet, until the file has changed.
    pub(super) fn reload_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &Theme::get(cx).colors;
        let (bg, fg) = if self.changed {
            (colors.accent, colors.text_on_accent)
        } else {
            (colors.header_bg, colors.text_secondary)
        };
        div()
            .id("viewer-reload-button")
            .debug_selector(|| "viewer-reload-button".into())
            .flex_none()
            .ml(rems_from_px(8.0))
            .px(rems_from_px(6.0))
            .h(rems_from_px(16.0))
            .flex()
            .items_center()
            .rounded(rems_from_px(3.0))
            .border_1()
            .border_color(colors.border)
            .bg(bg)
            .text_color(fg)
            .cursor_pointer()
            .font_family(icons::FONT_FAMILY)
            .text_size(rems_from_px(ICON_TEXT))
            .child(icons::RELOAD.to_string())
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Stopped here, the click can't reach the view's own
                    // focus handling. First, so a reload's message box
                    // still takes focus from it.
                    cx.stop_propagation();
                    this.focus.focus(window, cx);
                    this.reload(window, cx);
                }),
            )
    }
}
