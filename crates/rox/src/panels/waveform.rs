//! The waveform panel: the whole track's amplitude shape as mirrored bars
//! around a center line, played bars in the accent, the rest as a dim ghost,
//! with a playhead tracking the position clock. Click or drag the strip to
//! seek. Peaks come from the disk cache ([`crate::peaks`]) when the track
//! has played before, otherwise from a full decode on a background thread
//! that then fills the cache; while a decode runs the strip shows a gray
//! pulsing stand-in shape. Every change of what the strip shows - stand-in
//! to peaks, one track's peaks to the next, blank to anything - is a short
//! morph in geometry and color, never a pop. Painting is a row of quads;
//! with no track up (idle, or the queue played out) the panel is blank and
//! completely still.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, App, Bounds, Context, EventEmitter,
    FocusHandle, Focusable, MouseButton, Pixels, Rgba, SharedString, Subscription, WeakEntity,
    Window,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};

use rox_playback::engine;

use crate::design::{palette, tokens};
use crate::panel::{self, AppState, ScrubState, StatePanel};
use crate::peaks;

/// Resolution of the in-memory peaks. The paint resamples these down to
/// however many bars fit the width.
const PEAK_BINS: usize = 2048;

/// The shortest a bar draws, so quiet passages stay visible.
const MIN_BAR: f32 = 2.0;

enum Peaks {
    /// No track has been seen yet.
    None,
    Decoding,
    Ready(Arc<Vec<(f32, f32)>>),
    Failed,
}

/// One thing the strip can show. The morph runs between two of these,
/// sampled per display bar at paint time.
#[derive(Clone)]
enum Shape {
    /// Zero-height bars: what everything fades in from and out to.
    Blank,
    /// The gray generating stand-in, animated off the panel's clock.
    Placeholder,
    /// A track's decoded pairs and its playhead position: live while the
    /// shape is the target, frozen where it last painted once retired.
    Peaks(Arc<Vec<(f32, f32)>>, f32),
}

impl Shape {
    /// Same visual target: the playhead moving or the stand-in animating
    /// doesn't count, a different peaks buffer does.
    fn same(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Blank, Shape::Blank) | (Shape::Placeholder, Shape::Placeholder) => true,
            (Shape::Peaks(a, _), Shape::Peaks(b, _)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

pub struct WaveformPanel {
    state: AppState,
    /// The track the peaks (or the running decode) belong to.
    track: Option<PathBuf>,
    peaks: Peaks,
    /// Discards stale decode results when the track changes mid-decode.
    generation: u64,
    /// What the strip is morphing from and toward, and when the morph
    /// started.
    from: Shape,
    to: Shape,
    morph_at: Instant,
    /// The strip's painted bounds and drag state, for scrub mapping.
    scrub: ScrubState,
    /// Time zero for the generating animation's phase.
    epoch: Instant,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window notices the
    /// new track without the player bar's frame pump.
    _player_changed: Subscription,
}

impl WaveformPanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        WaveformPanel {
            state,
            track: None,
            peaks: Peaks::None,
            generation: 0,
            from: Shape::Blank,
            to: Shape::Blank,
            morph_at: Instant::now(),
            scrub: ScrubState::default(),
            epoch: Instant::now(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The playing track changed: fetch its peaks off the UI thread - the
    /// disk cache when it holds the track, a full decode that then fills
    /// the cache otherwise - and swap them in when done.
    fn start_decode(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.track = Some(path.clone());
        self.peaks = Peaks::Decoding;
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(peaks) = peaks::load(&path) {
                        return Ok::<_, String>(peaks);
                    }
                    let decoded = engine::decode_peaks(&path, PEAK_BINS)?;
                    peaks::store(&path, &decoded);
                    Ok(decoded)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.peaks = match result {
                    Ok(peaks) => Peaks::Ready(Arc::new(peaks)),
                    Err(e) => {
                        eprintln!("waveform decode failed: {e}");
                        Peaks::Failed
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Point the strip at what it should show: the same shape refreshes in
    /// place (the live playhead), a different one starts a morph from
    /// whatever was showing. A morph interrupted early keeps its original
    /// source, so an intermediate that barely painted - the stand-in when a
    /// cache hit lands a frame after a track switch - never flashes, and
    /// one track's peaks morph straight into the next's.
    fn retarget(&mut self, shape: Shape) {
        if self.to.same(&shape) {
            self.to = shape;
            return;
        }
        if self.morph_at.elapsed().as_secs_f32() >= tokens::EASE_SECS {
            self.from = self.to.clone();
        }
        self.to = shape;
        self.morph_at = Instant::now();
    }

    fn strip(&self) -> impl IntoElement {
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let from = self.from.clone();
        let to = self.to.clone();
        let u = (self.morph_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let t = self.epoch.elapsed().as_secs_f32();
        canvas(
            {
                let scrub = scrub.clone();
                move |bounds, _, _| scrub.set_bounds(bounds)
            },
            move |bounds, _, window, _| {
                paint_morph(&from, &to, u, t, bounds, window);
                panel::scrub_on_paint(&scrub, window, {
                    let player = player.clone();
                    move |fraction, cx| panel::seek_fraction(&player, fraction, cx)
                });
            },
        )
        .size_full()
    }

    fn message(&self, text: &'static str) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(palette::text_muted())
            .child(text)
    }
}

/// The gray the generating stand-in draws in, kept away from the accent so
/// it can't be mistaken for real peaks.
fn placeholder_tint() -> Rgba {
    palette::alpha(palette::text_muted(), 0x33)
}

/// The stand-in's mirrored half-height for bar `i` of `count` at time `t`:
/// a stable pseudo-random profile per slot so the strip reads as audio,
/// swelling under two pulse crests that travel left to right.
fn placeholder_bar(i: usize, count: usize, t: f32, max_bar: f32) -> f32 {
    // The classic one-liner hash: a fixed jagged profile per slot.
    let seed = ((i as f32 * 12.9898).sin() * 43758.547).fract().abs();
    let phase = i as f32 / count as f32 * std::f32::consts::TAU * 2.0 - t * 4.0;
    let pulse = phase.sin() * 0.5 + 0.5;
    ((0.2 + 0.8 * seed) * (0.25 + 0.75 * pulse) * max_bar).max(MIN_BAR / 2.0)
}

/// A shape's bar `i` of `count`: top and bottom in strip-local y, and the
/// bar's color. `x_mid` and `w` place the bar against the shape's playhead
/// for the played/ghost split.
fn sample(
    shape: &Shape,
    i: usize,
    count: usize,
    x_mid: f32,
    w: f32,
    t: f32,
    center: f32,
    max_bar: f32,
) -> (f32, f32, Rgba) {
    match shape {
        Shape::Blank => (center, center, palette::alpha(palette::text_muted(), 0)),
        Shape::Placeholder => {
            let bar = placeholder_bar(i, count, t, max_bar);
            (center - bar, center + bar, placeholder_tint())
        }
        Shape::Peaks(peaks, progress) => {
            if peaks.is_empty() {
                return (center, center, palette::alpha(palette::accent(), 0));
            }
            // Each display bar takes its bucket's extremes so transients
            // survive the downsample.
            let per = peaks.len() as f32 / count as f32;
            let from = (i as f32 * per) as usize;
            let to = (((i + 1) as f32 * per) as usize).clamp(from + 1, peaks.len());
            let (lo, hi) = peaks[from..to]
                .iter()
                .fold((0.0f32, 0.0f32), |(lo, hi), &(bl, bh)| {
                    (lo.min(bl), hi.max(bh))
                });
            let top = center - (hi * max_bar).max(MIN_BAR / 2.0);
            let bottom = center - (lo * max_bar).min(-MIN_BAR / 2.0);
            let played = x_mid <= progress.clamp(0.0, 1.0) * w;
            let color = if played {
                palette::accent()
            } else {
                palette::alpha(palette::accent(), 0x33)
            };
            (top, bottom, color)
        }
    }
}

/// The strip: `to`'s bars, blended per bar from wherever `from` had them
/// while the morph runs, geometry and color both, so shape changes flow
/// instead of popping. Each shape that has a playhead draws it, the
/// retiring one fading out as the incoming one fades in.
fn paint_morph(
    from: &Shape,
    to: &Shape,
    u: f32,
    t: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let count = ((w / (tokens::BAR_W + tokens::BAR_GAP)) as usize).max(1);
    let step = w / count as f32;
    let center = h / 2.0;
    let max_bar = h * 0.46;

    // Smoothstepped so the morph eases out instead of stopping dead.
    let u = u.clamp(0.0, 1.0);
    let u = u * u * (3.0 - 2.0 * u);

    for i in 0..count {
        let x = i as f32 * step;
        let x_mid = x + step * 0.5;
        let (top, bottom, color) = {
            let b = sample(to, i, count, x_mid, w, t, center, max_bar);
            if u < 1.0 {
                let a = sample(from, i, count, x_mid, w, t, center, max_bar);
                (
                    a.0 + (b.0 - a.0) * u,
                    a.1 + (b.1 - a.1) * u,
                    palette::mix(a.2, b.2, u),
                )
            } else {
                b
            }
        };
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(top)),
                size(px(tokens::BAR_W), px(bottom - top)),
            ),
            color,
        ));
    }

    for (shape, weight) in [(from, 1.0 - u), (to, u)] {
        let Shape::Peaks(_, progress) = shape else {
            continue;
        };
        let alpha = (0xd9 as f32 * weight) as u8;
        if alpha == 0 {
            continue;
        }
        let head_x = progress.clamp(0.0, 1.0) * w;
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(head_x - tokens::PLAYHEAD_W / 2.0),
                    bounds.origin.y,
                ),
                size(px(tokens::PLAYHEAD_W), px(h)),
            ),
            palette::alpha(palette::highlight(), alpha),
        ));
    }
}

impl StatePanel for WaveformPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn tab_panel(&self) -> Option<WeakEntity<TabPanel>> {
        self.tab_panel.clone()
    }

    fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self {
        WaveformPanel::new(state, cx)
    }
}

impl EventEmitter<PanelEvent> for WaveformPanel {}

impl Focusable for WaveformPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for WaveformPanel {
    fn panel_name(&self) -> &'static str {
        "waveform"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("waveform")
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = panel::duplicate_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for WaveformPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        // A played-out queue counts as nothing playing: the strip clears
        // instead of sitting there fully lit.
        let now = player.now_playing().filter(|_| !player.queue_ended());

        // Kick a decode when the playing track changes.
        if let Some(now) = &now {
            if self.track.as_deref() != Some(now.path.as_path()) {
                let path = now.path.clone();
                self.start_decode(path, cx);
            }
            // The position clock only moves while a session runs, and pause
            // and track skips do not notify; poll by frame like the player
            // bar does (the morphs and the generating animation ride these
            // frames too). No track up: fully parked.
            window.request_animation_frame();
        }

        let body = match (&now, &self.peaks) {
            (None, _) | (Some(_), Peaks::None) => {
                // Nothing on screen to morph from later; snap the strip
                // empty so the next track fades in from blank.
                self.from = Shape::Blank;
                self.to = Shape::Blank;
                div().into_any_element()
            }
            (Some(_), Peaks::Failed) => self
                .message("waveform unavailable for this track")
                .into_any_element(),
            (Some(_), Peaks::Decoding) => {
                self.retarget(Shape::Placeholder);
                self.strip().into_any_element()
            }
            (Some(now), Peaks::Ready(peaks)) => {
                let progress = now
                    .duration_secs
                    .filter(|d| *d > 0.0)
                    .map(|d| (now.position_secs / d) as f32)
                    .unwrap_or(0.0);
                self.retarget(Shape::Peaks(peaks.clone(), progress));
                self.strip().into_any_element()
            }
        };

        div()
            .size_full()
            .bg(palette::bg_root())
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        panel::seek_fraction(&this.state.player, fraction, cx);
                    }
                    cx.notify();
                }),
            )
            .child(body)
    }
}
