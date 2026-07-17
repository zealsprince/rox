//! The window backdrop per ADR 10: the playing track's art, downscaled and
//! gaussian-blurred once per track change, never per frame. GPUI has no
//! runtime blur (`blur_radius` is shadow-only), so the blur is baked into a
//! small [`RenderImage`] on the background executor and the bilinear
//! upscale to window size multiplies it. The shared [`NowPlayingArt`]
//! entity watches the player and owns the bake; window roots paint it
//! through [`WindowBackdrop`], which also retires the previous texture from
//! the window's atlas so a long session doesn't leak one per track. The
//! same bake extracts the derivation seed for the palette's tinted mode;
//! with several windows playing different tracks, the backdrop is per
//! window but the seed is process-global and follows the most recent bake
//! to land.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    div, img, prelude::*, AnyElement, App, Context, Entity, EntityId, ObjectFit, RenderImage, Rgba,
    Subscription, Window,
};
use image::{Frame, RgbaImage};

use crate::design::{palette, tokens};
use crate::player::Player;

/// The longest side of the baked image. Small enough that the decode,
/// blur, and upload cost nothing next to the art read that precedes them;
/// the upscale to window size does the rest of the softening.
const BAKE_SIZE: u32 = 128;

/// The gaussian sigma at bake size, a heavy blur so no cover detail
/// survives into the backdrop.
const BLUR_SIGMA: f32 = 8.0;

/// Who set the palette seed last, by entity. Windows race on the global
/// seed (the ADR's most-recently-started rule, approximated by bake
/// completion order), and only the owner may clear it: one window
/// stopping must not strip the tint another window's play owns.
static SEED_OWNER: Mutex<Option<EntityId>> = Mutex::new(None);

/// The playing track's art resolved once per track change and baked into
/// the backdrop. One per workspace through
/// [`AppState`](crate::panel::AppState), so with several windows playing
/// different tracks each window's backdrop follows its own player.
pub struct NowPlayingArt {
    player: Entity<Player>,
    /// The track the current bake, or the one in flight, belongs to.
    current: Option<PathBuf>,
    backdrop: Option<Arc<RenderImage>>,
    /// Discards stale bake results when the track changes mid-read.
    generation: u64,
    _player_changed: Subscription,
}

impl NowPlayingArt {
    pub fn new(player: Entity<Player>, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&player, |this: &mut Self, _, cx| this.sync(cx));
        NowPlayingArt {
            player,
            current: None,
            backdrop: None,
            generation: 0,
            _player_changed,
        }
    }

    /// The baked backdrop; None while nothing plays or the playing track
    /// has no art.
    pub fn backdrop(&self) -> Option<Arc<RenderImage>> {
        self.backdrop.clone()
    }

    /// Follow the player: a track change kicks one bake on the background
    /// executor, a stop clears the backdrop. The player notifies every
    /// pump tick, so everything up to the path compare stays cheap.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (playing, between_tracks) = {
            let player = self.player.read(cx);
            let playing = player.now_playing().map(|now| now.path);
            // The engine's position clock blinks off for a moment between
            // tracks and while a fresh queue opens, with the session very
            // much alive.
            let between_tracks = playing.is_none() && player.is_active() && !player.queue_ended();
            (playing, between_tracks)
        };
        // Hold through the blink instead of flashing the backdrop and
        // tint out and back on every song.
        if between_tracks {
            return;
        }
        if playing == self.current {
            return;
        }
        self.current = playing.clone();
        self.generation += 1;
        let generation = self.generation;
        let Some(path) = playing else {
            if self.backdrop.take().is_some() {
                cx.notify();
            }
            // Fall back to the plain palette, unless the tint belongs to
            // another window's play.
            if *SEED_OWNER.lock().unwrap() == Some(cx.entity().entity_id()) {
                palette::set_seed(None, cx);
            }
            return;
        };
        cx.spawn(async move |this, cx| {
            let baked = cx
                .background_executor()
                .spawn(async move {
                    let (bytes, _mime) = rox_library::art::cover_art(&path)?;
                    bake(&bytes)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                // A track without art clears the previous track's backdrop
                // and tint rather than leaving them up.
                let (backdrop, seed) = match baked {
                    Some((image, seed)) => (Some(image), Some(seed)),
                    None => (None, None),
                };
                this.backdrop = backdrop;
                *SEED_OWNER.lock().unwrap() = Some(cx.entity().entity_id());
                palette::set_seed(seed, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// One cover into backdrop form: downscale, extract the derivation seed,
/// blur, and repack for the renderer. The heavy work, run off the UI
/// thread once per track change.
fn bake(bytes: &[u8]) -> Option<(Arc<RenderImage>, palette::Seed)> {
    let art = image::load_from_memory(bytes).ok()?;
    let small = art.thumbnail(BAKE_SIZE, BAKE_SIZE).into_rgba8();
    let seed = extract_seed(&small);
    let mut baked = image::imageops::blur(&small, BLUR_SIGMA);
    // The renderer wants BGRA, the same swizzle gpui's own decode does.
    for pixel in baked.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some((Arc::new(RenderImage::new(vec![Frame::new(baked)])), seed))
}

/// The hue bands the seed vote runs over; 15 degrees each, wide enough
/// that one album color doesn't split across a boundary.
const SEED_BANDS: usize = 24;
/// Oklch chroma below this is gray noise: those pixels sit the vote out.
const SEED_MIN_CHROMA: f32 = 0.03;
/// How many bands a runner-up must sit from the winner before it reads
/// as a second color rather than a shade of the first: 60 degrees.
const SEED_MIN_SEPARATION: usize = 4;

/// The palette derivation seed off the pre-blur thumbnail. The primary
/// is the chroma-weighted mean color of the most-voted hue band; the
/// secondary the same off the strongest band far enough away in hue.
/// Near-gray pixels and near-black or near-white ones don't vote, so a
/// dark cover with one vivid element seeds that element; when too little
/// of the cover is colorful to trust, the colors stay None rather than
/// amplifying noise. The lightness mean runs over every pixel, gray mass
/// included - it is the bright-album signal, and the white a colorful
/// cover sits on is exactly what it must see.
fn extract_seed(small: &RgbaImage) -> palette::Seed {
    let mut weight = [0.0f32; SEED_BANDS];
    let mut lightness = [0.0f32; SEED_BANDS];
    let mut chroma = [0.0f32; SEED_BANDS];
    let mut sin = [0.0f32; SEED_BANDS];
    let mut cos = [0.0f32; SEED_BANDS];
    let mut cover_l = 0.0f32;
    for pixel in small.pixels() {
        let color = Rgba {
            r: pixel[0] as f32 / 255.0,
            g: pixel[1] as f32 / 255.0,
            b: pixel[2] as f32 / 255.0,
            a: 1.0,
        };
        let (l, c, h) = palette::rgba_to_oklch(color);
        cover_l += l;
        if c < SEED_MIN_CHROMA || !(0.15..=0.95).contains(&l) {
            continue;
        }
        let band = (((h / std::f32::consts::TAU) + 0.5) * SEED_BANDS as f32) as usize;
        let band = band.min(SEED_BANDS - 1);
        weight[band] += c;
        lightness[band] += l * c;
        chroma[band] += c * c;
        sin[band] += h.sin() * c;
        cos[band] += h.cos() * c;
    }
    let pixels = (small.width() * small.height()).max(1) as f32;
    // The floor: a band only counts if it carries at least the weight of
    // 2% of the cover voting at minimum chroma.
    let floor = pixels * 0.02 * SEED_MIN_CHROMA;
    let band_color = |band: usize| {
        palette::oklch_to_rgba(
            lightness[band] / weight[band],
            chroma[band] / weight[band],
            sin[band].atan2(cos[band]),
            1.0,
        )
    };
    let best = (0..SEED_BANDS)
        .filter(|&band| weight[band] >= floor)
        .max_by(|a, b| weight[*a].total_cmp(&weight[*b]));
    let second = best.and_then(|best| {
        (0..SEED_BANDS)
            .filter(|&band| {
                let apart = band.abs_diff(best);
                apart.min(SEED_BANDS - apart) >= SEED_MIN_SEPARATION
            })
            .filter(|&band| weight[band] >= floor)
            .max_by(|a, b| weight[*a].total_cmp(&weight[*b]))
    });
    palette::Seed {
        primary: best.map(band_color),
        secondary: second.map(band_color),
        lightness: cover_l / pixels,
    }
}

/// A window root's handle on the backdrop: cross-fades from bake to bake
/// and retires abandoned textures from that window's atlas. Each window
/// that paints the layer keeps its own.
pub struct WindowBackdrop {
    /// What the fade leaves behind: painted at full under the incoming
    /// bake so a same-cover track change stays invisible, or fading out
    /// bare when the backdrop clears.
    from: Option<Arc<RenderImage>>,
    to: Option<Arc<RenderImage>>,
    fade_at: Instant,
}

impl Default for WindowBackdrop {
    fn default() -> Self {
        WindowBackdrop {
            from: None,
            to: None,
            // Backdated so a fresh window starts settled.
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
        }
    }
}

/// One bake filling the window at a weight, cover-fit; the bilinear
/// upscale is what multiplies the baked blur.
fn sheet(image: &Arc<RenderImage>, opacity: f32) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .opacity(opacity)
        .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
        .into_any_element()
}

impl WindowBackdrop {
    /// Point the fade at a new bake: the settled slide becomes the floor
    /// of the next fade; an interrupted fade keeps its original floor and
    /// abandons the barely-shown intermediate, the cover panel's rule.
    fn retarget(&mut self, image: Option<Arc<RenderImage>>, window: &mut Window) {
        if self.to.as_ref().map(|i| i.id) == image.as_ref().map(|i| i.id) {
            return;
        }
        let abandoned = if self.fade_at.elapsed().as_secs_f32() >= tokens::EASE_SECS {
            std::mem::replace(&mut self.from, self.to.take())
        } else {
            self.to.take()
        };
        if let Some(old) = abandoned {
            let _ = window.drop_image(old);
        }
        self.to = image;
        self.fade_at = Instant::now();
    }

    /// The backdrop layer for a window root: the current bake cross-fading
    /// over the previous one, clipped to the window. Paint it first, under
    /// every surface; None while there is nothing to show, so the root's
    /// own background shows instead.
    pub fn layer(
        &mut self,
        art: &Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &App,
    ) -> Option<AnyElement> {
        // The song-theming switch gates the paint, not the bake: the bake
        // keeps following the player, so flipping the switch mid-track
        // takes effect right away, riding the normal cross-fade in and out.
        let image = if palette::art_theming() {
            art.read(cx).backdrop()
        } else {
            None
        };
        self.retarget(image, window);
        // Frames only while a fade is running; settled costs zero. On
        // settle the outgoing texture leaves the atlas.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        } else if let Some(old) = self.from.take() {
            let _ = window.drop_image(old);
        }
        if self.from.is_none() && self.to.is_none() {
            return None;
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);
        let mut root = div().absolute().inset_0().overflow_hidden();
        if let Some(from) = &self.from {
            // Under an incoming bake the floor holds at full, so the
            // cross-fade never dips toward black between two covers; with
            // nothing incoming it fades out bare.
            let opacity = if self.to.is_some() { 1.0 } else { 1.0 - u };
            root = root.child(sheet(from, opacity));
        }
        if let Some(to) = &self.to {
            root = root.child(sheet(to, u));
        }
        Some(
            root
                // Backdrop strength, applied as its inverse: a wash of the
                // floor color over the bake.
                .child(div().absolute().inset_0().bg(palette::backdrop_wash()))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;

    fn hue_of(color: Rgba) -> f32 {
        palette::rgba_to_oklch(color).2
    }

    fn hue_dist(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(std::f32::consts::TAU);
        d.min(std::f32::consts::TAU - d)
    }

    /// The Polychrome case: a mostly white cover with a red mass and a
    /// smaller blue one must read bright and carry both colors, red
    /// first.
    #[test]
    fn bright_cover_seeds_both_accents() {
        let small = RgbaImage::from_fn(100, 100, |x, _| {
            image::Rgba(match x {
                0..20 => [255, 0, 0, 255],
                20..30 => [0, 0, 255, 255],
                _ => [255, 255, 255, 255],
            })
        });
        let seed = extract_seed(&small);
        assert!(seed.lightness > 0.7, "read dark: {}", seed.lightness);
        let primary = seed.primary.expect("red should win the vote");
        let secondary = seed.secondary.expect("blue should place");
        assert!(hue_dist(hue_of(primary), hue_of(rgb(0xff0000))) < 0.3);
        assert!(hue_dist(hue_of(secondary), hue_of(rgb(0x0000ff))) < 0.3);
    }

    /// A dark cover with one vivid element seeds that element alone and
    /// reads dark; the gray mass votes for no hue but counts toward the
    /// lightness.
    #[test]
    fn dark_cover_seeds_its_one_color() {
        let small = RgbaImage::from_fn(100, 100, |x, _| {
            image::Rgba(if x < 10 {
                [0, 255, 0, 255]
            } else {
                [20, 20, 20, 255]
            })
        });
        let seed = extract_seed(&small);
        assert!(seed.lightness < 0.5, "read bright: {}", seed.lightness);
        let primary = seed.primary.expect("green should win the vote");
        assert!(hue_dist(hue_of(primary), hue_of(rgb(0x00ff00))) < 0.3);
        assert!(seed.secondary.is_none(), "found a second color in noise");
    }
}
