//! Disc dress-up shared by the cover panel and the art shelf: a pure pixel
//! bake (crop the art square, composite the CD or vinyl overlay, cut the
//! hole), plus the LRU cache the shelf needs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use serde::{Deserialize, Serialize};

/// The largest square side a bake takes. A ceiling, never a target: art
/// that arrives smaller bakes at its own size, since upsampling adds no
/// detail and the resample is the costliest step.
pub const DISC_SIZE: u32 = 512;

/// The CD's hole cut as a fraction of the disc radius, matched to the
/// transparent hole in `assets/disc/cd.png`.
const CD_HOLE: f32 = 0.132;

/// The vinyl's label window and spindle hole as fractions of the record
/// radius: an LP's 100 mm label and 7.24 mm hole on the 300 mm record.
/// The window matches the transparent center of `assets/disc/vinyl.png`.
const VINYL_LABEL: f32 = 0.33;
const VINYL_HOLE: f32 = 0.024;

const DISC_AA: f32 = 1.5;

/// How the artwork is drawn: flat, or baked into a CD face or a vinyl label.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscStyle {
    #[default]
    Off,
    Cd,
    Vinyl,
}

/// The bake shapes: the styles, plus the bare circular crop for a disc scan,
/// which already has its own hole and label.
#[derive(Clone, Copy, PartialEq)]
pub enum DiscShape {
    Crop,
    Cd,
    Vinyl,
}

/// The first element of each pair is an i18n key, not display text.
pub const DISC_STYLES: [(&str, DiscStyle); 3] = [
    ("cover-disc-off", DiscStyle::Off),
    ("cover-disc-cd", DiscStyle::Cd),
    ("cover-disc-vinyl", DiscStyle::Vinyl),
];

/// Bake artwork into a disc of `shape`. With an overlay missing or
/// unreadable, the styles fall back to the bare crop.
pub fn bake_disc(bytes: &[u8], shape: DiscShape) -> Option<RgbaImage> {
    let art = image::load_from_memory(bytes).ok()?;
    bake_from(art, shape)
}

/// The stand-in a cover shows until its own bake lands: the style's shape
/// over a neutral square. Built once per style.
pub fn blank_disc(style: DiscStyle) -> Option<Arc<RenderImage>> {
    static CD: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();
    static VINYL: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();
    let (cell, shape) = match style {
        DiscStyle::Cd => (&CD, DiscShape::Cd),
        DiscStyle::Vinyl => (&VINYL, DiscShape::Vinyl),
        DiscStyle::Off => return None,
    };
    cell.get_or_init(|| {
        let plate = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            DISC_SIZE,
            DISC_SIZE,
            image::Rgba([70, 70, 70, 255]),
        ));
        bake_from(plate, shape).map(|disc| Arc::new(RenderImage::new(vec![Frame::new(disc)])))
    })
    .clone()
}

fn bake_from(art: image::DynamicImage, shape: DiscShape) -> Option<RgbaImage> {
    let (width, height) = (art.width(), art.height());
    let side = width.min(height);
    if side == 0 {
        return None;
    }
    let size = side.min(DISC_SIZE);
    let art = art.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let plate = plate(shape, size);
    let mut disc = match (shape, &plate.overlay) {
        (DiscShape::Crop, _) | (_, None) => square(art, size),
        (DiscShape::Cd, Some(overlay)) => {
            let mut disc = square(art, size);
            for (pixel, top) in disc.pixels_mut().zip(overlay.pixels()) {
                pixel.0 = over(top.0, pixel.0);
            }
            disc
        }
        (DiscShape::Vinyl, Some(overlay)) => {
            // The art's square corners stay under the opaque record, so the
            // window's own edge does the masking.
            let label = (VINYL_LABEL * size as f32) as u32;
            let label_art = art.thumbnail_exact(label, label).into_rgba8();
            let offset = (size - label) / 2;
            let mut disc = RgbaImage::new(size, size);
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
            disc
        }
    };
    apply_mask(&mut disc, &plate.mask);
    // The renderer's BGRA, the same swizzle gpui's own decode does.
    for pixel in disc.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(disc)
}

/// Skips the resample, the bake's costliest step, when the art is already square at `size`.
fn square(art: image::DynamicImage, size: u32) -> RgbaImage {
    if art.width() == size && art.height() == size {
        art.into_rgba8()
    } else {
        art.thumbnail_exact(size, size).into_rgba8()
    }
}

/// Built once per shape and size and shared by every bake: the resized
/// overlay and the geometry mask.
struct Plate {
    overlay: Option<Arc<RgbaImage>>,
    mask: Arc<Vec<u8>>,
}

type Plates = Mutex<HashMap<(u8, u32), Arc<Plate>>>;

fn plate(shape: DiscShape, size: u32) -> Arc<Plate> {
    static PLATES: OnceLock<Plates> = OnceLock::new();
    let (style, hole) = match shape {
        DiscShape::Crop => (None, None),
        DiscShape::Cd => (Some(DiscStyle::Cd), Some(CD_HOLE)),
        DiscShape::Vinyl => (Some(DiscStyle::Vinyl), Some(VINYL_HOLE)),
    };
    let key = (shape as u8, size);
    let plates = PLATES.get_or_init(Default::default);
    if let Some(plate) = plates.lock().unwrap().get(&key) {
        return plate.clone();
    }
    // Built outside the lock: two racing bakes both doing the work beats
    // every bake queueing behind one resample.
    let plate = Arc::new(Plate {
        overlay: style.and_then(|style| disc_overlay(style, size)),
        mask: Arc::new(circle_mask(size, hole)),
    });
    plates.lock().unwrap().entry(key).or_insert(plate).clone()
}

fn disc_overlay(style: DiscStyle, size: u32) -> Option<Arc<RgbaImage>> {
    static CD: OnceLock<Option<image::DynamicImage>> = OnceLock::new();
    static VINYL: OnceLock<Option<image::DynamicImage>> = OnceLock::new();
    let (cell, path) = match style {
        DiscStyle::Cd => (&CD, "disc/cd.png"),
        DiscStyle::Vinyl => (&VINYL, "disc/vinyl.png"),
        DiscStyle::Off => return None,
    };
    let source = cell
        .get_or_init(|| {
            let file = crate::assets::Assets::get(path)?;
            image::load_from_memory(&file.data).ok()
        })
        .as_ref()?;
    Some(Arc::new(source.thumbnail_exact(size, size).into_rgba8()))
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

fn circle_mask(size: u32, hole: Option<f32>) -> Vec<u8> {
    let center = (size as f32 - 1.0) / 2.0;
    let radius = center;
    let mut mask = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let r = (dx * dx + dy * dy).sqrt();
            let mut alpha = ((radius - r) / DISC_AA).clamp(0.0, 1.0);
            if let Some(hole) = hole {
                alpha *= ((r - hole * radius) / DISC_AA).clamp(0.0, 1.0);
            }
            mask.push((alpha * 255.0).round() as u8);
        }
    }
    mask
}

fn apply_mask(disc: &mut RgbaImage, mask: &[u8]) {
    for (pixel, &alpha) in disc.pixels_mut().zip(mask) {
        if alpha != u8::MAX {
            pixel.0[3] = ((pixel.0[3] as u16 * alpha as u16) / 255) as u8;
        }
    }
}

/// Several visible windows' worth, so a scrub that doubles back doesn't re-bake.
const CACHE_CAP: usize = 128;

/// Bakes in flight at once. gpui's background executor runs one worker per
/// core, and a pool that reaches the worker count starves the main thread:
/// audio clocks, waveforms, and every panel stall until the queue drains. A
/// quarter of the machine keeps the shelf filling without making a frame late.
fn bake_pool() -> usize {
    static POOL: OnceLock<usize> = OnceLock::new();
    *POOL.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|cores| cores.get() / 4)
            .unwrap_or(2)
            .clamp(2, 8)
    })
}

/// A shelf's baked faces by art path, behind a small LRU. The style isn't
/// in the key: flipping it clears the cache.
#[derive(Default)]
pub struct DiscCache {
    entries: HashMap<PathBuf, Entry>,
    clock: u64,
    in_flight: usize,
}

struct Entry {
    slot: Slot,
    touch: u64,
}

enum Slot {
    Pending,
    Ready(Arc<RenderImage>),
    Failed,
}

impl DiscCache {
    /// Touches the entry for the LRU.
    pub fn ready(&mut self, path: &Path) -> Option<Arc<RenderImage>> {
        self.clock += 1;
        let entry = self.entries.get_mut(path)?;
        entry.touch = self.clock;
        match &entry.slot {
            Slot::Ready(disc) => Some(disc.clone()),
            _ => None,
        }
    }

    /// Claim a bake. False when the path is claimed or settled, or the pool
    /// is full; a refusal leaves no trace, so asking again next paint is the
    /// retry.
    pub fn begin(&mut self, path: &Path) -> bool {
        if self.entries.contains_key(path) || self.in_flight >= bake_pool() {
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
        self.in_flight += 1;
        true
    }

    /// A failure sticks so bad art doesn't re-bake every frame. The slot frees
    /// even when a style flip already dropped the entry.
    pub fn finish(&mut self, path: &Path, disc: Option<Arc<RenderImage>>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(entry) = self.entries.get_mut(path) {
            entry.slot = match disc {
                Some(disc) => Slot::Ready(disc),
                None => Slot::Failed,
            };
        }
    }

    /// Running bakes can't be cancelled; their slots stay held until they
    /// land.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Pending entries stay: `finish` needs an entry to write into.
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

        // Small art bakes at its own size, so sample the real dimensions.
        let center = 64 / 2;
        let at = |fraction: f32| center + (fraction * center as f32) as u32;
        let cd = bake_disc(&bytes, DiscShape::Cd).unwrap();
        assert_eq!(cd.width(), 64, "a bake keeps small art at its own size");
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

    /// Not a test: dumps the bakes to /tmp for eyeballing. Run with --ignored.
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
            for pixel in disc.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            disc.save(format!("/tmp/bake-{name}.png")).unwrap();
        }
    }

    #[test]
    fn blank_disc_matches_its_shape_and_stays_cached() {
        assert!(blank_disc(DiscStyle::Off).is_none());
        for style in [DiscStyle::Cd, DiscStyle::Vinyl] {
            let disc = blank_disc(style).expect("a shipped overlay bakes a blank plate");
            let size = disc.size(0);
            assert_eq!(
                (size.width.0, size.height.0),
                (DISC_SIZE as i32, DISC_SIZE as i32)
            );
            let bytes = disc.as_bytes(0).unwrap();
            let stride = size.width.0 as usize * 4;
            let corner_alpha = bytes[3];
            // Past either style's spindle hole, short of the outer edge.
            let ring_x = size.width.0 as usize / 2 + size.width.0 as usize / 4;
            let ring_y = size.height.0 as usize / 2;
            let ring_alpha = bytes[ring_y * stride + ring_x * 4 + 3];
            assert_eq!(
                corner_alpha, 0,
                "the mask cuts the corner outside the circle"
            );
            assert_eq!(ring_alpha, 255, "the disc's own face stays opaque");
            let again = blank_disc(style).expect("still bakes");
            assert!(
                Arc::ptr_eq(&disc, &again),
                "the same style hands back the same instance"
            );
        }
    }

    /// Timing harness for the bake, not a check: run with
    /// `cargo test --release -p rox-panels bake_cost -- --ignored --nocapture`.
    /// Release matters: the dev profile runs this about seven times slower.
    #[test]
    #[ignore]
    fn bake_cost() {
        let art = RgbaImage::from_fn(256, 256, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        let bytes = png_of(art);
        for (name, shape) in [("cd", DiscShape::Cd), ("vinyl", DiscShape::Vinyl)] {
            // Warm the plate so its one-time build isn't in the average.
            bake_disc(&bytes, shape).unwrap();
            let n = 40;
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(bake_disc(&bytes, shape).unwrap());
            }
            println!(
                "{name}: {:.2} ms/bake",
                t.elapsed().as_secs_f64() * 1000.0 / n as f64
            );
        }
    }

    fn face() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(RgbaImage::new(
            4, 4,
        ))]))
    }

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
        let bad = Path::new("bad.png");
        assert!(cache.begin(bad));
        cache.finish(bad, None);
        assert!(cache.ready(bad).is_none());
        assert!(!cache.begin(bad), "bad art doesn't re-bake every frame");
    }

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

    #[test]
    fn the_bake_pool_holds_its_bound() {
        let mut cache = DiscCache::default();
        for i in 0..bake_pool() {
            assert!(cache.begin(&PathBuf::from(format!("{i}.png"))));
        }
        let late = PathBuf::from("late.png");
        assert!(!cache.begin(&late), "the pool is full");
        assert!(
            !cache.entries.contains_key(&late),
            "a refused bake isn't claimed, so the next paint can ask again"
        );
        cache.finish(Path::new("0.png"), Some(face()));
        assert!(cache.begin(&late), "the finished bake freed its slot");
    }

    #[test]
    fn a_cleared_cache_still_frees_its_slots() {
        let mut cache = DiscCache::default();
        for i in 0..bake_pool() {
            assert!(cache.begin(&PathBuf::from(format!("{i}.png"))));
        }
        cache.clear();
        for i in 0..bake_pool() {
            cache.finish(Path::new(&format!("{i}.png")), Some(face()));
        }
        assert_eq!(cache.in_flight, 0);
        assert!(cache.begin(Path::new("fresh.png")));
    }
}
