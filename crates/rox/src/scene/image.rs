//! An image item: one asset from the store on a perspective quad, painted
//! through `paint_image_quad` the way the cover shelf paints its covers.
//!
//! An asset is untrusted bytes: a workspace or an object brought it in. The
//! header is read first and an image over [`MAX_PIXELS`] or [`MAX_SIDE`] is
//! refused before its body is decoded, and the decode runs on the
//! background executor, never the UI thread.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, RenderImage, Window, canvas, div, point, prelude::*, px, size,
};
use rox_core::settings::AssetKind;
use rox_core::settings::assets;
use rox_design::palette;
use rox_scene::{
    Anchor, Content, Frame, ImageFit, Length, Live, Place, SceneItem, child_frame, project,
};

use crate::panels::scene::ScenePanel;
use crate::scene::{Placing, bbox};

/// The most pixels an image may hold, 4096 square. A 4K frame fits with
/// room to spare, and the decoded copy stays under 64 MiB. Placeholder.
const MAX_PIXELS: u64 = 4096 * 4096;

/// The longest side, so a thin strip within the pixel count still fits a
/// texture.
const MAX_SIDE: u32 = 8192;

enum Slot {
    /// Started against this store revision.
    Loading(u64),
    Ready(Arc<RenderImage>),
    /// Not in the store, not an image, over the cap, or broken.
    Failed,
}

pub(crate) enum Look {
    Ready(Arc<RenderImage>),
    Pending,
    Missing,
}

/// Decoded images by asset id, for one scene. Each one sits in the window's
/// atlas until it's dropped here.
#[derive(Default)]
pub(crate) struct Images {
    rev: u64,
    slots: HashMap<String, Slot>,
}

impl Images {
    /// Once per render: a moved store can mean new bytes under an old id.
    pub(crate) fn follow_store(&mut self, window: &mut Window) {
        let rev = assets::rev();
        if rev != self.rev {
            self.clear(window);
            self.rev = rev;
        }
    }

    pub(crate) fn clear(&mut self, window: &mut Window) {
        for (_, slot) in self.slots.drain() {
            if let Slot::Ready(image) = slot {
                window.drop_image(image).ok();
            }
        }
    }

    /// A miss starts one decode and answers Pending until it lands.
    pub(crate) fn look(&mut self, id: &str, cx: &mut Context<ScenePanel>) -> Look {
        match self.slots.get(id) {
            Some(Slot::Ready(image)) => return Look::Ready(image.clone()),
            Some(Slot::Loading(_)) => return Look::Pending,
            Some(Slot::Failed) => return Look::Missing,
            None => {}
        }

        if assets::get(id).is_none() {
            self.slots.insert(id.to_string(), Slot::Failed);
            return Look::Missing;
        }

        let rev = self.rev;
        self.slots.insert(id.to_string(), Slot::Loading(rev));

        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let decoded = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move { load(&id) }
                })
                .await;

            this.update(cx, |this, cx| {
                if this.images_mut().land(&id, rev, decoded) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();

        Look::Pending
    }

    /// False when the store moved while it decoded, so this one is stale.
    fn land(&mut self, id: &str, rev: u64, decoded: Result<Arc<RenderImage>, String>) -> bool {
        if !matches!(self.slots.get(id), Some(Slot::Loading(started)) if *started == rev) {
            return false;
        }

        let slot = match decoded {
            Ok(image) => Slot::Ready(image),
            Err(e) => {
                log::warn!("scene: image {id}: {e}");
                Slot::Failed
            }
        };
        self.slots.insert(id.to_string(), slot);

        true
    }
}

fn load(id: &str) -> Result<Arc<RenderImage>, String> {
    let asset = assets::get(id).ok_or("it isn't in the store")?;
    if asset.kind != AssetKind::Image {
        return Err("it isn't an image".into());
    }

    decode(&assets::read(id)?)
}

/// BGRA, frame 0, refused over the cap before the body is read.
pub(crate) fn decode(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let reader = || {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())
    };

    let (width, height) = reader()?.into_dimensions().map_err(|e| e.to_string())?;
    fits(width, height)?;

    // The header could lie, so the decoder is held to the same sides.
    let mut reader = reader()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);

    let mut pixels = reader.decode().map_err(|e| e.to_string())?.into_rgba8();

    // The renderer needs BGRA, the same swizzle gpui's own decode does.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }

    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

fn fits(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("it has no pixels".into());
    }

    if width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
        return Err(format!("{width} by {height} is past the image cap"));
    }

    Ok(())
}

/// The fraction of the image to sample, and the face's size in the item's
/// units: Cover crops the image, Contain shrinks the face.
pub(crate) fn fit(how: ImageFit, item: [f32; 2], image: [f32; 2]) -> (Bounds<f32>, [f32; 2]) {
    let full = Bounds {
        origin: point(0., 0.),
        size: size(1., 1.),
    };
    if item[0] <= 0. || item[1] <= 0. || image[0] <= 0. || image[1] <= 0. {
        return (full, item);
    }

    let item_ratio = item[0] / item[1];
    let image_ratio = image[0] / image[1];
    let wider = image_ratio > item_ratio;

    match how {
        ImageFit::Stretch => (full, item),

        ImageFit::Cover if wider => {
            let w = item_ratio / image_ratio;
            let source = Bounds {
                origin: point((1. - w) / 2., 0.),
                size: size(w, 1.),
            };
            (source, item)
        }

        ImageFit::Cover => {
            let h = image_ratio / item_ratio;
            let source = Bounds {
                origin: point(0., (1. - h) / 2.),
                size: size(1., h),
            };
            (source, item)
        }

        ImageFit::Contain if wider => (full, [item[0], item[0] / image_ratio]),

        ImageFit::Contain => (full, [item[1] * image_ratio, item[1]]),
    }
}

/// The image's element with its depth, or None while nothing should show.
pub(crate) fn element(
    item: &SceneItem,
    parent: &Frame,
    at: &Placing,
    images: &mut Images,
    cx: &mut Context<ScenePanel>,
) -> Option<(f32, AnyElement)> {
    let Content::Image { asset, fit: how } = &item.content else {
        return None;
    };

    // The item's own box, turned and scaled, is the frame the face sits in.
    let live = Live::default();
    let frame = child_frame(item, parent, &live);
    let face_on = |face: [f32; 2]| {
        let face = SceneItem {
            place: Place {
                anchor: Anchor::Center,
                size: [Length::Units(face[0]), Length::Units(face[1])],
                ..Place::default()
            },
            ..SceneItem::default()
        };
        project(
            &face,
            &frame,
            &at.camera,
            at.viewport,
            at.pointer,
            false,
            &live,
        )
    };

    match images.look(asset, cx) {
        Look::Ready(image) => {
            let pixels = image.size(0);
            let (source, face) = fit(
                *how,
                frame.size,
                [pixels.width.0 as f32, pixels.height.0 as f32],
            );
            let projected = face_on(face)?;

            Some((
                projected.depth,
                quad(projected.corners, image, source, projected.opacity),
            ))
        }

        Look::Pending => None,

        Look::Missing if at.design => {
            let projected = face_on(frame.size)?;
            Some((projected.depth, missing(&projected.corners, asset)))
        }

        Look::Missing => None,
    }
}

/// A canvas over the quad's box. The corners are panel-relative, and the
/// paint re-anchors them to wherever the canvas lands.
fn quad(
    corners: [[f32; 2]; 4],
    image: Arc<RenderImage>,
    source: Bounds<f32>,
    opacity: f32,
) -> AnyElement {
    let (origin, extent) = bbox(&corners);
    let local = corners.map(|[x, y]| [x - origin[0], y - origin[1]]);

    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let corners =
                local.map(|[x, y]| point(bounds.origin.x + px(x), bounds.origin.y + px(y)));
            window
                .paint_image_quad(
                    corners,
                    source,
                    image,
                    0,
                    false,
                    [opacity; 4],
                    gpui::transparent_black(),
                )
                .ok();
        },
    )
    .absolute()
    .left(px(origin[0]))
    .top(px(origin[1]))
    .w(px(extent[0]))
    .h(px(extent[1]))
    .into_any_element()
}

fn missing(corners: &[[f32; 2]; 4], asset: &str) -> AnyElement {
    let (origin, extent) = bbox(corners);

    div()
        .absolute()
        .left(px(origin[0]))
        .top(px(origin[1]))
        .w(px(extent[0]))
        .h(px(extent[1]))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_dashed()
        .border_color(palette::text_faint())
        .text_xs()
        .text_color(palette::text_faint())
        .child(rox_i18n::t!(
            "scene-image-missing",
            asset = asset.to_string()
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("a png encodes");
        out.into_inner()
    }

    /// A BMP header and nothing after it, claiming any size.
    fn bmp_header(width: i32, height: i32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        out
    }

    #[test]
    fn an_image_decodes_to_bgra() {
        let image = decode(&png(2, 1, [200, 10, 20, 255])).expect("decodes");
        let pixels = image.as_bytes(0).expect("frame 0");
        assert_eq!(&pixels[..4], &[20, 10, 200, 255]);
    }

    #[test]
    fn an_oversized_header_is_refused_before_the_body() {
        let refused = decode(&bmp_header(60_000, 60_000)).expect_err("over the cap");
        assert!(refused.contains("past the image cap"), "{refused}");
    }

    #[test]
    fn the_cap_counts_pixels_and_sides() {
        assert!(fits(4096, 4096).is_ok());
        assert!(fits(4097, 4096).is_err());
        assert!(fits(MAX_SIDE, 2).is_ok());
        assert!(fits(MAX_SIDE + 1, 1).is_err());
        assert!(fits(0, 10).is_err());
    }

    #[test]
    fn cover_crops_and_contain_shrinks() {
        let (source, face) = fit(ImageFit::Cover, [100., 100.], [200., 100.]);
        assert_eq!(face, [100., 100.]);
        assert_eq!(source.origin.x, 0.25);
        assert_eq!(source.size.width, 0.5);

        let (source, face) = fit(ImageFit::Contain, [100., 100.], [200., 100.]);
        assert_eq!(source.size.width, 1.);
        assert_eq!(face, [100., 50.]);

        let (_, face) = fit(ImageFit::Contain, [100., 100.], [100., 200.]);
        assert_eq!(face, [50., 100.]);

        let (source, face) = fit(ImageFit::Stretch, [100., 50.], [10., 300.]);
        assert_eq!(face, [100., 50.]);
        assert_eq!(source.size.height, 1.);
    }
}
