//! Image mode (`I`, and at open for an image's name): core `read_picture` on
//! a `std::thread` of its own, reported over a std channel polled every
//! `OPENER_POLL` (a futures wake-up from another thread panics gpui's test
//! scheduler). We decode, not gpui's image loader: that one takes every format
//! the crate has, sets no limit, and a picture too big for a GPU texture
//! draws nothing. The mode lies over the others, which stay below it for the
//! way back when a picture can't be read.
//!
//! Zoom is the window's own (`Zoomed`), not the viewer's text level: none
//! is fit, drawn by `ObjectFit::ScaleDown`; a scale draws the picture at
//! that size and moves it by its center, kept inside its edges.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use gpui_kit::{
    App, Context, CursorStyle, ImageSource, MouseButton, MouseDownEvent, ObjectFit, Pixels, Point,
    RenderImage, StyledImage, Window, div, img, prelude::*, px,
};
use yagni_commander_core::viewer::{PICTURE_SIDE, Picture, read_picture};

use super::markdown::Scroll;
use super::{PADDING, ViewerView, padding, status_height};
use crate::file_manager::commands::{OPENER_POLL, show_error, show_message};
use crate::zoom::rems_from_px;

/// Never decode smaller than this for a display we can't size properly.
const MIN_SIDE: u32 = 1024;

struct Shown {
    image: Arc<RenderImage>,
    /// The decoded size, in device pixels: smaller than the picture's own
    /// when it was cut to the display.
    decoded: (u32, u32),
    width: u32,
    height: u32,
    format: &'static str,
}

struct Loading {
    id: u64,
    rx: Receiver<Result<Picture, String>>,
}

/// The zoom steps, as the size of one picture pixel in UI points.
const STEPS: [f32; 11] = [0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];

/// An arrow key's move, as a share of the picture's area.
const KEY_STEP: f32 = 0.125;
/// PageUp/PageDown, as a share of the area: a little stays in view.
const PAGE_STEP: f32 = 0.9;

/// A picture zoomed away from fit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Zoomed {
    scale: f32,
    /// The point of the picture at the area's center, as shares of its
    /// width and height.
    center: (f32, f32),
}

#[derive(Default)]
pub(super) struct PictureMode {
    /// `None` until the first read lands. A reload keeps it on screen until
    /// the new version is ready.
    shown: Option<Shown>,
    loading: Option<Loading>,
    /// `None`: fit to the area, never enlarged. Kept through a reload.
    zoom: Option<Zoomed>,
    /// The mouse position a drag last moved the picture to.
    drag: Option<Point<Pixels>>,
    /// Decoded up to `PICTURE_SIDE`, for a zoom past the display's size.
    sharp: bool,
}

impl PictureMode {
    /// The status line's part after the file's size.
    pub(super) fn status(&self) -> String {
        match &self.shown {
            Some(s) => match self.zoom {
                Some(z) => format!(
                    "image · {}x{} · {} · {}%",
                    s.width,
                    s.height,
                    s.format,
                    (z.scale * 100.0).round()
                ),
                None => format!("image · {}x{} · {}", s.width, s.height, s.format),
            },
            None => "image · loading...".to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    pub(super) fn is_zoomed(&self) -> bool {
        self.zoom.is_some()
    }

    #[cfg(test)]
    pub(super) fn is_sharp(&self) -> bool {
        self.sharp
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

/// The picture's area: the viewer less its padding and the status line (the
/// layout is fixed, as for the text's area), in points.
fn area(window: &Window, cx: &App) -> (f32, f32) {
    let v = window.viewport_size();
    let padding = padding(cx);
    (
        (f32::from(v.width) - 2.0 * padding).max(1.0),
        (f32::from(v.height) - 2.0 * padding - status_height(cx)).max(1.0),
    )
}

/// The scale that fits `own` (the picture's size) into `area`, never above 1.
fn fit_scale(own: (u32, u32), area: (f32, f32)) -> f32 {
    let (w, h) = (own.0.max(1) as f32, own.1.max(1) as f32);
    (area.0 / w).min(area.1 / h).min(1.0)
}

/// The zoom after a step in or out from `current` (`None`: fit), `None` past
/// the last step. A step that reaches or crosses `fit` stops at fit.
fn next_zoom(current: Option<f32>, fit: f32, zoom_in: bool) -> Option<Option<f32>> {
    const EPS: f32 = 1e-4;
    let from = current.unwrap_or(fit);
    let step = if zoom_in {
        STEPS.iter().copied().find(|&s| s > from + EPS)
    } else {
        STEPS.iter().rev().copied().find(|&s| s < from - EPS)
    }?;
    let to_fit = match current {
        Some(c) if zoom_in => c < fit - EPS && step >= fit - EPS,
        Some(c) => c > fit + EPS && step <= fit + EPS,
        None => false,
    };
    Some(if to_fit { None } else { Some(step) })
}

/// A center share kept so the picture covers the area along that axis, or
/// the middle when the picture is no larger than the area.
fn clamp_center(center: f32, drawn: f32, area: f32) -> f32 {
    if drawn <= area {
        0.5
    } else {
        let half = area / (2.0 * drawn);
        center.clamp(half, 1.0 - half)
    }
}

/// Where the picture starts along an axis, from the area's start.
fn offset(center: f32, drawn: f32, area: f32) -> f32 {
    if drawn <= area {
        (area - drawn) / 2.0
    } else {
        area / 2.0 - clamp_center(center, drawn, area) * drawn
    }
}

/// Whether a zoom needs more pixels than were decoded and a larger decode
/// would have them: `needed` device pixels along the longer side.
fn wants_sharper(needed: f32, decoded: u32, own: u32, sharp: bool) -> bool {
    !sharp && needed > decoded as f32 + 0.5 && decoded < own.min(PICTURE_SIDE)
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
        let sharp = self.picture.as_ref().is_some_and(|mode| mode.sharp);
        let max_side = if sharp {
            PICTURE_SIDE
        } else {
            side_limit(display_side(window, cx))
        };
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
                let image = Arc::new(RenderImage::new([picture.frame]));
                let decoded = image.size(0);
                let new = Shown {
                    decoded: (
                        decoded.width.0.max(0) as u32,
                        decoded.height.0.max(0) as u32,
                    ),
                    image,
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
    /// Ctrl-= / Ctrl-- in image mode: the next step, keeping the point at
    /// the area's center.
    pub(super) fn zoom_picture(
        &mut self,
        zoom_in: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let area = area(window, cx);
        let Some(mode) = self.picture.as_mut() else {
            return;
        };
        let Some(shown) = &mode.shown else {
            return;
        };
        let fit = fit_scale((shown.width, shown.height), area);
        let Some(next) = next_zoom(mode.zoom.map(|z| z.scale), fit, zoom_in) else {
            return;
        };
        let center = mode.zoom.map_or((0.5, 0.5), |z| z.center);
        mode.zoom = next.map(|scale| Zoomed { scale, center });
        mode.drag = None;
        self.clamp_picture(area);
        if let Some(scale) = next {
            self.sharpen(scale, window, cx);
        }
        cx.notify();
    }

    /// Ctrl-0 in image mode: back to fit.
    pub(super) fn fit_picture(&mut self, cx: &mut Context<Self>) {
        if let Some(mode) = self.picture.as_mut() {
            mode.zoom = None;
            mode.drag = None;
        }
        cx.notify();
    }

    /// Decodes again at up to `PICTURE_SIDE` when `scale` draws more pixels
    /// than the display-sized decode has. Once per mode; the old picture
    /// stays until the new one is ready.
    fn sharpen(&mut self, scale: f32, window: &mut Window, cx: &mut Context<Self>) {
        let factor = window.scale_factor();
        let Some(mode) = self.picture.as_mut() else {
            return;
        };
        let Some(shown) = &mode.shown else {
            return;
        };
        let (own, decoded) = if shown.width >= shown.height {
            (shown.width, shown.decoded.0)
        } else {
            (shown.height, shown.decoded.1)
        };
        if wants_sharper(own as f32 * scale * factor, decoded, own, mode.sharp) {
            mode.sharp = true;
            self.read_picture(window, cx);
        }
    }

    /// Keeps a zoomed picture's center inside its edges for `area`.
    fn clamp_picture(&mut self, area: (f32, f32)) {
        let Some(mode) = self.picture.as_mut() else {
            return;
        };
        let (Some(zoom), Some(shown)) = (mode.zoom.as_mut(), &mode.shown) else {
            return;
        };
        let drawn = (
            shown.width as f32 * zoom.scale,
            shown.height as f32 * zoom.scale,
        );
        zoom.center = (
            clamp_center(zoom.center.0, drawn.0, area.0),
            clamp_center(zoom.center.1, drawn.1, area.1),
        );
    }

    /// Moves a zoomed picture's view by `by` points (positive: right and
    /// down, so the picture moves left and up). Nothing at fit.
    pub(super) fn pan_picture(&mut self, by: (f32, f32), window: &Window, cx: &mut Context<Self>) {
        let area = area(window, cx);
        let Some(mode) = self.picture.as_mut() else {
            return;
        };
        let (Some(zoom), Some(shown)) = (mode.zoom.as_mut(), &mode.shown) else {
            return;
        };
        let drawn = (
            shown.width as f32 * zoom.scale,
            shown.height as f32 * zoom.scale,
        );
        zoom.center.0 += by.0 / drawn.0.max(1.0);
        zoom.center.1 += by.1 / drawn.1.max(1.0);
        self.clamp_picture(area);
        cx.notify();
    }

    /// Up, Down, PageUp, PageDown, Home and End on a zoomed picture.
    pub(super) fn scroll_picture(
        &mut self,
        scroll: Scroll,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let (_, h) = area(window, cx);
        let by = match scroll {
            Scroll::Lines(n) => n.signum() * h * KEY_STEP,
            Scroll::Pages(n) => n.signum() * h * PAGE_STEP,
            // Clamped to the edge.
            Scroll::Start => -1e9,
            Scroll::End => 1e9,
        };
        self.pan_picture((0.0, by), window, cx);
    }

    /// Left/Right on a zoomed picture.
    pub(super) fn scroll_picture_h(
        &mut self,
        right: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let (w, _) = area(window, cx);
        let by = if right { w * KEY_STEP } else { -w * KEY_STEP };
        self.pan_picture((by, 0.0), window, cx);
    }

    fn on_picture_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some(mode) = self.picture.as_mut().filter(|m| m.zoom.is_some()) {
            mode.drag = Some(event.position);
            cx.notify();
        }
    }

    /// A mouse move: moves the picture while a drag of it runs. True when it
    /// took the move.
    pub(super) fn drag_picture(
        &mut self,
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mode) = self.picture.as_mut() else {
            return false;
        };
        let Some(last) = mode.drag else {
            return false;
        };
        if pressed != Some(MouseButton::Left) {
            mode.drag = None; // released outside the window
            cx.notify();
            return true;
        }
        mode.drag = Some(position);
        let by = (
            f32::from(last.x - position.x),
            f32::from(last.y - position.y),
        );
        self.pan_picture(by, window, cx);
        true
    }

    pub(super) fn end_picture_drag(&mut self) {
        if let Some(mode) = self.picture.as_mut() {
            mode.drag = None;
        }
    }

    pub(super) fn render_picture(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mode = self.picture.as_ref();
        let shown = mode.and_then(|mode| mode.shown.as_ref());
        let zoom = mode.and_then(|mode| mode.zoom);
        let dragging = mode.is_some_and(|mode| mode.drag.is_some());
        let area = area(window, cx);
        // A zoomed picture's place (from the area's corner) and size, in
        // points, not rems: one picture pixel is `scale` points whatever the
        // viewer's text level.
        let placed = shown.zip(zoom).map(|(shown, zoom)| {
            let drawn = (
                shown.width as f32 * zoom.scale,
                shown.height as f32 * zoom.scale,
            );
            let at = (
                offset(zoom.center.0, drawn.0, area.0),
                offset(zoom.center.1, drawn.1, area.1),
            );
            (at, drawn)
        });
        let movable = placed.is_some_and(|(_, drawn)| drawn.0 > area.0 || drawn.1 > area.1);
        let image = shown.map(|shown| shown.image.clone());
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
                    .id("viewer-picture-area")
                    .debug_selector(|| "viewer-picture-area".into())
                    .absolute()
                    .top(inset)
                    .left(inset)
                    .right(inset)
                    .bottom(inset)
                    .overflow_hidden()
                    .when(movable, |this| {
                        this.cursor(if dragging {
                            CursorStyle::ClosedHand
                        } else {
                            CursorStyle::OpenHand
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, _, cx| this.on_picture_mouse_down(event, cx)),
                    )
                    .when_some(image, |this, image| {
                        let picture = img(ImageSource::Render(image))
                            .debug_selector(|| "viewer-picture-image".into())
                            .absolute();
                        this.child(match placed {
                            Some((at, drawn)) => picture
                                .left(px(at.0))
                                .top(px(at.1))
                                .w(px(drawn.0))
                                .h(px(drawn.1))
                                .object_fit(ObjectFit::Fill),
                            None => picture.size_full().object_fit(ObjectFit::ScaleDown),
                        })
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

    #[test]
    fn fit_never_enlarges() {
        assert_eq!(fit_scale((400, 300), (800.0, 800.0)), 1.0);
        assert_eq!(fit_scale((400, 300), (200.0, 300.0)), 0.5);
        assert_eq!(fit_scale((400, 300), (400.0, 75.0)), 0.25);
        assert_eq!(fit_scale((0, 0), (10.0, 10.0)), 1.0);
    }

    #[test]
    fn zoom_steps_stop_at_fit_on_the_way() {
        // From fit: the next step past it.
        assert_eq!(next_zoom(None, 0.37, true), Some(Some(0.5)));
        assert_eq!(next_zoom(None, 0.37, false), Some(Some(0.25)));
        assert_eq!(next_zoom(None, 1.0, true), Some(Some(1.5)));
        // Crossing or reaching fit stops there.
        assert_eq!(next_zoom(Some(0.5), 0.37, false), Some(None));
        assert_eq!(next_zoom(Some(0.25), 0.37, true), Some(None));
        assert_eq!(next_zoom(Some(1.5), 1.0, false), Some(None));
        assert_eq!(next_zoom(Some(0.75), 0.37, false), Some(Some(0.5)));
        // The ends.
        assert_eq!(next_zoom(Some(8.0), 1.0, true), None);
        assert_eq!(next_zoom(Some(0.1), 1.0, false), None);
        assert_eq!(next_zoom(None, 0.05, false), None);
    }

    #[test]
    fn a_zoomed_picture_covers_its_area() {
        assert_eq!(clamp_center(0.0, 1000.0, 500.0), 0.25);
        assert_eq!(clamp_center(1.0, 1000.0, 500.0), 0.75);
        assert_eq!(clamp_center(0.6, 1000.0, 500.0), 0.6);
        assert_eq!(clamp_center(0.1, 400.0, 500.0), 0.5, "smaller: centered");
        assert_eq!(offset(0.25, 1000.0, 500.0), 0.0);
        assert_eq!(offset(0.75, 1000.0, 500.0), -500.0);
        assert_eq!(offset(0.0, 400.0, 500.0), 50.0);
    }

    #[test]
    fn a_zoom_past_the_decoded_pixels_decodes_again() {
        assert!(wants_sharper(3000.0, 1920, 4032, false));
        assert!(!wants_sharper(1900.0, 1920, 4032, false), "enough pixels");
        assert!(!wants_sharper(3000.0, 1920, 4032, true), "once");
        assert!(!wants_sharper(3000.0, 1920, 1920, false), "all it has");
        assert!(
            !wants_sharper(9000.0, PICTURE_SIDE, 8000, false),
            "the limit"
        );
    }
}
