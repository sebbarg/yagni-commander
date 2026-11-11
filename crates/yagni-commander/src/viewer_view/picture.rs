//! Image mode (`I`, and at open for an image's name): core `read_picture` on
//! a `std::thread` of its own, reported over a std channel polled every
//! `OPENER_POLL` (a futures wake-up from another thread panics gpui's test
//! scheduler). We decode, not gpui's image loader: that one takes every format
//! the crate has, sets no limit, and a picture too big for a GPU texture
//! draws nothing. The mode lies over the others, which stay below it for the
//! way back when a picture can't be read.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::{
    App, Context, ImageSource, ObjectFit, RenderImage, StyledImage, Window, div, img, prelude::*,
};
use yagni_commander_core::viewer::{PICTURE_SIDE, Picture, read_picture};

use super::{PADDING, ViewerView};
use crate::file_manager::commands::{OPENER_POLL, show_error, show_message};
use crate::zoom::rems_from_px;

/// Never decode smaller than this for a display we can't size properly.
const MIN_SIDE: u32 = 1024;

struct Shown {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    format: &'static str,
}

struct Loading {
    id: u64,
    rx: Receiver<Result<Picture, String>>,
}

#[derive(Default)]
pub(super) struct PictureMode {
    /// `None` until the first read lands. A reload keeps it on screen until
    /// the new version is ready.
    shown: Option<Shown>,
    loading: Option<Loading>,
}

impl PictureMode {
    /// The status line's part after the file's size.
    pub(super) fn status(&self) -> String {
        match &self.shown {
            Some(s) => format!("image · {}x{} · {}", s.width, s.height, s.format),
            None => "image · loading...".to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    #[cfg(test)]
    pub(super) fn image(&self) -> Option<&Arc<RenderImage>> {
        self.shown.as_ref().map(|s| &s.image)
    }
}

/// How long a decoded picture's longest side may be: as long as the display,
/// since the GPU only filters well down to about half. Without a display
/// size, the texture limit.
fn side_limit(display: Option<f32>) -> u32 {
    display.map_or(PICTURE_SIDE, |longest| {
        (longest.ceil() as u32).clamp(MIN_SIDE, PICTURE_SIDE)
    })
}

fn display_side(window: &Window, cx: &App) -> Option<f32> {
    let size = window.display(cx)?.bounds().size;
    Some(f32::from(size.width).max(f32::from(size.height)) * window.scale_factor())
}

impl ViewerView {
    /// `I`: image mode, over the mode shown now; the key of the mode shown
    /// does nothing.
    pub(super) fn show_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picture.is_none() {
            self.picture = Some(PictureMode::default());
            self.read_picture(window, cx);
        }
    }

    /// Back to the mode below, freeing the picture's texture.
    pub(super) fn leave_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(shown) = self.picture.take().and_then(|mode| mode.shown) {
            cx.drop_image(shown.image, Some(window));
        }
        cx.notify();
    }

    /// Reads the open file's picture on a thread, replacing a read under way.
    pub(super) fn read_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = match self.doc.source().try_clone() {
            Ok(source) => source,
            Err(e) => {
                self.leave_picture(window, cx);
                let back = Some(self.focus.clone());
                show_error("Image", e.to_string(), back, window, cx);
                return;
            }
        };
        let max_side = side_limit(display_side(window, cx));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_picture(&source, max_side));
        });
        self.next_picture += 1;
        let id = self.next_picture;
        if let Some(mode) = &mut self.picture {
            mode.loading = Some(Loading { id, rx });
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(OPENER_POLL).await;
                let going = this
                    .update_in(cx, |this, window, cx| this.poll_picture(id, window, cx))
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Takes in read `id`'s picture. Returns whether it is still running.
    fn poll_picture(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(mode) = self.picture.as_mut() else {
            return false;
        };
        let Some(loading) = mode.loading.as_ref().filter(|l| l.id == id) else {
            return false;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return true,
            // Its thread died: a decoder's panic on a bad file.
            Err(TryRecvError::Disconnected) => Err("The image could not be decoded.".to_owned()),
        };
        mode.loading = None;
        match result {
            Ok(picture) => {
                let new = Shown {
                    image: Arc::new(RenderImage::new([picture.frame])),
                    width: picture.width,
                    height: picture.height,
                    format: picture.format,
                };
                if let Some(old) = mode.shown.replace(new) {
                    cx.drop_image(old.image, Some(window));
                }
                cx.notify();
            }
            Err(message) => {
                self.leave_picture(window, cx);
                let back = Some(self.focus.clone());
                show_message("Image", message, "OK", back, window, cx);
            }
        }
        false
    }

    /// The picture, centered, scaled down to fit and never up.
    pub(super) fn render_picture(&self) -> impl IntoElement {
        let image = self
            .picture
            .as_ref()
            .and_then(|mode| mode.shown.as_ref())
            .map(|shown| shown.image.clone());
        // Positioned boxes, because they have a definite size: `Img` sets its
        // own aspect ratio, and a percentage of a flex-sized parent doesn't
        // resolve, so its height would follow its width past the window.
        let inset = rems_from_px(PADDING);
        div()
            .debug_selector(|| "viewer-picture".into())
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .absolute()
                    .top(inset)
                    .left(inset)
                    .right(inset)
                    .bottom(inset)
                    .when_some(image, |this, image| {
                        this.child(
                            img(ImageSource::Render(image))
                                .debug_selector(|| "viewer-picture-image".into())
                                .absolute()
                                .size_full()
                                .object_fit(ObjectFit::ScaleDown),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_decoded_for_the_display() {
        assert_eq!(side_limit(Some(1920.0)), 1920);
        assert_eq!(side_limit(Some(2559.5)), 2560);
        assert_eq!(side_limit(Some(7680.0)), PICTURE_SIDE, "the texture limit");
        assert_eq!(side_limit(Some(640.0)), MIN_SIDE);
        assert_eq!(side_limit(None), PICTURE_SIDE);
    }
}
