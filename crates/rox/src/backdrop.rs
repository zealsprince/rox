//! The window backdrop per ADR 10: the playing track's art, downscaled and
//! gaussian-blurred once per track change, never per frame. GPUI has no
//! runtime blur (`blur_radius` is shadow-only), so the blur is baked into a
//! small [`RenderImage`] on the background executor and the bilinear
//! upscale to window size multiplies it. The shared [`NowPlayingArt`]
//! entity watches the player and owns the bake; window roots paint it
//! through [`WindowBackdrop`], which also retires the previous texture from
//! the window's atlas so a long session doesn't leak one per track.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, AnyElement, App, Context, Entity, ObjectFit, RenderImage, Subscription,
    Window,
};
use image::Frame;

use crate::player::Player;

/// The longest side of the baked image. Small enough that the decode,
/// blur, and upload cost nothing next to the art read that precedes them;
/// the upscale to window size does the rest of the softening.
const BAKE_SIZE: u32 = 128;

/// The gaussian sigma at bake size, a heavy blur so no cover detail
/// survives into the backdrop.
const BLUR_SIGMA: f32 = 8.0;

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
        let playing = self.player.read(cx).now_playing().map(|now| now.path);
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
                // rather than leaving it up.
                this.backdrop = baked;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// One cover into backdrop form: downscale, blur, and repack for the
/// renderer. The heavy work, run off the UI thread once per track change.
fn bake(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let art = image::load_from_memory(bytes).ok()?;
    let small = art.thumbnail(BAKE_SIZE, BAKE_SIZE).into_rgba8();
    let mut baked = image::imageops::blur(&small, BLUR_SIGMA);
    // The renderer wants BGRA, the same swizzle gpui's own decode does.
    for pixel in baked.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![Frame::new(baked)])))
}

/// A window root's handle on the backdrop: paints the current bake and
/// retires the previous one from that window's texture atlas when it
/// changes. Each window that paints the layer keeps its own.
#[derive(Default)]
pub struct WindowBackdrop {
    painted: Option<Arc<RenderImage>>,
}

impl WindowBackdrop {
    /// The backdrop layer for a window root: the bake filling the window,
    /// cover-fit and clipped. Paint it first, under every surface; None
    /// while there is no bake, so the root's own background shows instead.
    pub fn layer(
        &mut self,
        art: &Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &App,
    ) -> Option<AnyElement> {
        let image = art.read(cx).backdrop();
        if self.painted.as_ref().map(|i| i.id) != image.as_ref().map(|i| i.id) {
            if let Some(old) = self.painted.take() {
                let _ = window.drop_image(old);
            }
            self.painted = image.clone();
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .overflow_hidden()
                .child(img(image?).size_full().object_fit(ObjectFit::Cover))
                .into_any_element(),
        )
    }
}
