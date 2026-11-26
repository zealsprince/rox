//! The texture body (ADR 33): a caller-owned texture on a canvas item's
//! quad, through `paint_texture_quad`. It's the seam for frames drawn
//! outside the canvas: whatever produces them hands them over here as
//! [`Frame`]s, the way Milkdrop hands its frames to the renderer.
//!
//! The texture belongs to one window's registry, so it's registered, fed and
//! released from paint, and moves when the canvas pops out. Render targets
//! feed it ([`crate::canvas::target`]), with frames copied out of a slot or,
//! on the GPU path, with a texture another process draws into that this
//! body registers and paints and never uploads to. A debug build also proves
//! it with a gradient item (`{"kind": "debug_texture"}`), which a release
//! build keeps as saved like any kind it doesn't know.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, IntoElement, Render, UserTextureId, Window,
    canvas, point, prelude::*, px,
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

/// What an item shows now.
#[derive(Clone)]
pub(crate) enum Picture {
    /// Pixels to upload.
    Frame(Frame),
    /// The newest frame is in the external texture already, drawn in its
    /// top-left `width` by `height` of `extent`.
    External {
        width: u32,
        height: u32,
        extent: [u32; 2],
    },
}

impl Picture {
    /// The size the picture was drawn at.
    pub(crate) fn size(&self) -> [u32; 2] {
        match self {
            Picture::Frame(frame) => [frame.width, frame.height],
            Picture::External { width, height, .. } => [*width, *height],
        }
    }
}

/// Where a view's external texture goes once it's registered: a render
/// target's supervisor, which lends it to the renderer at spawn.
pub(crate) trait Lender {
    /// The size asked for, while one is.
    fn asked(&self) -> Option<(u32, u32)>;

    /// The registration's outcome. Never blocks.
    fn lend(&self, made: Result<gpui::ExternalTexture, String>);

    /// The texture stayed with the window it was made in, and the item now
    /// paints in another. Never blocks.
    fn lost(&self);

    /// Whether the texture was let go, so the view frees its own. Answers
    /// true once per letting go.
    fn released(&self) -> bool;
}

struct Held {
    window: AnyWindowHandle,
    id: UserTextureId,
    size: (u32, u32),
    seq: Option<u64>,
}

/// A texture another process draws into, registered in one window.
struct External {
    window: AnyWindowHandle,
    id: UserTextureId,
}

/// The textures one item draws from. The dynamic one is reallocated when
/// the frame size moves; both are released when the item goes.
#[derive(Default)]
pub(crate) struct Texture {
    held: Option<Held>,
    external: Option<External>,
}

impl Texture {
    /// Uploads `frame` if it's new, then paints it on `corners`, clockwise
    /// from the top-left in window coordinates. A flat item passes a
    /// screen-aligned box.
    pub(crate) fn paint(
        &mut self,
        frame: &Frame,
        corners: [gpui::Point<gpui::Pixels>; 4],
        source: Bounds<f32>,
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
            // Only the uploaded one: an external texture lent to a renderer
            // that draws its slots stays its own until the item goes.
            if let Some(held) = self.held.take() {
                free(held.window, held.id, window, cx);
            }

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

        quad(window, corners, source, held.id, opacity);
    }

    /// Makes a texture another process draws into, in this window, in place
    /// of any this item held, and answers what that process needs to import
    /// it. Blocks this thread for the GPU's handover, once per registration:
    /// GF-2 measured 0.7 to 4 ms at 1920x1080 on RADV.
    pub(crate) fn register_external(
        &mut self,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<gpui::ExternalTexture, String> {
        self.release_external(window, cx);

        let (id, texture) = window.register_external_texture(width, height)?;
        self.external = Some(External {
            window: window.window_handle(),
            id,
        });

        Ok(texture)
    }

    /// Whether this item holds an external texture made in a window other
    /// than `window`, which can't paint it.
    pub(crate) fn external_elsewhere(&self, window: &Window) -> bool {
        self.external
            .as_ref()
            .is_some_and(|external| external.window != window.window_handle())
    }

    pub(crate) fn release_external(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(external) = self.external.take() {
            free(external.window, external.id, window, cx);
        }
    }

    /// Paints the external texture as the memory holds it now. Nothing is
    /// ever uploaded to it, and nothing here waits on the process drawing
    /// it: its pixels are a stranger's, and only ever pixels.
    pub(crate) fn paint_external(
        &self,
        corners: [gpui::Point<gpui::Pixels>; 4],
        source: Bounds<f32>,
        opacity: f32,
        window: &mut Window,
    ) {
        let Some(external) = &self.external else {
            return;
        };
        if external.window != window.window_handle() {
            return;
        }

        quad(window, corners, source, external.id, opacity);
    }

    /// Only the registry that made a texture can free it. From another
    /// window's paint that one is updated in turn; if it's busy or gone the
    /// texture lives until it closes, the way an unreleased one does.
    pub(crate) fn release(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(held) = self.held.take() {
            free(held.window, held.id, window, cx);
        }
        self.release_external(window, cx);
    }

    fn release_away(&mut self, cx: &mut App) {
        let ids = [
            self.held.take().map(|held| (held.window, held.id)),
            self.external
                .take()
                .map(|external| (external.window, external.id)),
        ];

        for (window, id) in ids.into_iter().flatten() {
            window
                .update(cx, |_, window, _| window.release_user_texture(id))
                .ok();
        }
    }
}

fn free(owner: AnyWindowHandle, id: UserTextureId, window: &mut Window, cx: &mut App) {
    if owner == window.window_handle() {
        window.release_user_texture(id);
    } else {
        owner
            .update(cx, |_, window, _| window.release_user_texture(id))
            .ok();
    }
}

/// The whole texture, 0 to 1 across and down.
pub(crate) const WHOLE: Bounds<f32> = Bounds {
    origin: gpui::Point { x: 0., y: 0. },
    size: gpui::Size {
        width: 1.,
        height: 1.,
    },
};

/// `source` of the texture, in 0 to 1 across and down, on `corners`.
fn quad(
    window: &mut Window,
    corners: [gpui::Point<gpui::Pixels>; 4],
    source: Bounds<f32>,
    id: UserTextureId,
    opacity: f32,
) {
    window
        .paint_texture_quad(corners, source, id, [opacity; 4], gpui::transparent_black())
        .ok();
}

/// One texture item's view. A view of its own, so dropping it with its item
/// frees the texture.
pub(crate) struct TextureView {
    texture: Rc<RefCell<Texture>>,
    picture: Option<Picture>,
    face: Option<Projected>,
    /// A render target on the GPU path, whose texture this view registers
    /// when it's asked for one.
    lender: Option<Arc<dyn Lender>>,
    /// The part of the texture it paints. None paints all of it.
    crop: Option<Bounds<f32>>,
}

impl TextureView {
    pub(crate) fn new(frame: Frame, cx: &mut Context<Self>) -> TextureView {
        TextureView::with(Some(Picture::Frame(frame)), None, cx)
    }

    /// A render target's view, with nothing to show yet.
    pub(crate) fn lending(lender: Arc<dyn Lender>, cx: &mut Context<Self>) -> TextureView {
        TextureView::with(None, Some(lender), cx)
    }

    fn with(
        picture: Option<Picture>,
        lender: Option<Arc<dyn Lender>>,
        cx: &mut Context<Self>,
    ) -> TextureView {
        let texture = Rc::new(RefCell::new(Texture::default()));

        let held = texture.clone();
        cx.on_release(move |_, cx| held.borrow_mut().release_away(cx))
            .detach();

        TextureView {
            texture,
            picture,
            face: None,
            lender,
            crop: None,
        }
    }

    /// What it paints from now on. A frame it already shows costs no
    /// upload, and None paints nothing.
    pub(crate) fn show(&mut self, picture: Option<Picture>) {
        self.picture = picture;
    }

    /// Where it paints this frame, panel-relative. None paints nothing.
    pub(crate) fn place(&mut self, face: Option<Projected>) {
        self.face = face;
    }

    /// Which part of the texture it paints, 0 to 1 across and down. None
    /// paints the whole of it.
    pub(crate) fn crop(&mut self, crop: Option<Bounds<f32>>) {
        self.crop = crop;
    }

    /// For the canvas's own cleanup, which has the window in hand.
    pub(crate) fn release(&mut self, window: &mut Window, cx: &mut App) {
        self.texture.borrow_mut().release(window, cx);
    }

    /// The item moves to another window and keeps its session. What this
    /// window holds is freed here, where it's in hand: the new window may
    /// paint while this one is mid update, and couldn't free it from there.
    /// A lender whose texture it was hears it's lost, as on a dock back.
    pub(crate) fn leave(&mut self, window: &mut Window, cx: &mut App) {
        let mut texture = self.texture.borrow_mut();
        let lent = texture.external.is_some();
        texture.release(window, cx);

        if lent && let Some(lender) = &self.lender {
            lender.lost();
        }
    }
}

impl Render for TextureView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some(face) = self.face else {
            return gpui::div().into_any_element();
        };

        let texture = self.texture.clone();
        let picture = self.picture.clone();
        let lender = self.lender.clone();
        let source = self.crop.unwrap_or(WHOLE);
        on_box(face.corners, move |corners, window, cx| {
            let mut texture = texture.borrow_mut();

            // The one place with the window in hand, so a render target's
            // texture is made here and handed to its supervisor. A dock back
            // moves the item without a teardown, and a texture belongs to the
            // window that made it, so a move asks for another.
            if let Some(lender) = &lender
                && texture.external_elsewhere(window)
            {
                texture.release_external(window, cx);
                lender.lost();
            }
            if let Some(lender) = &lender
                && lender.released()
            {
                texture.release_external(window, cx);
            }
            if let Some(lender) = &lender
                && let Some((width, height)) = lender.asked()
            {
                let made = texture.register_external(width, height, window, cx);
                if let Err(e) = &made {
                    log::info!(
                        "canvas: no GPU texture at {width}x{height}, shared memory it is: {e}"
                    );
                }
                lender.lend(made);
            }

            match &picture {
                Some(Picture::Frame(frame)) => {
                    texture.paint(frame, corners, source, face.opacity, window, cx)
                }
                Some(Picture::External { .. }) => {
                    texture.paint_external(corners, source, face.opacity, window)
                }
                None => {}
            }
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
