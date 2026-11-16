//! An image item: one asset from the store on a perspective quad, painted
//! through `paint_image_quad` the way the cover shelf paints its covers.
//!
//! An asset is untrusted bytes: a workspace or an object brought it in. The
//! header is read first and an image over [`MAX_PIXELS`] or [`MAX_SIDE`] is
//! refused before its body is decoded, and the decode runs on the
//! background executor, never the UI thread.
//!
//! A GIF, an APNG or an animated WebP plays. Its frames are held to the
//! header's size, and decoding stops at [`MAX_FRAMES`] or
//! [`MAX_ANIMATION_BYTES`], whichever comes first. Each item keeps its own
//! clock, and the canvas wakes only when the next frame is due, from a
//! render. A canvas that isn't drawn (a hidden tab, a hidden window) stops
//! asking after one wake, and a still image never asks.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Context, RenderImage, Task, Window, canvas, div, point, prelude::*, px,
    size,
};
use image::{AnimationDecoder, ImageDecoder, ImageFormat};
use rox_canvas::{
    Anchor, CanvasItem, Content, Frame, ImageFit, Length, Live, Place, child_frame, project,
};
use rox_core::settings::AssetKind;
use rox_core::settings::assets;
use rox_design::palette;

use crate::canvas::{Placing, bbox, on_stage};
use crate::panels::canvas::CanvasPanel;

/// The most pixels an image may hold, 4096 square. A 4K frame fits with
/// room to spare, and the decoded copy stays under 64 MiB. Placeholder.
const MAX_PIXELS: u64 = 4096 * 4096;

/// The longest side, so a thin strip within the pixel count still fits a
/// texture.
const MAX_SIDE: u32 = 8192;

/// An animation keeps at most this many frames. Every one sits in the
/// window's atlas while it plays. Placeholder.
const MAX_FRAMES: usize = 1000;

/// Every kept frame's decoded bytes, summed: 128 frames of 512 square, or
/// two of the largest still. Placeholder.
const MAX_ANIMATION_BYTES: u64 = 128 << 20;

/// A frame asking for less than this plays for [`SHORT_FRAME_AS`], the way
/// browsers play a GIF that says 0 or 10 ms.
const SHORT_FRAME: Duration = Duration::from_millis(20);
const SHORT_FRAME_AS: Duration = Duration::from_millis(100);

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

/// Decoded images by asset id, for one canvas. Each one sits in the window's
/// atlas until it's dropped here.
#[derive(Default)]
pub(crate) struct Images {
    rev: u64,
    slots: HashMap<String, Slot>,
    /// By item id, so two items on one animation play apart.
    clocks: HashMap<String, Clock>,
    /// The earliest frame due among the animations placed this render.
    due: Option<Instant>,
    /// The one pending wake and when it fires.
    wake: Option<(Instant, Task<()>)>,
}

/// Where one item is in its animation.
#[derive(Clone, Copy, Debug)]
struct Clock {
    frame: usize,
    /// When the current frame went up.
    since: Instant,
}

impl Clock {
    fn new(now: Instant) -> Clock {
        Clock {
            frame: 0,
            since: now,
        }
    }

    /// Moves on by each frame's own delay and answers when the next one is
    /// due. A still has no next frame.
    fn advance(&mut self, delays: &[Duration], now: Instant) -> Option<Instant> {
        if delays.len() < 2 {
            return None;
        }
        if self.frame >= delays.len() {
            *self = Clock::new(now);
        }

        let mut elapsed = now.saturating_duration_since(self.since);

        // Back from a long while undrawn: skip whole loops rather than step
        // through every frame of them.
        let cycle: Duration = delays.iter().sum();
        if elapsed >= cycle {
            let loops = elapsed.as_nanos() / cycle.as_nanos();
            let skipped = Duration::from_nanos((loops * cycle.as_nanos()) as u64);
            self.since += skipped;
            elapsed -= skipped;
        }

        while elapsed >= delays[self.frame] {
            elapsed -= delays[self.frame];
            self.since += delays[self.frame];
            self.frame = (self.frame + 1) % delays.len();
        }

        Some(self.since + delays[self.frame])
    }
}

fn frame_delay(delay: image::Delay) -> Duration {
    let delay = Duration::from(delay);
    if delay < SHORT_FRAME {
        SHORT_FRAME_AS
    } else {
        delay
    }
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

    /// Which of `image`'s frames item `id` shows now. Notes when its next
    /// one is due, for [`wake_when_due`](Self::wake_when_due).
    fn frame(&mut self, id: &str, image: &RenderImage, now: Instant) -> usize {
        let count = image.frame_count();
        if count < 2 {
            self.clocks.remove(id);
            return 0;
        }

        let delays: Vec<Duration> = (0..count).map(|ix| frame_delay(image.delay(ix))).collect();
        let clock = self
            .clocks
            .entry(id.to_string())
            .or_insert_with(|| Clock::new(now));

        let due = clock.advance(&delays, now);
        self.due = match (self.due, due) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        clock.frame
    }

    /// After a render has placed every item: one timer for the earliest
    /// frame due, kept when one already fires in time. With nothing playing
    /// any pending wake goes too.
    pub(crate) fn wake_when_due(&mut self, cx: &mut Context<CanvasPanel>) {
        let Some(due) = self.due.take() else {
            self.wake = None;
            return;
        };
        if self.wake.as_ref().is_some_and(|(at, _)| *at <= due) {
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            let wait = due.saturating_duration_since(Instant::now());
            cx.background_executor().timer(wait).await;

            this.update(cx, |this, cx| {
                this.images_mut().wake = None;
                cx.notify();
            })
            .ok();
        });
        self.wake = Some((due, task));
    }

    /// A miss starts one decode and answers Pending until it lands.
    pub(crate) fn look(&mut self, id: &str, cx: &mut Context<CanvasPanel>) -> Look {
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
                log::warn!("canvas: image {id}: {e}");
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

/// BGRA, every frame of an animation and frame 0 of anything else, refused
/// over the cap before the body is read.
pub(crate) fn decode(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let (format, width, height, limits) = head(bytes)?;

    if let Some(frames) = animation(bytes, format, limits.clone(), width, height)? {
        return Ok(Arc::new(RenderImage::new(frames)));
    }

    still(bytes, limits)
}

/// [`decode`] held to frame 0, for a picture shown small, where an
/// animation's frames would be memory spent on nothing.
pub(crate) fn decode_still(bytes: &[u8]) -> Result<Arc<RenderImage>, String> {
    let (_, _, _, limits) = head(bytes)?;

    still(bytes, limits)
}

fn reader(bytes: &[u8]) -> Result<image::ImageReader<Cursor<&[u8]>>, String> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())
}

/// The format and size off the header alone, refused over the cap.
fn head(bytes: &[u8]) -> Result<(Option<ImageFormat>, u32, u32, image::Limits), String> {
    let head = reader(bytes)?;
    let format = head.format();
    let (width, height) = head.into_dimensions().map_err(|e| e.to_string())?;
    fits(width, height)?;

    // The header could lie, so the decoder is held to the same sides.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);

    Ok((format, width, height, limits))
}

fn still(bytes: &[u8], limits: image::Limits) -> Result<Arc<RenderImage>, String> {
    let mut reader = reader(bytes)?;
    reader.limits(limits);

    let mut pixels = reader.decode().map_err(|e| e.to_string())?.into_rgba8();

    // The renderer needs BGRA, the same swizzle gpui's own decode does.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }

    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// The frames of a GIF, an APNG or an animated WebP, or None for anything
/// that holds one picture.
fn animation(
    bytes: &[u8],
    format: Option<ImageFormat>,
    limits: image::Limits,
    width: u32,
    height: u32,
) -> Result<Option<Vec<image::Frame>>, String> {
    let fail = |e: image::ImageError| e.to_string();

    let frames = match format {
        Some(ImageFormat::Gif) => {
            let mut decoder =
                image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(fail)?;
            decoder.set_limits(limits).map_err(fail)?;
            decoder.into_frames()
        }

        Some(ImageFormat::Png) => {
            let mut decoder =
                image::codecs::png::PngDecoder::new(Cursor::new(bytes)).map_err(fail)?;
            decoder.set_limits(limits).map_err(fail)?;
            if !decoder.is_apng().map_err(fail)? {
                return Ok(None);
            }
            decoder.apng().map_err(fail)?.into_frames()
        }

        Some(ImageFormat::WebP) => {
            let mut decoder =
                image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).map_err(fail)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.set_limits(limits).map_err(fail)?;
            decoder.into_frames()
        }

        _ => return Ok(None),
    };

    let frames = keep_frames(frames, width, height)?;
    Ok((frames.len() > 1).then_some(frames))
}

/// Pulls frames until the caps, checked before each next frame is decoded.
/// What fits plays; a frame the decoder sized off the header is refused.
fn keep_frames(
    frames: impl Iterator<Item = image::ImageResult<image::Frame>>,
    width: u32,
    height: u32,
) -> Result<Vec<image::Frame>, String> {
    let each = width as u64 * height as u64 * 4;
    let mut kept = Vec::new();
    let mut bytes = 0;

    for frame in frames {
        let mut frame = frame.map_err(|e| e.to_string())?;
        if frame.buffer().dimensions() != (width, height) {
            return Err("a frame doesn't match the image's size".into());
        }

        for pixel in frame.buffer_mut().as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        kept.push(frame);
        bytes += each;

        if kept.len() == MAX_FRAMES || bytes + each > MAX_ANIMATION_BYTES {
            log::warn!(
                "canvas: an animation stops at {} frames, its cap",
                kept.len()
            );
            break;
        }
    }

    if kept.is_empty() {
        return Err("it has no frames".into());
    }

    Ok(kept)
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
    item: &CanvasItem,
    parent: &Frame,
    at: &Placing,
    live: &Live,
    images: &mut Images,
    cx: &mut Context<CanvasPanel>,
) -> Option<(f32, AnyElement)> {
    let Content::Image { asset, fit: how } = &item.content else {
        return None;
    };

    // The item's own box, turned and scaled, is the frame the face sits in.
    // The face rides that frame, so it takes no offsets of its own.
    let frame = child_frame(item, parent, live);
    let face_on = |face: [f32; 2]| {
        let face = CanvasItem {
            place: Place {
                anchor: Anchor::Center,
                size: [Length::Units(face[0]), Length::Units(face[1])],
                ..Place::default()
            },
            ..CanvasItem::default()
        };
        project(
            &face,
            &frame,
            &at.camera,
            at.viewport,
            at.pointer,
            false,
            &Live::default(),
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

            // Off the stage it can't be seen, so its clock stops too.
            if !on_stage(&projected.corners, at.viewport) {
                return None;
            }
            let frame = images.frame(&item.id, &image, Instant::now());

            Some((
                projected.depth,
                quad(projected.corners, image, frame, source, projected.opacity),
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
    frame: usize,
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
                    frame,
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
            "canvas-image-missing",
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

    /// A GIF of `frames` solid frames, each shown for `delay_ms`.
    fn gif(frames: usize, side: u32, delay_ms: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            let delay = image::Delay::from_numer_denom_ms(delay_ms, 1);
            let frames = (0..frames).map(|ix| {
                let shade = (ix * 40 % 256) as u8;
                let pixels =
                    image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 0, 0, 255]));
                image::Frame::from_parts(pixels, 0, 0, delay)
            });
            encoder.encode_frames(frames).expect("a gif encodes");
        }
        out
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_still_image_never_asks_for_a_frame() {
        let still = decode(&png(4, 4, [1, 2, 3, 255])).expect("decodes");
        let mut images = Images::default();

        assert_eq!(images.frame("a", &still, Instant::now()), 0);
        assert!(images.due.is_none());
        assert!(images.clocks.is_empty());

        let mut clock = Clock::new(Instant::now());
        assert!(clock.advance(&[ms(100)], Instant::now()).is_none());
    }

    #[test]
    fn the_clock_follows_each_frames_own_delay() {
        let start = Instant::now();
        let mut clock = Clock::new(start);
        let delays = [ms(100), ms(300), ms(50)];

        assert_eq!(
            clock.advance(&delays, start + ms(50)),
            Some(start + ms(100))
        );
        assert_eq!(clock.frame, 0);

        assert_eq!(
            clock.advance(&delays, start + ms(150)),
            Some(start + ms(400))
        );
        assert_eq!(clock.frame, 1);

        assert_eq!(
            clock.advance(&delays, start + ms(399)),
            Some(start + ms(400))
        );
        assert_eq!(clock.frame, 1);

        assert_eq!(
            clock.advance(&delays, start + ms(420)),
            Some(start + ms(450))
        );
        assert_eq!(clock.frame, 2);

        // Round the loop and back to the first frame.
        clock.advance(&delays, start + ms(460));
        assert_eq!(clock.frame, 0);

        // An hour undrawn lands where the loop would be, without stepping
        // through it.
        let later = start + Duration::from_secs(3600) + ms(120);
        clock.advance(&delays, later);
        assert_eq!(clock.frame, 1);
    }

    #[test]
    fn an_animation_keeps_its_frames_and_delays() {
        let image = decode(&gif(3, 8, 70)).expect("decodes");
        assert_eq!(image.frame_count(), 3);
        assert_eq!(Duration::from(image.delay(1)), ms(70));

        // The frames are swizzled like a still.
        let pixels = image.as_bytes(1).expect("frame 1");
        assert_eq!(&pixels[..4], &[0, 0, 40, 255]);

        let mut images = Images::default();
        let now = Instant::now();
        images.frame("a", &image, now);
        assert_eq!(images.due, Some(now + ms(70)));
    }

    #[test]
    fn a_zero_delay_plays_like_a_browser_plays_it() {
        assert_eq!(
            frame_delay(image::Delay::from_numer_denom_ms(0, 1)),
            SHORT_FRAME_AS
        );
        assert_eq!(
            frame_delay(image::Delay::from_numer_denom_ms(10, 1)),
            SHORT_FRAME_AS
        );
        assert_eq!(
            frame_delay(image::Delay::from_numer_denom_ms(40, 1)),
            ms(40)
        );
    }

    /// Frames made on demand, counting how many were asked for, so a test
    /// sees what a decoder would have been made to decode.
    fn lazy_frames(
        side: u32,
        pulled: &std::cell::Cell<usize>,
    ) -> impl Iterator<Item = image::ImageResult<image::Frame>> + '_ {
        std::iter::repeat_with(move || {
            pulled.set(pulled.get() + 1);
            Ok(image::Frame::new(image::RgbaImage::new(side, side)))
        })
    }

    #[test]
    fn decoding_stops_at_the_frame_cap() {
        let pulled = std::cell::Cell::new(0);
        let kept = keep_frames(lazy_frames(1, &pulled), 1, 1).expect("keeps some");

        assert_eq!(kept.len(), MAX_FRAMES);
        assert_eq!(pulled.get(), MAX_FRAMES, "nothing past the cap is decoded");
    }

    #[test]
    fn decoding_stops_at_the_byte_cap() {
        // 1024 square is 4 MiB a frame, so the cap holds 32 of them.
        let pulled = std::cell::Cell::new(0);
        let kept = keep_frames(lazy_frames(1024, &pulled), 1024, 1024).expect("keeps some");

        let fit = (MAX_ANIMATION_BYTES / (1024 * 1024 * 4)) as usize;
        assert_eq!(kept.len(), fit);
        assert_eq!(pulled.get(), fit);
    }

    #[test]
    fn a_frame_off_the_headers_size_is_refused() {
        let pulled = std::cell::Cell::new(0);
        assert!(keep_frames(lazy_frames(4, &pulled), 2, 2).is_err());
    }

    #[test]
    fn a_one_frame_gif_is_a_still() {
        let image = decode(&gif(1, 4, 50)).expect("decodes");
        assert_eq!(image.frame_count(), 1);
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
