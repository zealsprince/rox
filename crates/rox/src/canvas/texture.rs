//! The texture body (ADR 33): a caller-owned texture on a canvas item's
//! quad, through `paint_texture_quad`. It's the seam for frames drawn
//! outside the canvas: whatever produces them hands them over here as
//! [`Frame`]s, the way Milkdrop hands its frames to the renderer.
//!
//! The texture belongs to one window's registry, so it's registered, fed and
//! released from paint, and moves when the canvas pops out. Render targets
//! feed it ([`crate::canvas::target`]). A debug build also proves it with a
//! gradient item (`{"kind": "debug_texture"}`), which a release build keeps
//! as saved like any kind it doesn't know.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, IntoElement, Render, UserTextureId, Window,
    canvas, point, prelude::*, px, size,
};
use rox_canvas::{Content, Projected};

use crate::canvas::bbox;

/// One frame for the texture: `width * height * 4` bytes of straight-alpha
/// RGBA. `seq` moves on with each new frame, so a repaint of the same one
/// costs no upload.
#[derive(Clone)]
pub(crate) struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba8: Arc<Vec<u8>>,
    pub seq: u64,
}

struct Held {
    window: AnyWindowHandle,
    id: UserTextureId,
    size: (u32, u32),
    seq: Option<u64>,
}

/// The texture one item draws from. Reallocated when the frame size moves,
/// released when the item goes.
#[derive(Default)]
pub(crate) struct Texture {
    held: Option<Held>,
}

impl Texture {
    /// Uploads `frame` if it's new, then paints it on `corners`, clockwise
    /// from the top-left in window coordinates. A flat item passes a
    /// screen-aligned box.
    pub(crate) fn paint(
        &mut self,
        frame: &Frame,
        corners: [gpui::Point<gpui::Pixels>; 4],
        opacity: f32,
        window: &mut Window,
        cx: &mut App,
    ) {
        let here = window.window_handle();
        let want = (frame.width, frame.height);

        let fits = self
            .held
            .as_ref()
            .is_some_and(|held| held.window == here && held.size == want);
        if !fits {
            self.release(window, cx);

            match window.register_dynamic_texture(want.0, want.1) {
                Ok(id) => {
                    self.held = Some(Held {
                        window: here,
                        id,
                        size: want,
                        seq: None,
                    })
                }
                Err(e) => {
                    log::warn!("canvas: texture {}x{}: {e}", want.0, want.1);
                    return;
                }
            }
        }

        let Some(held) = self.held.as_mut() else {
            return;
        };

        if held.seq != Some(frame.seq) {
            if let Err(e) = window.update_user_texture(held.id, frame.rgba8.clone()) {
                log::warn!("canvas: texture upload: {e}");
                return;
            }
            held.seq = Some(frame.seq);
        }

        let source = Bounds {
            origin: point(0., 0.),
            size: size(1., 1.),
        };
        window
            .paint_texture_quad(
                corners,
                source,
                held.id,
                [opacity; 4],
                gpui::transparent_black(),
            )
            .ok();
    }

    /// Only the registry that made it can free it. From another window's
    /// paint that one is updated in turn; if it's busy or gone the texture
    /// lives until it closes, the way an unreleased one does.
    pub(crate) fn release(&mut self, window: &mut Window, cx: &mut App) {
        let Some(held) = self.held.take() else {
            return;
        };

        if held.window == window.window_handle() {
            window.release_user_texture(held.id);
        } else {
            held.window
                .update(cx, |_, window, _| window.release_user_texture(held.id))
                .ok();
        }
    }

    fn release_away(&mut self, cx: &mut App) {
        if let Some(held) = self.held.take() {
            held.window
                .update(cx, |_, window, _| window.release_user_texture(held.id))
                .ok();
        }
    }
}

/// One texture item's view. A view of its own, so dropping it with its item
/// frees the texture.
pub(crate) struct TextureView {
    texture: Rc<RefCell<Texture>>,
    frame: Frame,
    face: Option<Projected>,
}

impl TextureView {
    pub(crate) fn new(frame: Frame, cx: &mut Context<Self>) -> TextureView {
        let texture = Rc::new(RefCell::new(Texture::default()));

        let held = texture.clone();
        cx.on_release(move |_, cx| held.borrow_mut().release_away(cx))
            .detach();

        TextureView {
            texture,
            frame,
            face: None,
        }
    }

    /// The frame it paints from now on. One it already shows costs no
    /// upload.
    pub(crate) fn show(&mut self, frame: Frame) {
        self.frame = frame;
    }

    /// Where it paints this frame, panel-relative. None paints nothing.
    pub(crate) fn place(&mut self, face: Option<Projected>) {
        self.face = face;
    }

    /// For the canvas's own cleanup, which has the window in hand.
    pub(crate) fn release(&mut self, window: &mut Window, cx: &mut App) {
        self.texture.borrow_mut().release(window, cx);
    }
}

impl Render for TextureView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(face) = self.face else {
            return gpui::div().into_any_element();
        };

        let texture = self.texture.clone();
        let frame = self.frame.clone();
        on_box(face.corners, move |corners, window, cx| {
            texture
                .borrow_mut()
                .paint(&frame, corners, face.opacity, window, cx);
        })
    }
}

/// A canvas over the box around `corners`, which are panel-relative. The
/// paint gets them back in window coordinates.
pub(crate) fn on_box(
    corners: [[f32; 2]; 4],
    paint: impl FnOnce([gpui::Point<gpui::Pixels>; 4], &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (origin, extent) = bbox(&corners);
    let local = corners.map(|[x, y]| [x - origin[0], y - origin[1]]);

    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let corners =
                local.map(|[x, y]| point(bounds.origin.x + px(x), bounds.origin.y + px(y)));
            paint(corners, window, cx);
        },
    )
    .absolute()
    .left(px(origin[0]))
    .top(px(origin[1]))
    .w(px(extent[0]))
    .h(px(extent[1]))
    .into_any_element()
}

/// The debug build's proof item.
#[cfg(debug_assertions)]
pub(crate) fn is_test(content: &Content) -> bool {
    matches!(content, Content::Unknown(value)
        if value.get("kind").and_then(|kind| kind.as_str()) == Some("debug_texture"))
}

#[cfg(not(debug_assertions))]
pub(crate) fn is_test(_content: &Content) -> bool {
    false
}

/// A still gradient, red across and blue down, for the test item.
pub(crate) fn gradient(width: u32, height: u32) -> Frame {
    let ramp = |at: u32, of: u32| (at * 255 / (of.max(2) - 1)).min(255) as u8;
    let mut rgba8 = Vec::with_capacity(width as usize * height as usize * 4);

    for y in 0..height {
        for x in 0..width {
            rgba8.extend_from_slice(&[ramp(x, width), 96, ramp(y, height), 255]);
        }
    }

    Frame {
        width,
        height,
        rgba8: Arc::new(rgba8),
        seq: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gradient_fills_every_pixel_and_runs_corner_to_corner() {
        let frame = gradient(4, 3);
        assert_eq!(frame.rgba8.len(), 4 * 3 * 4);

        let px = |x: usize, y: usize| &frame.rgba8[(y * 4 + x) * 4..][..4];
        assert_eq!(px(0, 0), &[0, 96, 0, 255]);
        assert_eq!(px(3, 2), &[255, 96, 255, 255]);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_test_item_is_a_kind_of_its_own() {
        let item: rox_canvas::CanvasItem = serde_json::from_value(serde_json::json!({
            "id": "t",
            "content": { "kind": "debug_texture" },
        }))
        .expect("parses");

        assert!(is_test(&item.content));
        assert!(!is_test(&Content::default()));
    }
}
