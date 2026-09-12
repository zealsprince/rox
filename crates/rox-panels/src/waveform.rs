//! The waveform panel: the whole track's amplitude shape as mirrored bars
//! around a center line, played bars in the accent, the rest as a dim ghost,
//! with a playhead tracking the position clock. Click or drag the strip to
//! seek. A loudness toggle adds a second layer inside the envelope, a
//! flatter band at the bin's RMS, the pair that makes a quiet passage read
//! as quiet rather than just narrow. A color source picks what the layers
//! are tinted from: the
//! accent, the theme ramp, the cover art, or a custom pair, the same ramp
//! the spectrum and VU panels share. A split toggle stacks one row per
//! channel instead of the mono mix, the way Foobar draws it. Peaks come
//! from the disk cache
//! ([`crate::peaks`]) when the track
//! has played before, otherwise from a full decode on a background thread
//! that then fills the cache; while a decode runs the strip shows a gray
//! pulsing stand-in shape. Every change of what the strip shows (stand-in
//! to peaks, one track's peaks to the next, blank to anything) is a short
//! morph in geometry and color, never a pop. Painting is a row of quads;
//! with no track up (idle, or the queue played out) the panel is blank and
//! completely still.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, BorderStyle, Bounds, Context,
    Div, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Rgba, SharedString,
    Subscription, WeakEntity, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::bookmarks::Bookmark;
use rox_library::cue::TrackKey;
use rox_library::peaks::{PeakBin, PeakLanes};
use serde::{Deserialize, Serialize};

use rox_playback::engine;

use crate::assets::icons;
use crate::bookmark_ui;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{
    self, choices_shared, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::peaks;
use crate::settings::ui as settings_ui;
use crate::spectrum::{gradient_choices, ramp_color, Gradient};

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
    /// Draw the bin's RMS as a flatter band inside the peak envelope, so a
    /// passage that's quiet but spiky reads differently from a loud one.
    /// Off by default, leaving the envelope alone the way the strip always
    /// drew.
    pub loudness: bool,
    /// Where the envelope and the band take their colors: flat accent, the
    /// theme ramp, the cover art's pair, or a custom one. The envelope
    /// sits at the bottom of the ramp, the band at the top.
    pub gradient: Gradient,
    /// The custom ramp's low end as hex, the envelope's color when the
    /// source is custom.
    pub gradient_lo: String,
    /// The custom ramp's high end as hex, the band's color when the source
    /// is custom.
    pub gradient_hi: String,
    /// Stack one row per channel instead of the mono mix, left above
    /// right. Mono tracks stay a single row either way.
    pub split_channels: bool,
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for Last.fm. Only draws while scrobbling is
    /// connected and on.
    pub scrobble_marker: bool,
    /// The playing track's bookmarks as chevrons along the bottom edge,
    /// each one a seek and a right-click menu, the seek strip's own.
    pub bookmarks: bool,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        WaveformConfig {
            chrome: PanelChrome::default(),
            bar_width: tokens::BAR_W,
            bar_gap: tokens::BAR_GAP,
            outline: false,
            loudness: false,
            gradient: Gradient::default(),
            gradient_lo: "#22aa44".into(),
            gradient_hi: "#dd3322".into(),
            split_channels: false,
            scrobble_marker: false,
            bookmarks: true,
        }
    }
}

impl WaveformConfig {
    /// The bar rhythm, clamped to the knobs' range, typed
    /// values past the strip included, so a hand-edited file can't
    /// collapse the step to nothing.
    fn bars(&self) -> (f32, f32) {
        (
            self.bar_width
                .clamp(BAR_W_MIN, settings_ui::ceiling(BAR_W_MIN, BAR_W_MAX)),
            self.bar_gap
                .clamp(0.0, settings_ui::ceiling(0., BAR_GAP_MAX)),
        )
    }

    /// The custom ramp's ends parsed, falling back to the theme ramp's when a
    /// hand-edited hex doesn't parse, the same fallback the spectrum uses.
    fn custom_ramp(&self) -> (Rgba, Rgba) {
        (
            palette::parse_hex(&self.gradient_lo)
                .unwrap_or_else(|| palette::alpha(palette::text_faint(), 0x66)),
            palette::parse_hex(&self.gradient_hi).unwrap_or_else(palette::accent),
        )
    }
}

/// The shortest a bar draws, so quiet passages stay visible.
const MIN_BAR: f32 = 2.0;

enum Peaks {
    /// No track has been seen yet.
    None,
    Decoding,
    Ready(Arc<PeakLanes>),
    Failed,
}

/// The lanes a peak set draws: the per-channel lanes when the split is on
/// and the set has them, the mono mix otherwise.
fn display_lanes(set: &[Vec<PeakBin>], split: bool) -> &[Vec<PeakBin>] {
    if split && set.len() > 1 {
        &set[1..]
    } else {
        &set[..set.len().min(1)]
    }
}

/// One thing the strip can show. The morph runs between two of these,
/// sampled per display bar at paint time.
#[derive(Clone)]
enum Shape {
    /// Zero-height bars: what everything fades in from and out to.
    Blank,
    /// The gray generating stand-in, animated off the panel's clock.
    Placeholder,
    /// A track's decoded lanes, whether they draw split, and the playhead
    /// position: live while the shape is the target, frozen where it last
    /// painted once retired.
    Peaks(Arc<PeakLanes>, bool, f32),
}

impl Shape {
    /// Same visual target: the playhead moving or the stand-in animating
    /// doesn't count, a different peaks buffer or a flipped split does.
    fn same(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Blank, Shape::Blank) | (Shape::Placeholder, Shape::Placeholder) => true,
            (Shape::Peaks(a, sa, _), Shape::Peaks(b, sb, _)) => Arc::ptr_eq(a, b) && sa == sb,
            _ => false,
        }
    }

    /// How many rows this shape needs, or None where it adapts to
    /// whatever layout the other shape sets (blank and the stand-in).
    fn lanes(&self) -> Option<usize> {
        match self {
            Shape::Peaks(set, split, _) => Some(display_lanes(set, *split).len().max(1)),
            _ => None,
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
    /// The custom ramp's two color pickers, built the first time the
    /// settings page shows them, and the subscriptions writing their edits
    /// back into the config.
    ramp_pickers: Option<[Entity<ColorPickerState>; 2]>,
    _ramp_changes: Vec<Subscription>,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// Time zero for the generating animation's phase.
    epoch: Instant,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The playing track's bookmarks and which track they were read for,
    /// re-read on a track change and on a bookmark edit rather than on
    /// every tick the strip repaints on.
    marks: Vec<Bookmark>,
    marks_key: Option<TrackKey>,
    /// The bookmark chevron the pointer is on, for its readout.
    hover_mark: Option<i64>,
    /// Wakes the panel when a session starts, so an idle window notices the
    /// new track without the player bar's frame pump.
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl WaveformPanel {
    pub fn new(state: AppState, config: WaveformConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(
                    event,
                    LibraryEvent::BookmarksChanged | LibraryEvent::Updated
                ) {
                    this.marks_key = None;
                    cx.notify();
                }
            },
        );
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
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            value_edit: panel::ValueEdit::default(),
            epoch: Instant::now(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            marks: Vec::new(),
            marks_key: None,
            hover_mark: None,
            _player_changed,
            _library_changed,
        }
    }

    /// The playing track's bookmarks, read once per track (and again after
    /// an edit), so the per-tick repaint never touches the database.
    fn marks_for(&mut self, key: &TrackKey, cx: &App) -> &[Bookmark] {
        if self.marks_key.as_ref() != Some(key) {
            self.marks = self.state.library.read(cx).bookmarks_for(key);
            self.marks_key = Some(key.clone());
            self.hover_mark = None;
        }
        &self.marks
    }

    /// The playing track changed: fetch its peaks off the UI thread (the
    /// disk cache when it holds the track, a full decode that then fills
    /// the cache otherwise) and swap them in when done.
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
                    // Stamp the file before the decode reads it, so a track
                    // still being written to disk keys the entry it had,
                    // not the one it finishes as.
                    let stamp = peaks::identity(&path);
                    let decoded = engine::decode_peaks(&path, PEAK_BINS)?;
                    peaks::store(&path, stamp, &decoded);
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
                        log::warn!("waveform decode failed: {e}");
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
    /// source, so an intermediate that barely painted (the stand-in when a
    /// cache hit arrives a frame after a track switch) never flashes, and
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

    fn set_bar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.config.bar_width = width;
        cx.notify();
    }

    fn set_bar_gap(&mut self, gap: f32, cx: &mut Context<Self>) {
        self.config.bar_gap = gap;
        cx.notify();
    }

    fn strip(
        &self,
        marker: Option<f32>,
        ab: Option<(f32, Option<f32>)>,
        marks: Vec<bookmark_ui::Mark>,
    ) -> impl IntoElement {
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
                paint_morph(
                    &from, &to, u, t, marker, ab, &marks, &config, bounds, window,
                );
                panel::scrub_on_paint(&scrub, window, {
                    let player = player.clone();
                    move |fraction, cx| panel::seek_fraction(&player, fraction, cx)
                });
            },
        )
        .size_full()
    }

    fn message(&self, text: impl Into<SharedString>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(palette::text_muted())
            .child(text.into())
    }
}

/// The gray the generating stand-in draws in, kept away from the accent so
/// it can't be mistaken for real peaks.
fn placeholder_tint() -> Rgba {
    palette::alpha(palette::text_muted(), 0x33)
}

/// The stand-in's mirrored half-height for bar `i` of `count` in `lane` at
/// time `t`: a stable pseudo-random profile per slot so the strip reads as
/// audio, swelling under two pulse crests that travel left to right. Each
/// lane gets its own profile, the crests travel in step across them.
fn placeholder_bar(i: usize, lane: usize, count: usize, t: f32, max_bar: f32) -> f32 {
    // The classic one-liner hash: a fixed jagged profile per slot.
    let seed = (((i + lane * count) as f32 * 12.9898).sin() * 43758.547)
        .fract()
        .abs();
    let phase = i as f32 / count as f32 * std::f32::consts::TAU * 2.0 - t * 4.0;
    let pulse = phase.sin() * 0.5 + 0.5;
    ((0.2 + 0.8 * seed) * (0.25 + 0.75 * pulse) * max_bar).max(MIN_BAR / 2.0)
}

/// A lane's bins folded over display bar `i` of `count`, so transients
/// aren't lost in the downsample: the extremes reach as far as any bin in
/// the bucket did, the RMS is the quadratic mean across them, which is what
/// the bucket's frames would have given had they been measured in one go.
/// None for a lane with no bins to bucket.
fn bucket(lane: &[PeakBin], i: usize, count: usize) -> Option<PeakBin> {
    if lane.is_empty() {
        return None;
    }
    let per = lane.len() as f32 / count as f32;
    let from = (i as f32 * per) as usize;
    let to = (((i + 1) as f32 * per) as usize).clamp(from + 1, lane.len());
    let bins = &lane[from..to];
    let folded = bins.iter().fold(PeakBin::default(), |acc, bin| PeakBin {
        lo: acc.lo.min(bin.lo),
        hi: acc.hi.max(bin.hi),
        rms: acc.rms + bin.rms * bin.rms,
    });
    Some(PeakBin {
        rms: (folded.rms / bins.len() as f32).sqrt(),
        ..folded
    })
}

/// One display bar, both layers: the envelope's top and bottom in
/// strip-local y, the loudness band's inside them, and the color each layer
/// draws in. The morph blends two of these field by field.
#[derive(Clone, Copy)]
struct Bar {
    top: f32,
    bottom: f32,
    band_top: f32,
    band_bottom: f32,
    envelope: Rgba,
    band: Rgba,
}

impl Bar {
    /// A bar `u` of the way from `self` to `other`, geometry and color both.
    fn mix(&self, other: &Bar, u: f32) -> Bar {
        let lerp = |a: f32, b: f32| a + (b - a) * u;
        Bar {
            top: lerp(self.top, other.top),
            bottom: lerp(self.bottom, other.bottom),
            band_top: lerp(self.band_top, other.band_top),
            band_bottom: lerp(self.band_bottom, other.band_bottom),
            envelope: palette::mix(self.envelope, other.envelope, u),
            band: palette::mix(self.band, other.band, u),
        }
    }

    /// Both layers collapsed to the center line in `color`: what a lane
    /// with nothing to draw contributes, and what a morph fades in from.
    fn flat(center: f32, color: Rgba) -> Bar {
        Bar {
            top: center,
            bottom: center,
            band_top: center,
            band_bottom: center,
            envelope: color,
            band: color,
        }
    }
}

/// What the two layers are tinted with: the envelope at the bottom of the
/// ramp, the band at the top. Flat mode keeps the strip's old look, a
/// half-lit envelope under a full-strength band.
fn layer_colors(config: &WaveformConfig) -> (Rgba, Rgba) {
    match config.gradient {
        Gradient::Off => (palette::alpha(palette::accent(), 0x80), palette::accent()),
        gradient => {
            let custom = config.custom_ramp();
            (
                ramp_color(gradient, 0.0, custom),
                ramp_color(gradient, 1.0, custom),
            )
        }
    }
}

/// A layer color's unplayed ghost. The old strip dimmed the accent from
/// 0xff to 0x33, so this scales the alpha it already has by that ratio
/// instead of setting one, which a ramp color's own alpha would lose.
fn ghost(color: Rgba) -> Rgba {
    Rgba {
        a: color.a * 0.2,
        ..color
    }
}

/// A shape's bar `i` of `count` in display lane `lane` of `lanes`: both
/// layers' extents in strip-local y and their colors. `center` and
/// `max_bar` are the display lane's geometry; a shape whose own lane layout
/// differs maps into it: a single lane fills every row, a wider set folds
/// together. `x_mid` and `w` place the bar against the shape's playhead for
/// the played/ghost split. `layers` is the pair [`layer_colors`] resolved
/// for the config, already accounting for the band being off.
#[allow(clippy::too_many_arguments)]
fn sample(
    shape: &Shape,
    lane: usize,
    lanes: usize,
    i: usize,
    count: usize,
    x_mid: f32,
    w: f32,
    t: f32,
    center: f32,
    max_bar: f32,
    layers: (Rgba, Rgba),
) -> Bar {
    match shape {
        Shape::Blank => Bar::flat(center, palette::alpha(palette::text_muted(), 0)),
        Shape::Placeholder => {
            let bar = placeholder_bar(i, lane, count, t, max_bar);
            // The stand-in's band is a fixed share of its bar: enough to
            // read as two layers without pretending to a loudness it has
            // no track to take one from.
            let band = bar * 0.45;
            Bar {
                top: center - bar,
                bottom: center + bar,
                band_top: center - band,
                band_bottom: center + band,
                envelope: placeholder_tint(),
                band: placeholder_tint(),
            }
        }
        Shape::Peaks(set, split, progress) => {
            let data = display_lanes(set, *split);
            let extremes = match data.len() {
                0 => None,
                1 => bucket(&data[0], i, count),
                n if n == lanes => bucket(&data[lane], i, count),
                // This shape's layout differs from the display's (a morph
                // across a split flip or a channel-count change): fold its
                // lanes into one silhouette for every row.
                _ => data
                    .iter()
                    .filter_map(|lane| bucket(lane, i, count))
                    .reduce(|a, b| PeakBin {
                        lo: a.lo.min(b.lo),
                        hi: a.hi.max(b.hi),
                        rms: a.rms.max(b.rms),
                    }),
            };
            let Some(bin) = extremes else {
                return Bar::flat(center, palette::alpha(palette::accent(), 0));
            };
            let top = center - (bin.hi * max_bar).max(MIN_BAR / 2.0);
            let bottom = center - (bin.lo * max_bar).min(-MIN_BAR / 2.0);
            // The band gets no minimum of its own: silence leaves the
            // envelope's stub alone rather than laying a second stub over
            // it. Clamped into the envelope, which the normalization
            // already guarantees but the bar floors can undercut.
            let band = bin.rms * max_bar;
            let played = x_mid <= progress.clamp(0.0, 1.0) * w;
            let (envelope, band_color) = if played {
                layers
            } else {
                (ghost(layers.0), ghost(layers.1))
            };
            Bar {
                top,
                bottom,
                band_top: (center - band).max(top),
                band_bottom: (center + band).min(bottom),
                envelope,
                band: band_color,
            }
        }
    }
}

/// The strip: `to`'s bars, blended per bar from wherever `from` had them
/// while the morph runs, geometry and color both, so shape changes flow
/// instead of popping. Split shapes repeat the same blend per row, the
/// lane layout following the incoming shape. Each shape that has a
/// playhead draws it, the retiring one fading out as the incoming one
/// fades in; the scrobble marker, when it's on, and the A-B section use
/// the same fade.
#[allow(clippy::too_many_arguments)]
fn paint_morph(
    from: &Shape,
    to: &Shape,
    u: f32,
    t: f32,
    marker: Option<f32>,
    ab: Option<(f32, Option<f32>)>,
    marks: &[bookmark_ui::Mark],
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

    // The row layout: the incoming shape's when it sets one, the retiring
    // one's through a fade to blank, one row when neither does.
    let lanes = to.lanes().or(from.lanes()).unwrap_or(1);
    let lane_h = h / lanes as f32;

    // Smoothstepped so the morph eases out instead of stopping dead.
    let u = u.clamp(0.0, 1.0);
    let u = u * u * (3.0 - 2.0 * u);

    // With the band off the envelope is the only layer left, so it takes
    // the band's full-strength color and the strip paints exactly what it
    // did before the band existed.
    let (envelope, band) = layer_colors(config);
    let layers = if config.loudness {
        (envelope, band)
    } else {
        (band, band)
    };

    for lane in 0..lanes {
        let center = lane_h * lane as f32 + lane_h / 2.0;
        let max_bar = lane_h * 0.46;
        // The silhouette's neighbor edges, for the merged-outline risers.
        let mut prev = (center, center);
        for i in 0..count {
            let x = i as f32 * step;
            let x_mid = x + step * 0.5;
            let sampled = {
                let b = sample(
                    to, lane, lanes, i, count, x_mid, w, t, center, max_bar, layers,
                );
                if u < 1.0 {
                    let a = sample(
                        from, lane, lanes, i, count, x_mid, w, t, center, max_bar, layers,
                    );
                    a.mix(&b, u)
                } else {
                    b
                }
            };
            let (top, bottom) = (sampled.top, sampled.bottom);
            let color = sampled.envelope;
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
                // Merged bars: trace the silhouette instead, with 1px top and
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
            // The band goes over the envelope, and stays a filled quad even
            // in outline mode: an outlined band inside an outlined envelope
            // reads as noise at these heights.
            if config.loudness {
                let band_h = sampled.band_bottom - sampled.band_top;
                if band_h > 0.0 {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x0, bounds.origin.y + px(sampled.band_top)),
                            size(px(draw_w), px(band_h)),
                        ),
                        sampled.band,
                    ));
                }
            }
            prev = (top, bottom);
        }
    }

    for (shape, weight) in [(from, 1.0 - u), (to, u)] {
        let Shape::Peaks(_, _, progress) = shape else {
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
        panel::paint_ab(ab, weight, bounds, window);
        bookmark_ui::paint_marks(marks, weight, bounds, window);
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
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The custom ramp's pickers on first need; each edit writes its hex
        // back into the config, the format the layout dump stores.
        if self.config.gradient == Gradient::Custom && self.ramp_pickers.is_none() {
            let (lo, hi) = self.config.custom_ramp();
            let mut build = |seed: Rgba, write: fn(&mut Self, Rgba)| {
                let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(seed));
                let sub = cx.subscribe_in(
                    &picker,
                    window,
                    move |this, _, event: &ColorPickerEvent, _, cx| {
                        let ColorPickerEvent::Change(color) = event;
                        if let Some(color) = color {
                            write(this, Rgba::from(*color));
                            cx.notify();
                        }
                    },
                );
                self._ramp_changes.push(sub);
                picker
            };
            let lo = build(lo, |this, c| this.config.gradient_lo = palette::to_hex(c));
            let hi = build(hi, |this, c| this.config.gradient_hi = palette::to_hex(c));
            self.ramp_pickers = Some([lo, hi]);
        }
        let (bar_w, gap) = self.config.bars();
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("waveform-bar-width"),
                Some(rox_i18n::t!("waveform-bar-width.description")),
                settings_ui::scalar(
                    &self.bar_w_scrub,
                    &self.value_edit,
                    bar_w,
                    settings_ui::span(BAR_W_MIN, BAR_W_MAX, " px"),
                    Self::set_bar_width,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("waveform-bar-gap"),
                Some(rox_i18n::t!("waveform-bar-gap.description")),
                settings_ui::scalar(
                    &self.gap_scrub,
                    &self.value_edit,
                    gap,
                    settings_ui::span(0., BAR_GAP_MAX, " px"),
                    Self::set_bar_gap,
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("waveform-outline"),
                Some(rox_i18n::t!("waveform-outline.description")),
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
                rox_i18n::t!("waveform-loudness"),
                Some(rox_i18n::t!("waveform-loudness.description")),
                toggle(
                    self.config.loudness,
                    |this: &mut Self, on, cx| {
                        this.config.loudness = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("waveform-gradient-mode"),
                Some(rox_i18n::t!("waveform-gradient-mode.description")),
                choices_shared(
                    &gradient_choices(),
                    self.config.gradient,
                    |this: &mut Self, gradient, cx| {
                        this.config.gradient = gradient;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when_some(
                (self.config.gradient == Gradient::Custom)
                    .then(|| self.ramp_pickers.clone())
                    .flatten(),
                |d, [lo, hi]| {
                    d.child(setting_row(
                        rox_i18n::t!("spectrum-gradient-base-color"),
                        Some(rox_i18n::t!("spectrum-gradient-base-color.description")),
                        ColorPicker::new(&lo).small(),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("spectrum-gradient-tip-color"),
                        Some(rox_i18n::t!("spectrum-gradient-tip-color.description")),
                        ColorPicker::new(&hi).small(),
                    ))
                },
            )
            .child(setting_row(
                rox_i18n::t!("waveform-split-channels"),
                Some(rox_i18n::t!("waveform-split-channels.description")),
                toggle(
                    self.config.split_channels,
                    |this: &mut Self, on, cx| {
                        this.config.split_channels = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("waveform-scrobble-marker"),
                Some(rox_i18n::t!("waveform-scrobble-marker.description")),
                toggle(
                    self.config.scrobble_marker,
                    |this: &mut Self, on, cx| {
                        this.config.scrobble_marker = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("waveform-bookmarks"),
                Some(rox_i18n::t!("waveform-bookmarks.description")),
                toggle(
                    self.config.bookmarks,
                    |this: &mut Self, on, cx| {
                        this.config.bookmarks = on;
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-waveform"),
        )
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

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries above the core panel
        // items, like the transport panels'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("waveform-split-channels"))
                .checked(self.config.split_channels)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.split_channels = !this.config.split_channels;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("waveform-scrobble-marker"))
                .checked(self.config.scrobble_marker)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.scrobble_marker = !this.config.scrobble_marker;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("waveform-bookmarks"))
                .checked(self.config.bookmarks)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.bookmarks = !this.config.bookmarks;
                        cx.notify();
                    });
                }),
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                WaveformPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for WaveformPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl WaveformPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        // A played-out queue counts as nothing playing: the strip clears
        // instead of staying there fully lit.
        let now = player.now_playing().filter(|_| !player.queue_ended());
        let playing = player.is_playing();
        let ab_state = player.ab_state();
        // The engine's position clock blinks off for a moment between
        // tracks and while a fresh queue opens, with the session very much
        // alive (the backdrop holds through the same blink). Snapping blank
        // here would throw away the shape mid-switch, so the next track
        // could only fade in from nothing.
        let between_tracks = now.is_none() && player.is_active() && !player.queue_ended();

        // Kick a decode when the playing track changes.
        if let Some(now) = &now {
            // Keyed on the file: the strip draws the whole image's shape,
            // and a cue rip's tracks are all inside one.
            if self.track.as_deref() != Some(now.path()) {
                let path = now.path().to_path_buf();
                self.start_decode(path, cx);
            }
        }

        // The marker only shows where a scrobble could actually happen: the
        // toggle on and some destination armed.
        let marker = (self.config.scrobble_marker)
            .then(|| self.state.scrobble_marker(cx))
            .flatten();
        // The A-B section, or the lone A while the cycle waits for B.
        let ab = now
            .as_ref()
            .and_then(|now| panel::ab_fractions(ab_state, now.duration_secs));
        // The track's bookmarks, placed along the strip. Kept through the
        // between-tracks blink so the marks don't flash off with the shape.
        let marks = match (&now, self.config.bookmarks) {
            (Some(now), true) => {
                let key = now.key.clone();
                let duration = now.duration_secs;
                bookmark_ui::marks(self.marks_for(&key, cx), duration)
            }
            _ => Vec::new(),
        };
        let hover_mark = self.hover_mark;

        // The seek preview only shows on real peaks: the placeholder and
        // the unavailable message have no track shape to point along.
        let mut hover_duration: Option<f64> = None;
        let body = match (&now, &self.peaks) {
            // Hold the strip through the blink: whatever it shows stays up,
            // and the next track's shape morphs from it instead of popping
            // in from blank.
            (None, _) if between_tracks => self.strip(marker, ab, marks.clone()).into_any_element(),
            (None, _) | (Some(_), Peaks::None) => {
                // Nothing on screen to morph from later; snap the strip
                // empty so the next track fades in from blank.
                self.from = Shape::Blank;
                self.to = Shape::Blank;
                div().into_any_element()
            }
            (Some(_), Peaks::Failed) => {
                // Drop the placeholder the decode left behind. The message
                // replaces the strip, so the shape is unused, but a lingering
                // Placeholder keeps `generating` true and spins animation
                // frames at refresh rate while paused on a failed decode.
                self.from = Shape::Blank;
                self.to = Shape::Blank;
                self.message(rox_i18n::t!("waveform-unavailable"))
                    .into_any_element()
            }
            (Some(_), Peaks::Decoding) => {
                self.retarget(Shape::Placeholder);
                self.strip(marker, ab, marks.clone()).into_any_element()
            }
            (Some(now), Peaks::Ready(peaks)) => {
                let progress = now
                    .duration_secs
                    .filter(|d| *d > 0.0)
                    .map(|d| (now.position_secs / d) as f32)
                    .unwrap_or(0.0);
                hover_duration = now.duration_secs.filter(|d| *d > 0.0);
                self.retarget(Shape::Peaks(
                    peaks.clone(),
                    self.config.split_channels,
                    progress,
                ));
                self.strip(marker, ab, marks.clone()).into_any_element()
            }
        };

        // While playing, the direct observe re-renders the strip on every
        // pump tick, the rate the playhead actually moves at, so frame
        // polling on top only redraws identical pixels. Frames are for the
        // windows the pump doesn't notify through: the morph, the
        // generating stand-in, and the between-tracks blink (pause and
        // skips don't notify on their own, so those windows cover the
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
            // The chevrons' hit layer goes over the seek readout's, so a
            // pointer on a mark reads the mark. Only over real peaks, where
            // the seek readout shows too: the placeholder has no length.
            .when_some(
                now.as_ref()
                    .filter(|_| hover_duration.is_some() && !marks.is_empty()),
                |d, now| {
                    d.child(bookmark_ui::overlay(
                        &self.state,
                        &now.key,
                        &marks,
                        hover_mark,
                        &self.scrub,
                        |this: &mut Self, id, _| this.hover_mark = id,
                        cx,
                    ))
                },
            )
    }
}
