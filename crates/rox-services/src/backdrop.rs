//! The window backdrop per ADR 10: the playing track's art, downscaled and
//! gaussian-blurred once per track change, never per frame. gpui has no
//! runtime blur (`blur_radius` is shadow-only), so the blur is baked into a
//! small [`RenderImage`] on the background executor and the bilinear
//! upscale to window size multiplies it. The shared [`NowPlayingArt`]
//! entity watches the player and owns the bake; window roots paint it
//! through [`WindowBackdrop`], which also retires the previous texture from
//! the window's atlas so a long session doesn't leak one per track. The
//! same bake extracts the derivation seed for the palette's tinted mode;
//! with several windows playing different tracks, the backdrop is per
//! window but the seed is process-global and follows the most recent bake
//! to finish.
//!
//! A row from a source with no files under it has no cover to read, so
//! the picture comes from the thumbnail pool by the string that stands in
//! for its path: a station's favicon, a Subsonic song's stored cover. A
//! station goes one further and looks the song on air up online, which is
//! [`crate::radio_art`]'s half; this is where that picture is held and
//! what decides which of the two is showing.

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

/// The shade the app lays over every backdrop layer: the backdrop
/// shader's element, built by the layer above this crate, which owns the
/// shader machinery this crate can't see. Registered once at startup, and
/// asked per window so the app decides which windows show it; None paints
/// the bake bare, which is also every window before the app wires it.
type ShadeFn = dyn Fn(&Window, &App) -> Option<AnyElement> + Send + Sync;

static SHADE: RwLock<Option<Arc<ShadeFn>>> = RwLock::new(None);

/// Register the backdrop shade. The app calls this once at startup;
/// calling again replaces the hook.
pub fn set_shade(shade: impl Fn(&Window, &App) -> Option<AnyElement> + Send + Sync + 'static) {
    *SHADE.write().unwrap() = Some(Arc::new(shade));
}

/// Whether a window paints a backdrop at all, asked the same way as the
/// shade: the service can't tell a workspace from a settings window, so
/// the app's hook says. Unregistered means every window does, which is
/// also the behavior before the app wires it.
type GateFn = dyn Fn(&Window, &App) -> bool + Send + Sync;

static GATE: RwLock<Option<Arc<GateFn>>> = RwLock::new(None);

/// Register the backdrop gate, [`set_shade`]'s twin.
pub fn set_gate(gate: impl Fn(&Window, &App) -> bool + Send + Sync + 'static) {
    *GATE.write().unwrap() = Some(Arc::new(gate));
}

/// The longest side of the baked image. Small enough that the decode,
/// blur, and upload cost nothing next to the art read that precedes them;
/// the upscale to window size does the rest of the softening.
const BAKE_SIZE: u32 = 128;

/// How long a replaced cover handle stays decodable after the swap. Long
/// enough for any surface's morph off it (`EASE_SECS` is a third of a
/// second) plus the frame that started it, with room to spare.
const RETIRE_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// The gaussian sigma at bake size, a heavy blur so no cover detail is
/// left in the backdrop.
const BLUR_SIGMA: f32 = 8.0;

/// What the bake is following. A file is read for its cover the way it
/// always was; a row from a source with no files under it carries the
/// string that stands in for its path, which is what the thumbnail pool
/// keyed its picture on.
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

/// The playing track's art resolved once per track change and baked into
/// the backdrop. One per workspace through
/// the app's shared state, so with several windows playing
/// different tracks each window's backdrop follows its own player.
pub struct NowPlayingArt {
    player: Entity<Player>,
    /// The artwork store, for the rows whose picture is in the pool
    /// rather than in a file.
    thumbs: Entity<Thumbs>,
    /// The row the current bake, or the one in flight, belongs to.
    current: Option<Playing>,
    /// A remote row's two pictures and which of them wins: the song a
    /// station is playing, and the row's own. Empty for a local file,
    /// which reads its cover straight off disk.
    station: StationArt,
    backdrop: Option<Arc<RenderImage>>,
    /// The same picture the bake was made from, kept in the form the cover
    /// surfaces take: a station's song art or its favicon, ready to hand
    /// to `img`. Built once per change beside the bake rather than per
    /// frame, since gpui keys a decode by the bytes' hash and re-minting
    /// the handle every paint would re-hash them. None for a local file,
    /// which every cover surface already reads off disk for itself.
    live: Option<Arc<Image>>,
    /// Handles this replaced and hasn't dropped yet. A cover surface that
    /// took the old handle may still paint it this frame, and a morph
    /// keeps it on screen for the ease after that; dropping the texture
    /// under a paint is a panic in the atlas. So a handle waits here for
    /// [`RETIRE_AFTER`] before it leaves the asset cache.
    retiring: Vec<(Arc<Image>, Instant)>,
    /// Discards stale art reads when the track changes mid-read.
    generation: u64,
    /// Discards stale bakes. The picture can change without the row
    /// moving (a station turning over mid-song), so the bake needs a
    /// stamp of its own rather than riding the row's.
    bake_gen: u64,
    /// What the standing bake was made from, so a picture that hasn't
    /// actually changed doesn't cost a bake and a cross-fade to itself.
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
        // A station's song turns over without the queue moving, so this
        // event is the only thing that says the picture should change
        // while the same row keeps playing.
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

    /// The baked backdrop; None while nothing plays or the playing track
    /// has no art.
    pub fn backdrop(&self) -> Option<Arc<RenderImage>> {
        self.backdrop.clone()
    }

    /// What a remote row has for a cover right now, unblurred: the song the
    /// station announced where the lookup found one, its own picture
    /// otherwise. None for a local track and for a station with neither, so
    /// a cover surface asking this gets either a picture or the honest
    /// nothing it should draw its own placeholder for.
    ///
    /// This is the same precedence the backdrop follows, read off the same
    /// state, which is the point: the blur behind the window and the cover
    /// on the transport are never two different pictures.
    pub fn live_art(&self) -> Option<Arc<Image>> {
        self.live.clone()
    }

    /// Follow the player: a track change kicks one bake on the background
    /// executor, a stop clears the backdrop. The player notifies every
    /// pump tick, so everything up to the path compare stays cheap.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.retire_due(cx);

        let (playing, between_tracks) = {
            let player = self.player.read(cx);
            let now = player.now_playing();
            // Keyed on the file, not the track: cue tracks of one image share
            // its cover, so a boundary between two of them is no reason to
            // bake the backdrop again.
            let playing = now.as_ref().map(|now| Playing::of(&now.key));
            // The engine's position clock blinks off for a moment between
            // tracks and while a fresh queue opens, with the session very
            // much alive.
            let between_tracks = now.is_none() && player.is_active() && !player.queue_ended();
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
        // The row moved, so the station's picture and the song that was on
        // air belong to it rather than to whatever is playing now, and a
        // lookup still out for that song lands on nothing.
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
                        // A track without art clears the previous track's
                        // backdrop and tint rather than leaving them up.
                        this.show(bytes, cx);
                    })
                    .ok();
                })
                .detach();
            }

            // No file to read, so the picture is whatever the pool holds
            // under the row's own key, or for a server row one fetched now,
            // since a track played from the queue may never have been on
            // screen for a list to ask. A station may replace it a moment
            // later with the song it announces.
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

    /// A station announced the next song: look its cover up online and
    /// show that until the song after it. Nothing is written anywhere,
    /// see [`crate::radio_art`].
    fn on_title(&mut self, title: &IcyTitle, cx: &mut Context<Self>) {
        // A title for a row that isn't the one playing here, which is the
        // narrow window between the turnover and the player's own pump
        // reporting the row change.
        if !matches!(self.current, Some(Playing::Row(_))) {
            return;
        }

        let generation = self.station.arm();
        let Some(query) = radio_art::query(title) else {
            // A station that sends one unsplittable field says nothing
            // worth searching on, so its own picture stands for this song
            // too, and the last song's cover comes down.
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

    /// Drop the replaced handles whose grace has run out. On the pump
    /// cadence, so a swap costs nothing at the moment it happens.
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

    /// Put up whichever of a remote row's pictures currently wins.
    fn show_station(&mut self, cx: &mut Context<Self>) {
        let bytes = self.station.current().map(<[u8]>::to_vec);
        self.show(bytes, cx);
    }

    /// Bake a picture and put it up, or clear the backdrop when there is
    /// none. Every route into the backdrop ends here, and each call
    /// abandons the bake before it: a station's picture changes without
    /// the row moving, so the bake carries a stamp of its own. A picture
    /// identical to the standing one is left alone, so a turnover that
    /// finds nothing doesn't cross-fade the station's favicon to itself.
    fn show(&mut self, bytes: Option<Vec<u8>>, cx: &mut Context<Self>) {
        let shown = bytes.as_deref().map(rox_library::hash::fnv1a);
        if shown == self.shown {
            return;
        }
        self.shown = shown;

        // The cover surfaces' copy, for a remote row only: a file's cover
        // is on disk and every one of those surfaces already reads it
        // there. Retiring the handle this replaces is the thumbnail pool's
        // rule, since a decode never leaves gpui's asset cache on its own.
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
            // Fall back to the plain palette for this player's windows; the
            // tint is keyed by the player, so clearing it never touches
            // another window's play.
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
                // A picture that won't decode clears the backdrop and tint
                // rather than leaving the last one up.
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

/// Which decoder the bytes want. gpui decodes by the format it's told
/// rather than by sniffing, so a station's cover coming back as a PNG from
/// one provider and a JPEG from the next has to be named correctly or it
/// simply fails to decode. Anything unrecognized is called a JPEG, which is
/// what the thumbnail pool assumes too; the decode fails either way.
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

/// One cover into backdrop form: downscale, extract the derivation seed,
/// blur, and repack for the renderer. The heavy work, run off the UI
/// thread once per track change.
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

/// The hue bands the seed vote runs over; 15 degrees each, wide enough
/// that one album color doesn't split across a boundary.
const SEED_BANDS: usize = 24;
/// Oklch chroma below this is gray noise, so those pixels are skipped.
const SEED_MIN_CHROMA: f32 = 0.03;
/// How many bands a runner-up must be from the winner before it reads
/// as a second color rather than a shade of the first: 60 degrees.
const SEED_MIN_SEPARATION: usize = 4;

/// The palette derivation seed off the pre-blur thumbnail. The primary
/// is the chroma-weighted mean color of the most-voted hue band; the
/// secondary the same off the strongest band far enough away in hue.
/// Near-gray pixels and near-black or near-white ones don't vote, so a
/// dark cover with one vivid element seeds that element; when too little
/// of the cover is colorful to trust, the colors stay None rather than
/// amplifying noise. The lightness mean runs over every pixel, gray mass
/// included: it's the bright-album signal, and the white behind a
/// colorful cover is exactly what it has to see.
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
    // The floor: a band only counts if it has at least the weight of
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

/// How far past the window the bake is drawn, in baked texels. gpui packs
/// images into a shared atlas and samples them with a linear filter that
/// runs to the tile's exact edge, so a sprite's outermost half texel blends
/// with whatever tile got packed beside it. At bake size that half texel is
/// invisible; blown up to window width it's a several-pixel band of someone
/// else's texture down the left and right edges, which is the weird edge.
/// Two texels of overscan puts the whole contaminated band outside the
/// layer's clip. Cover-fit already crops one axis, so the only cost is a
/// couple of percent more crop on a blur nobody can read anyway.
const OVERSCAN_TEXELS: f32 = 2.0;

/// One bake filling the window at a weight, cover-fit; the bilinear
/// upscale multiplies the baked blur. `overscan` is the atlas-bleed
/// margin, see [`OVERSCAN_TEXELS`].
fn sheet(image: &Arc<RenderImage>, opacity: f32, overscan: Pixels) -> AnyElement {
    div()
        .absolute()
        .inset(-overscan)
        .opacity(opacity)
        .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
        .into_any_element()
}

/// The overscan for this window: whole texels of the bake at the size it
/// gets drawn. Cover-fit on a square bake scales by the window's long side,
/// so that side over [`BAKE_SIZE`] is one texel on screen.
fn overscan(window: &Window) -> Pixels {
    let viewport = window.viewport_size();
    viewport.width.max(viewport.height) / BAKE_SIZE as f32 * OVERSCAN_TEXELS
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
        // takes effect right away, through the normal cross-fade in and
        // out. A gated-off window reads as having no bake, so it fades the
        // same way and the retired textures leave the atlas the usual way.
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
        // Frames only while a fade is running; settled costs zero. On
        // settle the outgoing texture leaves the atlas.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        } else {
            if let Some(old) = self.from.take() {
                let _ = window.drop_image(old);
            }
        }
        // The registered shade is drawn inside the layer, over the wash, so
        // every window that paints a bake gets it the same way and
        // everything drawn after still goes over the result. It doesn't
        // need the bake: with song theming off, or nothing playing, the
        // shade runs over the bare root and is the whole layer.
        let shade = SHADE
            .read()
            .unwrap()
            .clone()
            .and_then(|shade| shade(window, cx));
        let baked = self.from.is_some() || self.to.is_some();
        if !baked && shade.is_none() {
            return None;
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);
        // Clipping makes the overscan work, so the sheets can only
        // ever be children of an overflow_hidden root.
        let mut root = div().absolute().inset_0().overflow_hidden();
        let overscan = overscan(window);
        if let Some(from) = &self.from {
            // Under an incoming bake the floor holds at full, so the
            // cross-fade never dips toward black between two covers; with
            // nothing incoming it fades out bare.
            let opacity = if self.to.is_some() { 1.0 } else { 1.0 - u };
            root = root.child(sheet(from, opacity, overscan));
        }
        if let Some(to) = &self.to {
            root = root.child(sheet(to, u, overscan));
        }
        if baked {
            // Backdrop strength, applied as its inverse: a wash of the
            // floor color over the bake. Nothing baked has nothing to
            // wash; the root's own background is already the floor.
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

    /// The Polychrome case: a mostly white cover with a red mass and a
    /// smaller blue one must read bright and seed both colors, red
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
