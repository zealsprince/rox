//! The window backdrop per ADR 10: the playing track's art, blurred once
//! per track change and never per frame. gpui has no runtime blur
//! (`blur_radius` is shadow-only), so a small [`RenderImage`] is baked on
//! the background executor and the bilinear upscale does the rest.
//! [`NowPlayingArt`] owns the bake; [`WindowBackdrop`] paints it and retires
//! old textures from each window's atlas so a long session doesn't leak one
//! per track. The same bake extracts the palette seed, keyed by player.
//!
//! A remote row's picture comes from the thumbnail pool by its path string;
//! a station may replace it with [`crate::radio_art`]'s guess for the song.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Entity, Image, ImageFormat, ObjectFit, Pixels, RenderImage, Rgba,
    Subscription, Window, div, img, prelude::*,
};
use image::{Frame, RgbaImage};
use std::sync::RwLock;

use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;
use rox_playback::IcyTitle;

use crate::player::Player;
use crate::radio::{Radio, TitleChanged};
use crate::radio_art::{self, StationArt};
use crate::thumbs::Thumbs;

/// The backdrop shader's element, built by the app, which owns the shader
/// machinery. None paints the bake bare.
type ShadeFn = dyn Fn(&Window, &App) -> Option<AnyElement> + Send + Sync;

static SHADE: RwLock<Option<Arc<ShadeFn>>> = RwLock::new(None);

pub fn set_shade(shade: impl Fn(&Window, &App) -> Option<AnyElement> + Send + Sync + 'static) {
    *SHADE.write().unwrap() = Some(Arc::new(shade));
}

/// The service can't tell a workspace from a settings window, so the app's
/// hook decides. Unregistered means every window paints one.
type GateFn = dyn Fn(&Window, &App) -> bool + Send + Sync;

static GATE: RwLock<Option<Arc<GateFn>>> = RwLock::new(None);

pub fn set_gate(gate: impl Fn(&Window, &App) -> bool + Send + Sync + 'static) {
    *GATE.write().unwrap() = Some(Arc::new(gate));
}

/// Small enough that decode, blur, and upload cost nothing next to the art read.
const BAKE_SIZE: u32 = 128;

/// Covers any surface's morph off the old handle (`EASE_SECS` is a third of
/// a second), with room to spare.
const RETIRE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

const BLUR_SIGMA: f32 = 8.0;

/// A remote row carries the path string the thumbnail pool keys its picture on.
#[derive(Clone, PartialEq)]
enum Playing {
    File(PathBuf),
    Row(String),
}

impl Playing {
    fn of(key: &TrackKey) -> Self {
        if key.is_local() {
            Playing::File(key.path.clone())
        } else {
            Playing::Row(key.path.to_string_lossy().into_owned())
        }
    }
}

/// One per workspace, so each window's backdrop follows its own player.
pub struct NowPlayingArt {
    player: Entity<Player>,
    thumbs: Entity<Thumbs>,
    current: Option<Playing>,
    /// A remote row's two pictures: the song on air and the row's own.
    station: StationArt,
    backdrop: Option<Arc<RenderImage>>,
    /// The bake's source as an `img` handle for the cover surfaces, remote
    /// rows only. Built once per change: re-minting it every paint would
    /// re-hash the bytes.
    live: Option<Arc<Image>>,
    /// Replaced handles waiting out [`RETIRE_AFTER`]. A surface may still
    /// paint the old handle, and dropping a texture under a paint panics in
    /// the atlas.
    retiring: Vec<(Arc<Image>, Instant)>,
    generation: u64,
    /// The picture can change without the row moving (a station turnover),
    /// so bakes carry their own stamp.
    bake_gen: u64,
    /// So an unchanged picture doesn't cost a bake and a cross-fade to itself.
    shown: Option<u64>,
    _player_changed: Subscription,
    _title_changed: Subscription,
}

impl NowPlayingArt {
    pub fn new(
        player: Entity<Player>,
        radio: &Entity<Radio>,
        thumbs: &Entity<Thumbs>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _player_changed = cx.observe(&player, |this: &mut Self, _, cx| this.sync(cx));
        let _title_changed = cx.subscribe(radio, |this: &mut Self, _, event: &TitleChanged, cx| {
            this.on_title(&event.title, cx);
        });

        NowPlayingArt {
            player,
            thumbs: thumbs.clone(),
            current: None,
            station: StationArt::default(),
            backdrop: None,
            live: None,
            retiring: Vec::new(),
            generation: 0,
            bake_gen: 0,
            shown: None,
            _player_changed,
            _title_changed,
        }
    }

    pub fn backdrop(&self) -> Option<Arc<RenderImage>> {
        self.backdrop.clone()
    }

    /// The same precedence the backdrop follows, so the blur and the
    /// transport's cover are never two different pictures. None for a local
    /// track.
    pub fn live_art(&self) -> Option<Arc<Image>> {
        self.live.clone()
    }

    /// Runs every pump tick, so everything up to the path compare stays cheap.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.retire_due(cx);

        let (playing, between_tracks) = {
            let player = self.player.read(cx);
            let now = player.now_playing();
            // Keyed on the file: cue tracks of one image share its cover.
            let playing = now.as_ref().map(|now| Playing::of(&now.key));
            // The position clock blinks off between tracks with the session
            // still alive.
            let between_tracks = now.is_none() && player.is_active() && !player.queue_ended();
            (playing, between_tracks)
        };
        // Hold through the blink rather than flash the backdrop every song.
        if between_tracks {
            return;
        }
        if playing == self.current {
            return;
        }

        self.current = playing.clone();
        self.generation += 1;
        let generation = self.generation;
        self.station.clear();

        match playing {
            Some(Playing::File(path)) => {
                cx.spawn(async move |this, cx| {
                    let bytes = cx
                        .background_executor()
                        .spawn(async move {
                            rox_library::art::cover_art(&path).map(|(bytes, _mime)| bytes)
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.show(bytes, cx);
                    })
                    .ok();
                })
                .detach();
            }

            // A queued server row may never have been on screen, so fetch
            // its cover now if the pool has none.
            Some(Playing::Row(key)) => {
                let Some(conn) = self.thumbs.read(cx).store_conn() else {
                    self.show(None, cx);
                    return;
                };

                cx.spawn(async move |this, cx| {
                    let bytes = cx
                        .background_executor()
                        .spawn(async move { crate::sources::art(&conn, &key) })
                        .await;
                    this.update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.station.set_station(bytes);
                        this.show_station(cx);
                    })
                    .ok();
                })
                .detach();
            }

            None => self.show(None, cx),
        }
    }

    /// Nothing is written anywhere; see [`crate::radio_art`].
    fn on_title(&mut self, title: &IcyTitle, cx: &mut Context<Self>) {
        // A title can arrive before the pump reports the row change.
        if !matches!(self.current, Some(Playing::Row(_))) {
            return;
        }

        let generation = self.station.arm();
        let Some(query) = radio_art::query(title) else {
            if self.station.land(generation, None) {
                self.show_station(cx);
            }
            return;
        };

        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn(async move { radio_art::lookup(&query) })
                .await;
            this.update(cx, |this, cx| {
                if this.station.land(generation, bytes) {
                    this.show_station(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// On the pump cadence, so a swap costs nothing when it happens.
    fn retire_due(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut due = Vec::new();
        self.retiring.retain(|(image, since)| {
            if now.duration_since(*since) < RETIRE_AFTER {
                return true;
            }
            due.push(Arc::clone(image));
            false
        });

        for image in due {
            image.remove_asset(cx);
        }
    }

    fn show_station(&mut self, cx: &mut Context<Self>) {
        let bytes = self.station.current().map(<[u8]>::to_vec);
        self.show(bytes, cx);
    }

    /// Every route into the backdrop ends here. An identical picture is left
    /// alone, so a turnover that finds nothing doesn't cross-fade to itself.
    fn show(&mut self, bytes: Option<Vec<u8>>, cx: &mut Context<Self>) {
        let shown = bytes.as_deref().map(rox_library::hash::fnv1a);
        if shown == self.shown {
            return;
        }
        self.shown = shown;

        // Remote rows only: a file's cover is read off disk by every surface.
        // Retire the replaced handle, since gpui's asset cache never evicts.
        let live = bytes
            .as_ref()
            .filter(|_| matches!(self.current, Some(Playing::Row(_))))
            .map(|bytes| Arc::new(Image::from_bytes(image_format(bytes), bytes.clone())));
        if let Some(old) = std::mem::replace(&mut self.live, live)
            && self.live.as_ref().is_none_or(|new| new.id() != old.id())
        {
            self.retiring.push((old, Instant::now()));
        }

        self.bake_gen += 1;
        let bake_gen = self.bake_gen;

        let Some(bytes) = bytes else {
            if self.backdrop.take().is_some() {
                cx.notify();
            }
            // The tint is keyed by player, so this never touches another window.
            palette::set_seed(self.player.entity_id(), None, cx);
            return;
        };

        cx.spawn(async move |this, cx| {
            let baked = cx
                .background_executor()
                .spawn(async move { bake(&bytes) })
                .await;
            this.update(cx, |this, cx| {
                if this.bake_gen != bake_gen {
                    return;
                }
                let (backdrop, seed) = match baked {
                    Some((image, seed)) => (Some(image), Some(seed)),
                    None => (None, None),
                };
                this.backdrop = backdrop;
                palette::set_seed(this.player.entity_id(), seed, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// gpui decodes by the format it's told rather than by sniffing, and
/// providers mix PNG and JPEG.
fn image_format(bytes: &[u8]) -> ImageFormat {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => ImageFormat::Png,
        Ok(image::ImageFormat::WebP) => ImageFormat::Webp,
        Ok(image::ImageFormat::Gif) => ImageFormat::Gif,
        Ok(image::ImageFormat::Bmp) => ImageFormat::Bmp,
        Ok(image::ImageFormat::Tiff) => ImageFormat::Tiff,
        _ => ImageFormat::Jpeg,
    }
}

/// Downscale, extract the seed, blur, and repack. Off the UI thread.
fn bake(bytes: &[u8]) -> Option<(Arc<RenderImage>, palette::Seed)> {
    let art = image::load_from_memory(bytes).ok()?;
    let small = art.thumbnail(BAKE_SIZE, BAKE_SIZE).into_rgba8();
    let seed = extract_seed(&small);
    let mut baked = image::imageops::blur(&small, BLUR_SIGMA);
    // The renderer needs BGRA, the same swizzle gpui's own decode does.
    for pixel in baked.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some((Arc::new(RenderImage::new(vec![Frame::new(baked)])), seed))
}

/// 15 degrees each, wide enough that one album color doesn't split.
const SEED_BANDS: usize = 24;
const SEED_MIN_CHROMA: f32 = 0.03;
/// 60 degrees: how far a runner-up must be to read as a second color.
const SEED_MIN_SEPARATION: usize = 4;

/// Chroma-weighted mean of the most-voted hue band, and of the strongest
/// band far enough away. Near-gray, near-black, and near-white pixels don't
/// vote, and too little color leaves the seed None rather than amplifying
/// noise. Lightness averages every pixel: it's the bright-album signal.
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
    // A band counts only with the weight of 2% of the cover at minimum chroma.
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

/// Each window that paints the layer keeps its own.
pub struct WindowBackdrop {
    /// Painted at full under the incoming bake, so a same-cover change is
    /// invisible.
    from: Option<Arc<RenderImage>>,
    to: Option<Arc<RenderImage>>,
    fade_at: Instant,
}

impl Default for WindowBackdrop {
    fn default() -> Self {
        WindowBackdrop {
            from: None,
            to: None,
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
        }
    }
}

/// gpui's atlas sampler blends a sprite's outer half texel with the tile
/// packed beside it, which the upscale turns into a band of someone else's
/// texture down the edges. Two texels of overscan push it outside the clip.
const OVERSCAN_TEXELS: f32 = 2.0;

fn sheet(image: &Arc<RenderImage>, opacity: f32, overscan: Pixels) -> AnyElement {
    div()
        .absolute()
        .inset(-overscan)
        .opacity(opacity)
        .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
        .into_any_element()
}

/// Cover-fit on a square bake scales by the window's long side.
fn overscan(window: &Window) -> Pixels {
    let viewport = window.viewport_size();
    viewport.width.max(viewport.height) / BAKE_SIZE as f32 * OVERSCAN_TEXELS
}

impl WindowBackdrop {
    /// An interrupted fade keeps its floor and abandons the barely-shown
    /// intermediate, the cover panel's rule.
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

    /// Paint it first, under every surface. None while there's nothing to show.
    pub fn layer(
        &mut self,
        art: &Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &App,
    ) -> Option<AnyElement> {
        // The switch gates the paint, not the bake, so flipping it mid-track
        // cross-fades right away.
        let allowed = GATE
            .read()
            .unwrap()
            .clone()
            .is_none_or(|gate| gate(window, cx));
        let image = if allowed && palette::art_theming() {
            art.read(cx).backdrop()
        } else {
            None
        };
        self.retarget(image, window);
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        } else {
            if let Some(old) = self.from.take() {
                let _ = window.drop_image(old);
            }
        }
        // The shade goes over the wash and needs no bake: with nothing baked
        // it's the whole layer.
        let shade = SHADE
            .read()
            .unwrap()
            .clone()
            .and_then(|shade| shade(window, cx));
        let baked = self.from.is_some() || self.to.is_some();
        if !baked && shade.is_none() {
            return None;
        }
        let u = u * u * (3.0 - 2.0 * u);
        // The overscan only works under an overflow_hidden root.
        let mut root = div().absolute().inset_0().overflow_hidden();
        let overscan = overscan(window);
        if let Some(from) = &self.from {
            // Under an incoming bake the floor holds at full, so the fade
            // never dips toward black between covers.
            let opacity = if self.to.is_some() { 1.0 } else { 1.0 - u };
            root = root.child(sheet(from, opacity, overscan));
        }
        if let Some(to) = &self.to {
            root = root.child(sheet(to, u, overscan));
        }
        if baked {
            // Backdrop strength, applied as a floor-color wash over the bake.
            root = root.child(div().absolute().inset_0().bg(palette::backdrop_wash()));
        }
        Some(root.children(shade).into_any_element())
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
