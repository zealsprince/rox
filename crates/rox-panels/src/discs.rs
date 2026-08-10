//! Disc dress-up shared by the art surfaces: the cover panel bakes one
//! disc for the playing track, the art shelf bakes a rack of them. The
//! bake itself is a pure pixel pass - crop the art square, composite the
//! CD or vinyl overlay, cut the hole - so it lives here once, with the
//! cache the shelf needs sitting beside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui::RenderImage;
use image::RgbaImage;
use serde::{Deserialize, Serialize};

/// The disc bake's square side, in pixels: big enough that the panel's
/// letterboxed fit stays sharp, small enough that the per-frame rotation
/// stays a couple of milliseconds.
pub const DISC_SIZE: u32 = 512;

/// The CD's hole cut as a fraction of the disc radius, matched to the
/// transparent hole in `assets/disc/cd.png`.
const CD_HOLE: f32 = 0.132;

/// The vinyl's label window and spindle hole as fractions of the record
/// radius: an LP's 100 mm label and 7.24 mm hole on the 300 mm record.
/// The window matches the transparent center of `assets/disc/vinyl.png`.
const VINYL_LABEL: f32 = 0.33;
const VINYL_HOLE: f32 = 0.024;

/// The mask edges' anti-alias falloff, in bake pixels.
const DISC_AA: f32 = 1.5;

/// The dress-up a panel persists: what look the artwork wears. Cd and
/// Vinyl bake the picture into the disc each name carries: the face of a
/// CD under its translucent plastic, or the label of a vinyl record. Off
/// leaves the picture flat.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscStyle {
    #[default]
    Off,
    Cd,
    Vinyl,
}

/// The shape a disc bake takes: the styles above, plus the bare circular
/// crop a spinning disc scan gets, since a real scan carries its own hole
/// and label.
#[derive(Clone, Copy, PartialEq)]
pub enum DiscShape {
    Crop,
    Cd,
    Vinyl,
}

/// The labelled disc styles, the settings rows' and the flyouts' one
/// list.
pub const DISC_STYLES: [(&str, DiscStyle); 3] = [
    ("Off", DiscStyle::Off),
    ("CD", DiscStyle::Cd),
    ("Vinyl", DiscStyle::Vinyl),
];

/// Bake artwork into a disc: the square center crop of the art, masked
/// and dressed by shape. Crop is the bare circle, since a real disc scan
/// carries its own hole and label. CD lays the translucent plastic
/// overlay over the art and cuts the hole; Vinyl shrinks the art into
/// the record's label window and punches the spindle. With an overlay
/// missing or unreadable the styles fall back to the bare crop.
pub fn bake_disc(bytes: &[u8], shape: DiscShape) -> Option<RgbaImage> {
    let art = image::load_from_memory(bytes).ok()?;
    let (width, height) = (art.width(), art.height());
    let side = width.min(height);
    if side == 0 {
        return None;
    }
    let art = art.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let overlay = match shape {
        DiscShape::Crop => None,
        DiscShape::Cd => disc_overlay(DiscStyle::Cd),
        DiscShape::Vinyl => disc_overlay(DiscStyle::Vinyl),
    };
    let mut disc = match (shape, overlay) {
        (DiscShape::Crop, _) | (_, None) => {
            let size = side.min(DISC_SIZE);
            let mut disc = art.thumbnail_exact(size, size).into_rgba8();
            mask_circle(&mut disc, None);
            disc
        }
        (DiscShape::Cd, Some(overlay)) => {
            let mut disc = art.thumbnail_exact(DISC_SIZE, DISC_SIZE).into_rgba8();
            for (pixel, top) in disc.pixels_mut().zip(overlay.pixels()) {
                pixel.0 = over(top.0, pixel.0);
            }
            mask_circle(&mut disc, Some(CD_HOLE));
            disc
        }
        (DiscShape::Vinyl, Some(overlay)) => {
            // The art shrinks to the label window; its square corners
            // reach past the window's circle but stay under the opaque
            // record, so the window's own edge does the masking.
            let label = (VINYL_LABEL * DISC_SIZE as f32) as u32;
            let label_art = art.thumbnail_exact(label, label).into_rgba8();
            let offset = (DISC_SIZE - label) / 2;
            let mut disc = RgbaImage::new(DISC_SIZE, DISC_SIZE);
            for (x, y, pixel) in disc.enumerate_pixels_mut() {
                let base = if (offset..offset + label).contains(&x)
                    && (offset..offset + label).contains(&y)
                {
                    label_art.get_pixel(x - offset, y - offset).0
                } else {
                    [0, 0, 0, 0]
                };
                pixel.0 = over(overlay.get_pixel(x, y).0, base);
            }
            mask_circle(&mut disc, Some(VINYL_HOLE));
            disc
        }
    };
    // The renderer's BGRA, the same swizzle gpui's own decode does.
    for pixel in disc.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(disc)
}

/// The disc overlay art, decoded and sized to the bake once per run.
fn disc_overlay(style: DiscStyle) -> Option<&'static RgbaImage> {
    static CD: OnceLock<Option<RgbaImage>> = OnceLock::new();
    static VINYL: OnceLock<Option<RgbaImage>> = OnceLock::new();
    let (cell, path) = match style {
        DiscStyle::Cd => (&CD, "disc/cd.png"),
        DiscStyle::Vinyl => (&VINYL, "disc/vinyl.png"),
        DiscStyle::Off => return None,
    };
    cell.get_or_init(|| {
        let file = crate::assets::Assets::get(path)?;
        let overlay = image::load_from_memory(&file.data).ok()?;
        Some(overlay.thumbnail_exact(DISC_SIZE, DISC_SIZE).into_rgba8())
    })
    .as_ref()
}

/// Straight-alpha "over": `top` composited onto `base`.
fn over(top: [u8; 4], base: [u8; 4]) -> [u8; 4] {
    let top_a = top[3] as f32 / 255.0;
    let base_a = base[3] as f32 / 255.0 * (1.0 - top_a);
    let alpha = top_a + base_a;
    if alpha <= 0.0 {
        return [0, 0, 0, 0];
    }
    let mix = |t: u8, b: u8| ((t as f32 * top_a + b as f32 * base_a) / alpha).round() as u8;
    [
        mix(top[0], base[0]),
        mix(top[1], base[1]),
        mix(top[2], base[2]),
        (alpha * 255.0).round() as u8,
    ]
}

/// The bake's geometry mask: the anti-aliased outer circle, and the
/// center hole when the shape cuts one.
fn mask_circle(disc: &mut RgbaImage, hole: Option<f32>) {
    let size = disc.width();
    let center = (size as f32 - 1.0) / 2.0;
    let radius = center;
    for (x, y, pixel) in disc.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        let r = (dx * dx + dy * dy).sqrt();
        let mut alpha = ((radius - r) / DISC_AA).clamp(0.0, 1.0);
        if let Some(hole) = hole {
            alpha *= ((r - hole * radius) / DISC_AA).clamp(0.0, 1.0);
        }
        if alpha < 1.0 {
            pixel.0[3] = (pixel.0[3] as f32 * alpha).round() as u8;
        }
    }
}

/// How many baked faces a shelf keeps: several visible windows' worth,
/// so a scrub that doubles back doesn't re-bake what it just dropped.
const CACHE_CAP: usize = 128;

/// A shelf's baked disc faces, keyed by art path. The cover panel gets
/// away with a one-slot swap because it shows one track; the art shelf
/// shows a dozen covers and streams more under a scrub, so its bakes sit
/// behind a small LRU. The style isn't in the key: flipping it clears the
/// cache outright. A cover edit mid-session keeps its old face until
/// then, the staleness the thumbs already accept.
#[derive(Default)]
pub struct DiscCache {
    entries: HashMap<PathBuf, Entry>,
    /// The request clock behind each entry's touch, the LRU's order.
    clock: u64,
}

struct Entry {
    slot: Slot,
    touch: u64,
}

enum Slot {
    /// A bake is in flight; asking again would double it.
    Pending,
    Ready(Arc<RenderImage>),
    /// The bytes wouldn't bake; trying again would fail the same way.
    Failed,
}

impl DiscCache {
    /// The baked face, once it has landed. Touches the entry, so the
    /// eviction sees what the shelf still shows.
    pub fn ready(&mut self, path: &Path) -> Option<Arc<RenderImage>> {
        self.clock += 1;
        let entry = self.entries.get_mut(path)?;
        entry.touch = self.clock;
        match &entry.slot {
            Slot::Ready(disc) => Some(disc.clone()),
            _ => None,
        }
    }

    /// Claim a bake: true means the caller starts one, false that this
    /// path is already in flight or already answered.
    pub fn begin(&mut self, path: &Path) -> bool {
        if self.entries.contains_key(path) {
            return false;
        }
        self.evict();
        self.clock += 1;
        self.entries.insert(
            path.to_path_buf(),
            Entry {
                slot: Slot::Pending,
                touch: self.clock,
            },
        );
        true
    }

    /// Land a bake, or its failure, which sticks so bad art doesn't
    /// re-bake every frame.
    pub fn finish(&mut self, path: &Path, disc: Option<Arc<RenderImage>>) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.slot = match disc {
                Some(disc) => Slot::Ready(disc),
                None => Slot::Failed,
            };
        }
    }

    /// Forget everything, what a style flip does.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Hold the map at the cap by dropping the longest-unseen settled
    /// entries. Pending bakes stay; their tasks are already running and
    /// `finish` needs somewhere to land.
    fn evict(&mut self) {
        while self.entries.len() >= CACHE_CAP {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| !matches!(entry.slot, Slot::Pending))
                .min_by_key(|(_, entry)| entry.touch)
                .map(|(path, _)| path.clone());
            match oldest {
                Some(path) => {
                    self.entries.remove(&path);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(image: RgbaImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// Each shape masks as its physical object: the crop keeps its center
    /// (a scan carries its own hole), the CD shows the art through the
    /// plastic and cuts the hole, the vinyl shrinks the art to the label
    /// window, stays dark across the grooves, and punches the spindle.
    #[test]
    fn bake_shapes_the_disc() {
        let bytes = png_of(RgbaImage::from_pixel(
            64,
            64,
            image::Rgba([200, 10, 10, 255]),
        ));
        let crop = bake_disc(&bytes, DiscShape::Crop).unwrap();
        assert_eq!(crop.get_pixel(0, 0).0[3], 0, "corner should mask out");
        assert_eq!(
            crop.get_pixel(32, 32).0[3],
            255,
            "the crop keeps its center"
        );
        let sample = crop.get_pixel(32, 32).0;
        assert!(
            sample[2] > sample[0],
            "red should land in the BGRA red slot"
        );

        let center = DISC_SIZE / 2;
        let at = |fraction: f32| center + (fraction * center as f32) as u32;
        let cd = bake_disc(&bytes, DiscShape::Cd).unwrap();
        assert_eq!(cd.width(), DISC_SIZE, "styles bake at full size");
        assert_eq!(cd.get_pixel(center, center).0[3], 0, "the CD cuts its hole");
        let face = cd.get_pixel(at(0.6), center).0;
        assert_eq!(face[3], 255);
        assert!(face[2] > face[0], "the art shows through the plastic");

        let vinyl = bake_disc(&bytes, DiscShape::Vinyl).unwrap();
        assert_eq!(
            vinyl.get_pixel(center, center).0[3],
            0,
            "the vinyl punches its spindle hole"
        );
        let label = vinyl.get_pixel(at(0.2), center).0;
        assert!(label[2] > 100, "the label window shows the art");
        let groove = vinyl.get_pixel(at(0.7), center).0;
        assert_eq!(groove[3], 255);
        assert!(groove[2] < 60, "the record stays dark");
    }

    /// Not a test: dumps the bakes to /tmp for eyeballing. Run by hand
    /// with --ignored.
    #[test]
    #[ignore]
    fn dump_bakes() {
        let art = RgbaImage::from_fn(400, 400, |x, y| {
            let checker = ((x / 50) + (y / 50)) % 2 == 0;
            image::Rgba(if checker {
                [200, 60, 30, 255]
            } else {
                [240, 220, 200, 255]
            })
        });
        let bytes = png_of(art);
        for (name, shape) in [
            ("crop", DiscShape::Crop),
            ("cd", DiscShape::Cd),
            ("vinyl", DiscShape::Vinyl),
        ] {
            let mut disc = bake_disc(&bytes, shape).unwrap();
            for pixel in disc.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            disc.save(format!("/tmp/bake-{name}.png")).unwrap();
        }
    }

    fn face() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(RgbaImage::new(
            4, 4,
        ))]))
    }

    /// One claim per path: the first begin starts the bake, the rest wait
    /// on it, and the finish is what ready hands back.
    #[test]
    fn the_cache_claims_once_and_lands_once() {
        let mut cache = DiscCache::default();
        let path = Path::new("a.png");
        assert!(cache.ready(path).is_none());
        assert!(cache.begin(path), "the first ask claims the bake");
        assert!(!cache.begin(path), "the second waits on the first");
        assert!(cache.ready(path).is_none(), "pending has nothing to show");
        cache.finish(path, Some(face()));
        assert!(cache.ready(path).is_some());
        assert!(!cache.begin(path), "a landed bake never re-bakes");
        // A failure sticks the same way.
        let bad = Path::new("bad.png");
        assert!(cache.begin(bad));
        cache.finish(bad, None);
        assert!(cache.ready(bad).is_none());
        assert!(!cache.begin(bad), "bad art doesn't re-bake every frame");
    }

    /// The cap drops the longest-unseen settled entries and leaves the
    /// in-flight ones for their finish.
    #[test]
    fn eviction_keeps_the_cap_and_the_pending() {
        let mut cache = DiscCache::default();
        for i in 0..CACHE_CAP {
            let path = PathBuf::from(format!("{i}.png"));
            assert!(cache.begin(&path));
            cache.finish(&path, Some(face()));
        }
        // Touch the first so the second is the oldest.
        assert!(cache.ready(Path::new("0.png")).is_some());
        let over = PathBuf::from("over.png");
        assert!(cache.begin(&over));
        assert!(cache.entries.len() <= CACHE_CAP);
        assert!(
            cache.ready(Path::new("0.png")).is_some(),
            "the touched entry survives"
        );
        assert!(
            cache.ready(Path::new("1.png")).is_none(),
            "the oldest settled entry goes"
        );
    }
}
