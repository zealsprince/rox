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
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, BorderStyle, Bounds, Context,
    Div, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Rgba, SharedString,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_playback::engine;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::peaks;

/// Resolution of the in-memory peaks. The paint resamples these down to
/// however many bars fit the width.
const PEAK_BINS: usize = 2048;

/// The spans the bar sliders pick across, px. Values snap to whole pixels
/// so the bars stay crisp.
const BAR_W_MIN: f32 = 1.0;
const BAR_W_MAX: f32 = 12.0;
const BAR_GAP_MAX: f32 = 8.0;

/// The waveform panel's per-view config: what a saved layout restores, and
/// what the customize window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WaveformConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Bar thickness, px: the sampling step follows it, so thicker bars
    /// mean fewer of them.
    pub bar_width: f32,
    /// Space between bars, px: zero merges them into a solid shape.
    pub bar_gap: f32,
    /// Trace the bars as outlines instead of filling them; with the gap
    /// at zero the strip reads as one outlined shape.
    pub outline: bool,
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for last.fm. Only draws while scrobbling is
    /// connected and on.
    pub scrobble_marker: bool,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        WaveformConfig {
            chrome: PanelChrome::default(),
            bar_width: tokens::BAR_W,
            bar_gap: tokens::BAR_GAP,
            outline: false,
            scrobble_marker: false,
        }
    }
}

impl WaveformConfig {
    /// The bar rhythm, clamped to the slider spans so a hand-edited file
    /// can't collapse the step to nothing.
    fn bars(&self) -> (f32, f32) {
        (
            self.bar_width.clamp(BAR_W_MIN, BAR_W_MAX),
            self.bar_gap.clamp(0.0, BAR_GAP_MAX),
        )
    }
}

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
    config: WaveformConfig,
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
    /// The customize window's slider strips, one per knob so a drag on one
    /// never moves the other.
    bar_w_scrub: ScrubState,
    gap_scrub: ScrubState,
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
    pub fn new(state: AppState, config: WaveformConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        WaveformPanel {
            state,
            config,
            track: None,
            peaks: Peaks::None,
            generation: 0,
            from: Shape::Blank,
            to: Shape::Blank,
            morph_at: Instant::now(),
            scrub: ScrubState::default(),
            bar_w_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
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

    fn set_bar_width(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.bar_width = (BAR_W_MIN + fraction * (BAR_W_MAX - BAR_W_MIN)).round();
        cx.notify();
    }

    fn set_bar_gap(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.bar_gap = (fraction * BAR_GAP_MAX).round();
        cx.notify();
    }

    fn strip(&self, marker: Option<f32>) -> impl IntoElement {
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let from = self.from.clone();
        let to = self.to.clone();
        let u = (self.morph_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let t = self.epoch.elapsed().as_secs_f32();
        let config = self.config.clone();
        canvas(
            {
                let scrub = scrub.clone();
                move |bounds, _, _| scrub.set_bounds(bounds)
            },
            move |bounds, _, window, _| {
                paint_morph(&from, &to, u, t, marker, &config, bounds, window);
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
#[allow(clippy::too_many_arguments)]
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
/// retiring one fading out as the incoming one fades in; the scrobble
/// marker, when asked for, rides the same fade.
#[allow(clippy::too_many_arguments)]
fn paint_morph(
    from: &Shape,
    to: &Shape,
    u: f32,
    t: f32,
    marker: Option<f32>,
    config: &WaveformConfig,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let (bar_w, gap) = config.bars();
    let count = ((w / (bar_w + gap)) as usize).max(1);
    let step = w / count as f32;
    // Bars fill the step minus the gap, so a zero gap tiles them into a
    // solid shape with no seams.
    let draw_w = (step - gap).max(1.0);
    let center = h / 2.0;
    let max_bar = h * 0.46;

    // Smoothstepped so the morph eases out instead of stopping dead.
    let u = u.clamp(0.0, 1.0);
    let u = u * u * (3.0 - 2.0 * u);

    // The silhouette's neighbor edges, for the merged-outline risers.
    let mut prev = (center, center);
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
        let x0 = bounds.origin.x + px(x);
        let bar = Bounds::new(
            point(x0, bounds.origin.y + px(top)),
            size(px(draw_w), px(bottom - top)),
        );
        if !config.outline {
            window.paint_quad(fill(bar, color));
        } else if gap > 0.0 {
            // Separate bars: each its own hollow frame, the spectrum's
            // outline look.
            window.paint_quad(gpui::outline(bar, color, BorderStyle::default()));
        } else {
            // Merged bars: trace the silhouette instead - 1px top and
            // bottom edges plus risers spanning the jump to the neighbor,
            // one continuous outlined shape.
            for y in [top, bottom - 1.0] {
                window.paint_quad(fill(
                    Bounds::new(
                        point(x0, bounds.origin.y + px(y)),
                        size(px(draw_w), px(1.0)),
                    ),
                    color,
                ));
            }
            for (a, b) in [(prev.0, top), (prev.1, bottom)] {
                let rise = (b - a).abs();
                if rise >= 1.0 {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x0, bounds.origin.y + px(a.min(b))),
                            size(px(1.0), px(rise)),
                        ),
                        color,
                    ));
                }
            }
        }
        prev = (top, bottom);
    }

    for (shape, weight) in [(from, 1.0 - u), (to, u)] {
        let Shape::Peaks(_, progress) = shape else {
            continue;
        };
        if let Some(marker) = marker {
            let alpha = (0x80 as f32 * weight) as u8;
            if alpha > 0 {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x + px(marker.clamp(0.0, 1.0) * w),
                            bounds.origin.y,
                        ),
                        size(px(1.0), px(h)),
                    ),
                    palette::alpha(palette::highlight(), alpha),
                ));
            }
        }
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

impl PanelSettings for WaveformPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Display", icons::EYE)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (bar_w, gap) = self.config.bars();
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "Bar Width",
                Some("How thick each bar draws"),
                panel::value_slider(
                    &self.bar_w_scrub,
                    (bar_w - BAR_W_MIN) / (BAR_W_MAX - BAR_W_MIN),
                    format!("{bar_w:.0} px"),
                    Self::set_bar_width,
                    cx,
                ),
            ))
            .child(setting_row(
                "Bar Gap",
                Some("Space between bars, zero merges them into a solid shape"),
                panel::value_slider(
                    &self.gap_scrub,
                    gap / BAR_GAP_MAX,
                    format!("{gap:.0} px"),
                    Self::set_bar_gap,
                    cx,
                ),
            ))
            .child(setting_row(
                "Outline",
                Some("Trace the bars instead of filling them; merged bars read as one shape"),
                toggle(
                    self.config.outline,
                    |this: &mut Self, on, cx| {
                        this.config.outline = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Scrobble Marker",
                Some("A thin line where the track counts as scrobbled to last.fm"),
                toggle(
                    self.config.scrobble_marker,
                    |this: &mut Self, on, cx| {
                        this.config.scrobble_marker = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
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
        panel::title_text(self.config.chrome.title.as_deref(), "Waveform")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
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
        // The config block: the panel's quick entries above the core panel
        // items, like the transport panels'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Scrobble Marker")
                .checked(self.config.scrobble_marker)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.scrobble_marker = !this.config.scrobble_marker;
                        cx.notify();
                    });
                }),
        );
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than shared: the copy takes the
        // config along, like every configured panel's.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| WaveformPanel::new(state, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
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
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl WaveformPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        // A played-out queue counts as nothing playing: the strip clears
        // instead of sitting there fully lit.
        let now = player.now_playing().filter(|_| !player.queue_ended());
        let playing = player.is_playing();
        // The engine's position clock blinks off for a moment between
        // tracks and while a fresh queue opens, with the session very much
        // alive (the backdrop holds through the same blink). Snapping blank
        // here would throw away the shape mid-switch, so the next track
        // could only fade in from nothing.
        let between_tracks = now.is_none() && player.is_active() && !player.queue_ended();

        // Kick a decode when the playing track changes.
        if let Some(now) = &now {
            if self.track.as_deref() != Some(now.path.as_path()) {
                let path = now.path.clone();
                self.start_decode(path, cx);
            }
        }

        // The marker only shows where a scrobble could actually land: the
        // toggle on and the scrobbler armed.
        let marker = (self.config.scrobble_marker)
            .then(|| self.state.scrobbler.read(cx).marker())
            .flatten();

        // The seek preview rides on real peaks only: the placeholder and
        // the unavailable message have no track shape to point along.
        let mut hover_duration: Option<f64> = None;
        let body = match (&now, &self.peaks) {
            // Hold the strip through the blink: whatever it shows stays up,
            // and the next track's shape morphs from it instead of popping
            // in from blank.
            (None, _) if between_tracks => self.strip(marker).into_any_element(),
            (None, _) | (Some(_), Peaks::None) => {
                // Nothing on screen to morph from later; snap the strip
                // empty so the next track fades in from blank.
                self.from = Shape::Blank;
                self.to = Shape::Blank;
                div().into_any_element()
            }
            (Some(_), Peaks::Failed) => self
                .message("Waveform unavailable for this track")
                .into_any_element(),
            (Some(_), Peaks::Decoding) => {
                self.retarget(Shape::Placeholder);
                self.strip(marker).into_any_element()
            }
            (Some(now), Peaks::Ready(peaks)) => {
                let progress = now
                    .duration_secs
                    .filter(|d| *d > 0.0)
                    .map(|d| (now.position_secs / d) as f32)
                    .unwrap_or(0.0);
                hover_duration = now.duration_secs.filter(|d| *d > 0.0);
                self.retarget(Shape::Peaks(peaks.clone(), progress));
                self.strip(marker).into_any_element()
            }
        };

        // While playing, the direct observe re-renders the strip on every
        // pump tick - the rate the playhead actually moves at, so frame
        // polling on top only redraws identical pixels. Frames are for the
        // windows the pump does not notify through: the morph, the
        // generating stand-in, and the between-tracks blink (pause and
        // skips do not notify on their own, so those windows carry the
        // transitions). A paused strip with a settled shape parks; the
        // pump's play-state notify wakes it on resume.
        let morphing = self.morph_at.elapsed().as_secs_f32() < tokens::EASE_SECS;
        let generating = matches!(self.to, Shape::Placeholder);
        if !playing && (between_tracks || morphing || generating) {
            window.request_animation_frame();
        }

        div()
            .size_full()
            .bg(palette::bg_root())
            .relative()
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
            .when_some(hover_duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            })
    }
}
