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
//!
//! A station has no file to decode and no length to draw one against, so
//! the strip switches to the other thing a waveform can be: the audio tap
//! itself, the last few seconds of it rolling right to left, in the same
//! bars and colors the decoded shape uses. The corner carries the stream's
//! own state, matching the seek strip's mark.
//!
//! The live mode is where this panel comes nearest the spectrogram. This
//! strip draws amplitude over the last few seconds, one mirrored bar per
//! slice of the tap; the spectrogram draws frequency over time, a column
//! of heatmap per slice. Both scroll right to left. They answer different
//! questions: how loud the last moment was, against where in the spectrum
//! it sat. Window length is the trace's only knob, and it's the speed
//! control too. The columns always fill the strip, so a longer window
//! holds more of the broadcast and crawls, a shorter one holds a phrase
//! and races.
//!
//! What the strip does over a station is a choice of three. Off leaves
//! the corner mark alone on the panel and asks for no frames, for anyone
//! who wants the strip quiet while the radio is on. Trace is the rolling
//! oscilloscope above. Motion is a shape the strip draws itself, out of
//! an expression in `x` across the strip and `t` off the panel's clock,
//! which is what a waveform panel can honestly show when the thing
//! playing has no waveform to show. It's slow smooth waves by default,
//! and whatever the expression field will take after that.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Instant;

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, MouseButton, Pixels, Rgba, SharedString, Subscription, WeakEntity, Window, canvas,
    div, fill, point, prelude::*, px, size,
};
use gpui_component::Sizable as _;
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::bookmarks::Bookmark;
use rox_library::cue::TrackKey;
use rox_library::peaks::{PeakBin, PeakLanes};
use rox_panel_api::{cue_ui, position_bound};
use rox_panel_kit::expr::Expr;
use rox_services::cues::{Cue, CuesChanged};
use serde::{Deserialize, Serialize};

use rox_playback::{LiveGap, StreamState, engine};
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::bookmark_ui;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, choices_shared, setting_row, toggle,
};
use crate::panel_settings;
use crate::peaks;
use crate::settings::ui as settings_ui;
use crate::spectrum::{Gradient, gradient_choices, ramp_color};
use crate::transport::seek::{self, live_tint};

/// Resolution of the in-memory peaks. The paint resamples these down to
/// however many bars fit the width.
const PEAK_BINS: usize = 2048;

/// How much of the tap the live trace holds, seconds: the default and the
/// span the slider picks across. Eight seconds is a couple of phrases,
/// long enough that the shape reads as the broadcast rather than as a
/// twitching meter, short enough that what's on the left is still what you
/// just heard. The floor is where the strip turns into an oscilloscope
/// with a long memory; past the ceiling a single hit stops being visible
/// in the bar it lands in.
const LIVE_SECS_DEFAULT: f32 = 8.0;
const LIVE_SECS_MIN: f32 = 2.0;
const LIVE_SECS_MAX: f32 = 30.0;

/// How many columns the trace holds before the strip has painted once and
/// said how many bars it draws. From then on the trace is cut to that bar
/// count exactly, one column per bar: folding a fixed column count down
/// to whatever bars fit put the bucket edges on different columns every
/// time the trace moved a step, and a bar changed shape as it slid left.
const LIVE_COLS: usize = 256;

/// The shape a strip with no expression of its own draws: two sines at
/// different rates running against each other, one a little over a cycle
/// across the strip and the other two, both drifting slowly. Slow enough
/// that it reads as motion rather than as a meter, and small enough at
/// 0.75 peak that it never touches the edge.
const LIVE_MOTION_DEFAULT: &str = "0.5 * sin(6.28 * x - 1.2 * t) + 0.25 * sin(12.6 * x + 0.7 * t)";

/// What the strip does while a station plays. A stream has no shape to
/// decode, so the panel has to be told which of the three things it can
/// honestly draw instead is wanted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveMode {
    /// Nothing but the corner mark. No trace, no shape, no frames.
    Off,
    /// The rolling oscilloscope off the audio tap.
    Trace,
    /// A shape drawn from the config's expression.
    #[default]
    Motion,
}

impl<'de> Deserialize<'de> for LiveMode {
    /// A hand-edited layout is where a name that isn't one of these
    /// arrives. Taking the default for it keeps the rest of the config
    /// loading, where failing would drop every other knob on the panel
    /// back to stock over one typo.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Name;

        impl serde::de::Visitor<'_> for Name {
            type Value = LiveMode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("off, trace, or motion")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<LiveMode, E> {
                Ok(match value {
                    "off" => LiveMode::Off,
                    "trace" => LiveMode::Trace,
                    _ => LiveMode::default(),
                })
            }
        }

        deserializer.deserialize_str(Name)
    }
}

fn live_mode_choices() -> [(SharedString, LiveMode); 3] {
    [
        (rox_i18n::t!("panel-size-off"), LiveMode::Off),
        (rox_i18n::t!("waveform-live-trace"), LiveMode::Trace),
        (rox_i18n::t!("waveform-live-motion"), LiveMode::Motion),
    ]
}

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
    /// How many seconds of the tap the live trace spans. The column count
    /// is fixed, so this is the scroll speed as much as the window: a
    /// wider one holds more of the broadcast and crawls.
    pub live_secs: f32,
    /// What the strip does over a station: nothing, the trace, or the
    /// drawn shape.
    pub live: LiveMode,
    /// The expression the drawn shape comes out of, evaluated per bar
    /// with `x` across the strip and `t` off the panel's clock. Stored as
    /// typed rather than as a parse, so a layout round-trips what's in
    /// the field and the settings row can complain about it in place.
    pub live_motion: String,
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
            live_secs: LIVE_SECS_DEFAULT,
            live: LiveMode::default(),
            live_motion: LIVE_MOTION_DEFAULT.into(),
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

    /// The live trace's window, seconds, with junk out of a hand-edited
    /// layout falling back to the default rather than through. A NaN casts
    /// to zero frames a column, and the floor under that would finish a
    /// column on every sample: the scroll would be a blur.
    fn live_secs(&self) -> f32 {
        if self.live_secs.is_nan() {
            LIVE_SECS_DEFAULT
        } else {
            self.live_secs.clamp(LIVE_SECS_MIN, LIVE_SECS_MAX)
        }
    }

    /// The motion expression as typed, with an empty field reading as
    /// "never set" rather than as a shape of nothing: clearing the box
    /// puts the default back instead of flattening the strip.
    fn live_motion(&self) -> &str {
        let typed = self.live_motion.trim();
        if typed.is_empty() {
            LIVE_MOTION_DEFAULT
        } else {
            typed
        }
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

/// The motion expression compiled, kept beside the text it came from so
/// an edit is caught by a string compare instead of by parsing on every
/// frame. Paint reads the parse and never makes one.
struct Motion {
    source: String,
    expr: Arc<Expr>,
    /// What the config's own text did wrong, when it didn't take. The
    /// parse above is the default shape in that case, so the strip keeps
    /// drawing something and the settings row carries the complaint,
    /// rather than the panel going blank over a typo.
    error: Option<String>,
}

impl Motion {
    fn compile(source: &str) -> Motion {
        let (expr, error) = match Expr::parse(source) {
            Ok(expr) => (expr, None),
            Err(e) => (
                Expr::parse(LIVE_MOTION_DEFAULT).expect("the default expression parses"),
                Some(e.to_string()),
            ),
        };

        Motion {
            source: source.to_string(),
            expr: Arc::new(expr),
            error,
        }
    }

    /// Catch up to the config when its text has moved: an edit in the
    /// settings field, or a whole config swapped in by a preset.
    fn sync(&mut self, source: &str) {
        if self.source != source {
            *self = Motion::compile(source);
        }
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
    /// The rolling trace of a stream: one column per slice of the tap,
    /// oldest at the left. Every column is played, so there's no ghost
    /// half and no playhead; the shape simply moves.
    Live(Arc<Vec<PeakBin>>),
    /// A stream drawn from an expression instead of from its audio: the
    /// bar's own position and the panel's clock go in, a height comes
    /// out. Nothing is sampled and nothing is stored, so this shape is
    /// the same picture at every width. The clock rides along the way the
    /// playhead does on peaks, so a paused station holds its frame.
    Motion(Arc<Expr>, f32),
}

impl Shape {
    /// Same visual target: the playhead moving or the stand-in animating
    /// doesn't count, a different peaks buffer or a flipped split does.
    fn same(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Blank, Shape::Blank) | (Shape::Placeholder, Shape::Placeholder) => true,
            (Shape::Peaks(a, sa, _), Shape::Peaks(b, sb, _)) => Arc::ptr_eq(a, b) && sa == sb,
            // The trace refreshes in place the way the playhead does: it's
            // the same picture every frame, moving. Morphing column by
            // column into its own next frame would fight the scroll. The
            // drawn shape is the same case: it moves because `t` moved,
            // and an edit to the expression is the field's business, not
            // a shape change to ease through.
            (Shape::Live(_), Shape::Live(_)) | (Shape::Motion(..), Shape::Motion(..)) => true,
            _ => false,
        }
    }

    /// How many rows this shape needs, or None where it adapts to
    /// whatever layout the other shape sets (blank and the stand-in).
    fn lanes(&self) -> Option<usize> {
        match self {
            Shape::Peaks(set, split, _) => Some(display_lanes(set, *split).len().max(1)),
            // One row whatever the split says: the tap is mixed to mono on
            // the way in, so there are no channels to stack, and the drawn
            // shape has no channels at all.
            Shape::Live(_) | Shape::Motion(..) => Some(1),
            _ => None,
        }
    }
}

/// How many frames go into one column of a trace spanning `secs` at
/// `rate` across `cols` columns: the window cut into as many slices as
/// the strip draws bars, so a column covers `secs / cols` of audio
/// wherever the slider sits. Never zero, or a column would finish on
/// every sample.
fn frames_per_col(rate: f32, secs: f32, cols: usize) -> usize {
    ((rate * secs) as usize / cols.max(1)).max(1)
}

/// How many bars a strip `w` wide draws under `config`'s width and gap.
/// One definition shared by the paint and the live trace, so the trace is
/// cut to exactly the bars that will show it.
fn bar_count(w: f32, config: &WaveformConfig) -> usize {
    let (bar_w, gap) = config.bars();

    ((w / (bar_w + gap)) as usize).max(1)
}

/// Whether the strip has to ask for a frame of its own, given what it's
/// showing. Pure, because the answer is the whole difference between an
/// idle panel costing nothing and one repainting at refresh rate.
///
/// While something plays, the direct observe re-renders the strip on
/// every pump tick, the rate the playhead actually moves at, so frame
/// polling on top only redraws identical pixels. Frames are for what
/// `settling` gathers, the windows the pump doesn't notify through: a
/// morph, the generating stand-in, and the between-tracks blink. Pause
/// and skips don't notify on their own, so those windows are what cover
/// the transitions. A paused strip with a settled shape parks; the
/// pump's play-state notify wakes it on resume.
///
/// A playing station is the one case that really does move every frame.
/// The trace scrolls and the drawn shape runs off the clock, and both
/// should be smooth rather than stepping with the pump. Off moves
/// nothing, so a station left on with the strip off costs no frames at
/// all.
fn wants_frames(mode: LiveMode, streaming: bool, playing: bool, settling: bool) -> bool {
    if streaming {
        return mode != LiveMode::Off;
    }

    !playing && settling
}

/// The rolling trace behind [`Shape::Live`]: the audio tap cut into
/// columns as it arrives, newest at the back. A station has no file to
/// decode, so this is built from the same feed the visualizers read,
/// accumulated here because the feed itself only holds a fraction of a
/// second and the strip wants seconds.
struct LiveTrace {
    /// The window the held columns were cut at, seconds, and how many of
    /// them the strip draws. Kept so the next tick can notice either moved
    /// under it.
    secs: f32,
    width: usize,
    /// Where the last pull of the feed stopped.
    cursor: u64,
    /// Scratch for one pull's interleaved samples, reused every tick.
    pull: Vec<f32>,
    /// The column being filled: its extremes, the running sum of squares
    /// behind its RMS, and how many frames have gone into it.
    lo: f32,
    hi: f32,
    square: f32,
    frames: usize,
    /// The finished columns, oldest at the front. Always `width` long: a
    /// fresh trace starts silent, so the first seconds of a station scroll
    /// in from the right instead of stretching one column across the
    /// strip.
    cols: VecDeque<PeakBin>,
    /// What the paint reads, rebuilt when a column lands rather than per
    /// frame.
    shape: Arc<Vec<PeakBin>>,
    /// Whether anything has run through this trace since it was last
    /// reset, so the reset can cost nothing on the strip's ordinary path.
    armed: bool,
}

impl LiveTrace {
    /// A silent trace spanning `secs` of audio over `width` columns.
    fn new(secs: f32, width: usize) -> Self {
        let width = width.max(1);
        let cols: VecDeque<PeakBin> = std::iter::repeat_n(PeakBin::default(), width).collect();
        LiveTrace {
            secs,
            width,
            cursor: 0,
            pull: Vec::new(),
            lo: 0.0,
            hi: 0.0,
            square: 0.0,
            frames: 0,
            shape: Arc::new(cols.iter().copied().collect()),
            cols,
            armed: false,
        }
    }

    /// Take whatever the tap has pushed since the last tick and fold it
    /// into columns. Each column is one slice of `secs` across the strip;
    /// a tick usually lands part of one, and a tick after a stall can land
    /// several. True when at least one column finished, which is the only
    /// time the painted snapshot has to be rebuilt.
    ///
    /// A `secs` or a `width` the held columns weren't cut at starts the
    /// trace over. Nothing can re-cut a column after the fact, and
    /// stretching the old ones across a new window or a new bar count
    /// would draw seconds that never sounded like that.
    ///
    /// The cursor is the feed's own write count, so audio that fell off the
    /// ring while the panel was hidden is skipped rather than replayed: the
    /// trace jumps, which is honest, instead of running behind forever.
    fn step(&mut self, feed: &AudioFeed, secs: f32, width: usize) -> bool {
        // Both sides come out of the same clamped accessor, so an exact
        // compare is the whole test.
        if secs != self.secs || width.max(1) != self.width {
            self.restart(secs, width, feed);
        }

        self.armed = true;
        // A device rate off the far end of plausible would put absurdly
        // many frames in a column; the clamp is the oscilloscope's.
        let rate = feed.sample_rate().clamp(8_000, 384_000) as f32;
        let per_col = frames_per_col(rate, self.secs, self.width);
        self.cursor = feed.since(self.cursor, &mut self.pull);

        let mut landed = false;
        // Interleaved stereo in, mono out: the strip draws one row, and
        // the mix is what a mirrored envelope of a stream should show.
        for frame in self.pull.as_chunks::<2>().0 {
            let sample = (frame[0] + frame[1]) * 0.5;
            self.lo = self.lo.min(sample);
            self.hi = self.hi.max(sample);
            self.square += sample * sample;
            self.frames += 1;
            if self.frames < per_col {
                continue;
            }

            self.cols.push_back(PeakBin {
                lo: self.lo,
                hi: self.hi,
                rms: (self.square / self.frames as f32).sqrt(),
            });
            self.cols.pop_front();
            self.lo = 0.0;
            self.hi = 0.0;
            self.square = 0.0;
            self.frames = 0;
            landed = true;
        }
        if landed {
            self.shape = Arc::new(self.cols.iter().copied().collect());
        }

        landed
    }

    /// Start over at the tap's present: the trace belongs to the station
    /// that was playing, so the next one scrolls in clean rather than
    /// inheriting a tail of somebody else's broadcast. A no-op unless
    /// something has actually run through it, since the strip asks on
    /// every tick of every local file it draws.
    fn reset(&mut self, feed: &AudioFeed) {
        if !self.armed {
            return;
        }

        self.restart(self.secs, self.width, feed);
    }

    /// A fresh trace over `secs` and `width` columns, starting at what the
    /// tap holds now. The feed keeps counting whether or not a station is
    /// playing, so the cursor jump is what stops the new trace from
    /// replaying a second of whatever came before it.
    fn restart(&mut self, secs: f32, width: usize, feed: &AudioFeed) {
        *self = LiveTrace::new(secs, width);
        self.cursor = feed.written();
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
    live_scrub: ScrubState,
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
    /// The PCM tap, for the rolling trace a station draws instead of a
    /// decoded shape.
    feed: Arc<AudioFeed>,
    live: LiveTrace,
    /// The config's motion expression, parsed.
    motion: Motion,
    /// The drawn shape's `t`, and when it was last advanced. Its own clock
    /// rather than the epoch, because the strip still repaints while a
    /// station is paused (the pump notifies as the buffer falls behind
    /// live) and a shape read off wall time would keep moving under it.
    motion_secs: f32,
    motion_tick: Instant,
    /// The expression field on the settings page, built the first time
    /// that page shows it, with the subscription writing what's typed
    /// back into the config.
    motion_input: Option<(Entity<InputState>, Subscription)>,
    /// The playing track's bookmarks and which track they were read for,
    /// re-read on a track change and on a bookmark edit rather than on
    /// every tick the strip repaints on.
    marks: Vec<Bookmark>,
    marks_key: Option<TrackKey>,
    /// The bookmark ribbon the pointer is on, for its readout.
    hover_mark: Option<i64>,
    /// The playing track's session cues, cached on the same terms as the
    /// bookmarks above and for the same reason.
    cues: Vec<Cue>,
    cues_key: Option<TrackKey>,
    /// The cue chevron the pointer is on, for its readout.
    hovered_cue: Option<u64>,
    /// Where the last right click on bare strip landed, as a position in
    /// the track. The insert menu builds a frame after the press and never
    /// sees the event, so the position is parked here on the way past.
    insert_at_ms: Arc<AtomicU32>,
    /// Wakes the panel when a session starts, so an idle window notices the
    /// new track without the player bar's frame pump.
    _player_changed: Subscription,
    _library_changed: Subscription,
    _cues_changed: Subscription,
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
        // A cue dropped or taken off the track this strip is drawing. The
        // event names its track, so a strip on a different song ignores it
        // rather than throwing away a set that didn't move.
        let _cues_changed = cx.subscribe(
            &state.cues,
            |this: &mut Self, _, event: &CuesChanged, cx| {
                if this.cues_key.as_ref() == Some(&event.key) {
                    this.cues_key = None;
                    cx.notify();
                }
            },
        );
        WaveformPanel {
            feed: state.player.read(cx).feed(),
            live: LiveTrace::new(config.live_secs(), LIVE_COLS),
            motion: Motion::compile(config.live_motion()),
            motion_secs: 0.0,
            motion_tick: Instant::now(),
            motion_input: None,
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
            live_scrub: ScrubState::default(),
            ramp_pickers: None,
            _ramp_changes: Vec::new(),
            value_edit: panel::ValueEdit::default(),
            epoch: Instant::now(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            marks: Vec::new(),
            marks_key: None,
            hover_mark: None,
            cues: Vec::new(),
            cues_key: None,
            hovered_cue: None,
            insert_at_ms: Arc::new(AtomicU32::new(0)),
            _player_changed,
            _library_changed,
            _cues_changed,
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

    /// The playing track's session cues, on the same terms as the
    /// bookmarks above: read once per track and again after an edit.
    fn cues_for(&mut self, key: &TrackKey, cx: &App) -> &[Cue] {
        if self.cues_key.as_ref() != Some(key) {
            self.cues = self.state.cues.read(cx).for_key(key);
            self.cues_key = Some(key.clone());
            self.hovered_cue = None;
        }

        &self.cues
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

    /// The trace reads the new window on its next tick and starts over
    /// there, so a drag across the slider redraws from the tap's present
    /// rather than stretching what it was already holding.
    fn set_live_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        self.config.live_secs = secs;
        cx.notify();
    }

    /// The parsed expression, caught up to whatever the config holds.
    /// Parsing lands here rather than in the paint closure, and on the
    /// frames where nothing changed it costs one string compare.
    fn motion(&mut self) -> &Motion {
        self.motion.sync(self.config.live_motion());

        &self.motion
    }

    /// The expression field, seeded from the config the first time the
    /// settings page asks for it and writing every keystroke straight
    /// back. Nothing is debounced: a panel config goes to disk with the
    /// layout dump rather than through the settings file, so a keystroke
    /// costs a reparse and no I/O.
    fn motion_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some((input, _)) = &self.motion_input {
            return input.clone();
        }

        let current = self.config.live_motion.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let events = cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }

            this.config.live_motion = input.read(cx).value().to_string();
            cx.notify();
        });
        self.motion_input = Some((input.clone(), events));

        input
    }

    /// Put the default shape back, in the config and in the field both,
    /// so the box goes on showing what the strip is drawing.
    fn reset_live_motion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.live_motion = LIVE_MOTION_DEFAULT.into();
        let input = self.motion_input.as_ref().map(|(input, _)| input.clone());
        if let Some(input) = input {
            input.update(cx, |input, cx| {
                input.set_value(LIVE_MOTION_DEFAULT, window, cx)
            });
        }

        cx.notify();
    }

    fn strip(
        &self,
        marker: Option<f32>,
        ab: Option<(f32, Option<f32>)>,
        marks: Vec<bookmark_ui::Mark>,
        cues: Vec<cue_ui::CueMark>,
        gaps: Vec<f32>,
    ) -> impl IntoElement + use<> {
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
                    &from, &to, u, t, marker, ab, &marks, &cues, &gaps, &config, bounds, window,
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

/// Dashes across the strip while a stream is being waited for, and how
/// much of each dash's slot is drawn. A broken line reads as "nothing yet"
/// where a solid one would read as silence that the stream is actually
/// sending.
const SCAN_DASHES: usize = 32;
const SCAN_DUTY: f32 = 0.45;

/// What the strip says about the stream itself, over the trace: the corner
/// mark in the seek strip's colors, and, while the stream is opening or
/// reconnecting, a dashed line down the middle where the trace would be.
/// The trace under it is silent at that point (no audio is arriving), so
/// the dashes stand in for the flat line rather than covering anything.
///
/// `behind` is the playhead sitting back in the timeshift tape. The trace
/// keeps drawing what's being heard, which is then the past rather than
/// what's on air, so the mark takes the seek strip's behind-live face and
/// stops claiming the edge. The dashes stay on the stream's own color: the
/// wait they draw is about the connection, not about where the playhead is.
fn live_overlay(stream: Option<StreamState>, paused: bool, behind: bool, t: f32) -> AnyElement {
    let (color, opacity) = live_tint(stream, paused, t);
    let (mark_color, mark_opacity) = seek::live_mark_tint(stream, paused, behind, t);
    // A paused station scans nothing: it's hung up, not waiting.
    let waiting = !paused
        && matches!(
            stream,
            Some(StreamState::Opening) | Some(StreamState::Reconnecting)
        );
    let dash = palette::alpha(color, 0x99);

    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .top(tokens::SPACE_XS)
                .right(tokens::SPACE_SM)
                .text_xs()
                .text_color(mark_color)
                .opacity(mark_opacity)
                .child(rox_i18n::t!("waveform-streaming")),
        )
        .when(waiting, |d| {
            d.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let w = f32::from(bounds.size.width);
                        let h = f32::from(bounds.size.height);
                        if w <= 0.0 || h <= 0.0 {
                            return;
                        }

                        let step = w / SCAN_DASHES as f32;
                        let y = bounds.origin.y + px(h / 2.0 - 0.5);
                        for i in 0..SCAN_DASHES {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + px(i as f32 * step), y),
                                    size(px(step * SCAN_DUTY), px(1.0)),
                                ),
                                dash,
                            ));
                        }
                    },
                )
                .absolute()
                .size_full()
                .opacity(opacity),
            )
        })
        .into_any_element()
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

/// A bin as a mirrored bar around `center`: the extremes reaching out to
/// `max_bar` with the strip's floor under them, the loudness band inside
/// them. Every shape that has bins to draw goes through here, so the
/// decoded strip, the live trace, and the drawn shape all read at the
/// same scale.
///
/// The band gets no minimum of its own: silence leaves the envelope's
/// stub alone rather than laying a second stub over it. It's clamped into
/// the envelope, which the normalization already guarantees but the bar
/// floors can undercut.
fn envelope_bar(bin: PeakBin, center: f32, max_bar: f32, envelope: Rgba, band: Rgba) -> Bar {
    let top = center - (bin.hi * max_bar).max(MIN_BAR / 2.0);
    let bottom = center - (bin.lo * max_bar).min(-MIN_BAR / 2.0);
    let reach = bin.rms * max_bar;

    Bar {
        top,
        bottom,
        band_top: (center - reach).max(top),
        band_bottom: (center + reach).min(bottom),
        envelope,
        band,
    }
}

/// One bar of the drawn shape: the expression at this bar's position and
/// this frame's clock, as a symmetric envelope with the band at half.
///
/// A hand-typed expression is free to divide by zero or take the root of
/// a negative, so the result is squared away here rather than at the
/// quad. An infinity is a value that ran off the top and clamps to the
/// edge like any other; a NaN is no value at all and draws flat, which
/// at least says on screen that something is wrong with the line.
fn motion_bin(expr: &Expr, x: f32, t: f32) -> PeakBin {
    let value = expr.eval(x, t);
    let reach = if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0).abs()
    };

    PeakBin {
        lo: -reach,
        hi: reach,
        rms: reach * 0.5,
    }
}

/// The station's reconnects placed along the live trace. The strip holds
/// the last `secs` of audio that came out of the speakers and a gap says
/// how long ago it went past, so the two line up with no tape arithmetic
/// in between.
///
/// A break the cursor hasn't reached, or one that has scrolled off the
/// left end, has no column to sit on and is dropped rather than pinned to
/// an edge: a mark stuck at the end of the strip would claim a seam in
/// audio that either hasn't been heard or is no longer drawn.
fn trace_gaps(gaps: &[LiveGap], secs: f32) -> Vec<f32> {
    if secs <= 0.0 {
        return Vec::new();
    }

    gaps.iter()
        .map(|gap| gap.heard_ago_secs)
        .filter(|ago| (0.0..=secs as f64).contains(ago))
        .map(|ago| (1.0 - ago / secs as f64) as f32)
        .collect()
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
        // Every column is audio that already played, so the trace takes
        // the full colors with no ghost half and no split to fold. The
        // trace is cut to this bar count, so a column is read straight
        // through; the fold only runs on the frame between a resize and
        // the restart that follows it.
        Shape::Live(cols) => {
            let bin = if cols.len() == count {
                cols.get(i).copied()
            } else {
                bucket(cols, i, count)
            };
            let Some(bin) = bin else {
                return Bar::flat(center, palette::alpha(palette::accent(), 0));
            };

            envelope_bar(bin, center, max_bar, layers.0, layers.1)
        }
        // Nothing sampled and nothing stored: the bar's own place on the
        // strip and the clock go into the expression, and a height comes
        // back. Full colors, like the trace: there's no past half of a
        // stream to ghost.
        Shape::Motion(expr, clock) => envelope_bar(
            motion_bin(expr, x_mid / w, *clock),
            center,
            max_bar,
            layers.0,
            layers.1,
        ),
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

            let played = x_mid <= progress.clamp(0.0, 1.0) * w;
            let (envelope, band) = if played {
                layers
            } else {
                (ghost(layers.0), ghost(layers.1))
            };

            envelope_bar(bin, center, max_bar, envelope, band)
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
    cues: &[cue_ui::CueMark],
    gaps: &[f32],
    config: &WaveformConfig,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let (_, gap) = config.bars();
    let count = bar_count(w, config);
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

    // The station's reconnects, straight down the trace. The stall either
    // side of one is silent, so there's often no bar left for the break to
    // cut, and the notch over the top is what says a seam went past rather
    // than a quiet passage. The seek strip draws the same shape; it lives
    // over there because that's the strip that can't be clicked across.
    seek::paint_gaps(
        gaps,
        (seek::GAP_NOTCH_H, h - seek::GAP_NOTCH_H),
        1.0,
        bounds,
        window,
    );

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
        cue_ui::paint_marks(cues, weight, bounds, window);
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
        let live_secs = self.config.live_secs();
        // How the strip draws, whether the shape under it is decoded or live.
        let strip = div()
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
            ));
        // What a station gets, off in a group of its own. It's here
        // whatever is playing rather than appearing the first time a
        // station opens, so the heading and each row's own line have to
        // say what they're for to somebody who has never played one.
        let mode = self.config.live;
        let motion_error = self.motion().error.clone();
        let motion_input = self.motion_input(window, cx);
        let live = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("waveform-live-mode"),
                Some(rox_i18n::t!("waveform-live-mode.description")),
                choices_shared(
                    &live_mode_choices(),
                    mode,
                    |this: &mut Self, mode, cx| {
                        this.config.live = mode;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(mode == LiveMode::Trace, |d| {
                d.child(setting_row(
                    rox_i18n::t!("waveform-live-window"),
                    Some(rox_i18n::t!("waveform-live-window.description")),
                    settings_ui::scalar(
                        &self.live_scrub,
                        &self.value_edit,
                        live_secs,
                        settings_ui::span(LIVE_SECS_MIN, LIVE_SECS_MAX, "s").hard(),
                        Self::set_live_secs,
                        cx,
                    ),
                ))
            })
            .when(mode == LiveMode::Motion, |d| {
                // The complaint goes under the field rather than in place
                // of the strip: the shape keeps drawing (the default one)
                // while the expression is half-typed, which is most of
                // the time somebody is editing it.
                let field = div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .child(Input::new(&motion_input).small())
                    .when_some(motion_error, |d, error| {
                        let line = rox_i18n::t!("waveform-live-expression-error", reason = error);

                        d.child(div().text_xs().text_color(palette::tone_bad()).child(line))
                    });

                d.child(panel::setting_block(
                    rox_i18n::t!("waveform-live-expression"),
                    Some(rox_i18n::t!("waveform-live-expression.description")),
                    Some(
                        settings_ui::small_button(
                            rox_i18n::t!("panel-reset"),
                            icons::REFRESH_CW,
                            self.config.live_motion() == LIVE_MOTION_DEFAULT,
                            cx.listener(|this, _, window, cx| this.reset_live_motion(window, cx)),
                        )
                        .into_any_element(),
                    ),
                    field,
                ))
            });

        div()
            .flex()
            .flex_col()
            .gap(settings_ui::SECTION_GAP)
            .child(strip)
            .child(settings_ui::section(
                rox_i18n::t!("waveform-section-live"),
                None,
                live,
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

        // A station has no file to decode a shape out of and no length to
        // draw one against, so the strip says so instead of keeping the
        // last file's waveform up over a stream that has nothing to do
        // with it.
        let live = now.as_ref().is_some_and(|now| now.live);

        // The drawn shape's clock runs only while a station plays. The tick
        // moves on regardless, so a resume picks up where the pause left
        // off instead of jumping by however long it lasted.
        let tick = Instant::now();
        if live && playing {
            self.motion_secs += tick.duration_since(self.motion_tick).as_secs_f32();
        }
        self.motion_tick = tick;

        // Kick a decode when the playing track changes.
        if let Some(now) = &now {
            // Keyed on the file: the strip draws the whole image's shape,
            // and a cue rip's tracks are all inside one. A remote track has
            // no file to decode a shape out of, so the strip stays on
            // whatever it last drew rather than flashing empty.
            if let Some(path) = now.path()
                && self.track.as_deref() != Some(path)
            {
                self.start_decode(path.to_path_buf(), cx);
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
        // Everything keyed to a position in the track drops out together
        // while a station plays: the marks, the layer that drops new ones,
        // and the menus over both.
        let positional = position_bound::allowed(&self.state, cx);
        // The track's bookmarks, placed along the strip. Kept through the
        // between-tracks blink so the marks don't flash off with the shape.
        let marks = match (&now, self.config.bookmarks && positional) {
            (Some(now), true) => {
                let key = now.key.clone();
                let duration = now.duration_secs;
                bookmark_ui::marks(self.marks_for(&key, cx), duration)
            }
            _ => Vec::new(),
        };
        let hover_mark = self.hover_mark;
        // The track's session cues, on the top edge opposite them.
        let cues = match (&now, positional) {
            (Some(now), true) => {
                let key = now.key.clone();
                let duration = now.duration_secs;
                cue_ui::marks(self.cues_for(&key, cx), duration)
            }
            _ => Vec::new(),
        };
        let hovered_cue = self.hovered_cue;
        // The station's reconnects, on the trace's own axis. Only the trace
        // asks for them. Off draws nothing to break, and the motion shape
        // is an expression over the strip's width rather than over anything
        // that was heard, so a seam in the broadcast has no place on it.
        let gaps = match live && self.config.live == LiveMode::Trace {
            true => trace_gaps(
                &self.state.player.read(cx).live_gaps(),
                self.config.live_secs(),
            ),

            false => Vec::new(),
        };

        // The seek preview only shows on real peaks: the placeholder and
        // the unavailable message have no track shape to point along.
        let mut hover_duration: Option<f64> = None;
        let body = match (&now, &self.peaks) {
            // A stream: no file to decode a shape out of, so the strip
            // draws the tap itself, the last few seconds of it rolling
            // right to left. The decoded shape it was holding morphs into
            // the trace the way any two shapes do.
            (Some(_), _) if live => match self.config.live {
                // Nothing on the strip but the corner mark over it. The
                // shape snaps blank rather than easing out, the way an
                // ended queue does, so turning the trace back on fades in
                // from nothing.
                LiveMode::Off => {
                    self.from = Shape::Blank;
                    self.to = Shape::Blank;
                    div().into_any_element()
                }

                LiveMode::Trace => {
                    // Cut to the bars the strip drew last frame, so each
                    // column is one bar and slides left whole. Before the
                    // first paint the width is a guess and the first real
                    // one restarts it.
                    let width = self
                        .scrub
                        .width()
                        .map(|w| bar_count(w, &self.config))
                        .unwrap_or(LIVE_COLS);
                    self.live.step(&self.feed, self.config.live_secs(), width);
                    self.retarget(Shape::Live(self.live.shape.clone()));
                    self.strip(None, None, Vec::new(), Vec::new(), gaps.clone())
                        .into_any_element()
                }

                LiveMode::Motion => {
                    let expr = self.motion().expr.clone();
                    self.retarget(Shape::Motion(expr, self.motion_secs));
                    self.strip(None, None, Vec::new(), Vec::new(), Vec::new())
                        .into_any_element()
                }
            },
            // Hold the strip through the blink: whatever it shows stays up,
            // and the next track's shape morphs from it instead of popping
            // in from blank.
            (None, _) if between_tracks => self
                .strip(marker, ab, marks.clone(), cues.clone(), Vec::new())
                .into_any_element(),
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
                self.strip(marker, ab, marks.clone(), cues.clone(), Vec::new())
                    .into_any_element()
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
                self.strip(marker, ab, marks.clone(), cues.clone(), Vec::new())
                    .into_any_element()
            }
        };

        // Nothing tracing on screen: the columns belong to the station
        // that was playing, so the next one starts clean. A mode that
        // isn't the trace counts as nothing tracing, or switching back to
        // it would scroll in seconds of a broadcast that ended while the
        // strip was drawing something else.
        if !live || self.config.live != LiveMode::Trace {
            self.live.reset(&self.feed);
        }

        let morphing = self.morph_at.elapsed().as_secs_f32() < tokens::EASE_SECS;
        let generating = matches!(self.to, Shape::Placeholder);
        let settling = between_tracks || morphing || generating;
        if wants_frames(self.config.live, live && playing, playing, settling) {
            window.request_animation_frame();
        }

        // The strip's own right click. It follows the seek readout's rule,
        // a real shape with a length under it, since a press on the
        // placeholder has no position to name.
        let insert =
            now.as_ref()
                .zip(hover_duration.filter(|_| positional))
                .map(|(now, duration)| {
                    seek::insert_layer(
                        &self.state,
                        &now.key,
                        duration,
                        &self.scrub,
                        &self.insert_at_ms,
                        cx,
                    )
                });

        div()
            .size_full()
            .bg(palette::bg_root())
            .relative()
            .when(!live, |d| {
                d.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.scrub.begin();
                        if let Some(fraction) = this.scrub.fraction(event.position.x) {
                            panel::seek_fraction(&this.state.player, fraction, cx);
                        }
                        cx.notify();
                    }),
                )
            })
            // Ahead of the shape and both overlays, which is what puts it
            // last in line for a right click. See `seek::insert_layer`.
            .children(insert)
            .child(body)
            .when(live, |d| {
                let stream = now.as_ref().and_then(|now| now.stream);
                // Where the playhead sits in the station's tape, which the
                // corner mark reads and the trace under it doesn't: the
                // trace is whatever is coming out of the speakers.
                let behind = now
                    .as_ref()
                    .is_some_and(|now| seek::behind_live(now.shift.as_ref()));
                d.child(live_overlay(
                    stream,
                    !playing,
                    behind,
                    self.epoch.elapsed().as_secs_f32(),
                ))
            })
            .when_some(hover_duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            })
            // The marks' hit layers go over the seek readout's, so a
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
            .when_some(
                now.as_ref()
                    .filter(|_| hover_duration.is_some() && !cues.is_empty()),
                |d, now| {
                    d.child(cue_ui::overlay(
                        &self.state,
                        &now.key,
                        &cues,
                        hovered_cue,
                        &self.scrub,
                        |this: &mut Self, id, _| this.hovered_cue = id,
                        cx,
                    ))
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One column's worth of frames, loud at the start and quiet after, so
    /// the extremes and the RMS all land somewhere different.
    fn feed_with(frames: usize, level: f32) -> AudioFeed {
        let feed = AudioFeed::new();
        feed.set_sample_rate(48_000);
        let samples: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = if i % 2 == 0 { level } else { -level };
                [v, v]
            })
            .collect();
        feed.push(&samples);
        feed
    }

    /// The trace keeps a fixed width and scrolls: a finished column lands
    /// at the newest end carrying the extremes of the audio that filled
    /// it, and the ring stays the length the strip draws, so the first
    /// seconds of a station come in from the right rather than stretching
    /// across the panel.
    #[test]
    fn the_trace_scrolls_a_fixed_number_of_columns() {
        let per_col = frames_per_col(48_000.0, LIVE_SECS_DEFAULT, LIVE_COLS);
        let feed = feed_with(per_col, 0.5);
        let mut trace = LiveTrace::new(LIVE_SECS_DEFAULT, LIVE_COLS);

        assert_eq!(trace.shape.len(), LIVE_COLS, "a fresh trace is full width");
        assert!(
            trace.step(&feed, LIVE_SECS_DEFAULT, LIVE_COLS),
            "a full column landed"
        );
        assert_eq!(trace.shape.len(), LIVE_COLS, "and the width didn't change");

        let newest = trace.cols.back().copied().expect("the column that landed");
        assert!((newest.hi - 0.5).abs() < 0.01);
        assert!((newest.lo + 0.5).abs() < 0.01);
        assert!((newest.rms - 0.5).abs() < 0.01, "a square wave is its peak");
        assert_eq!(
            trace.cols.front().copied().map(|bin| bin.hi),
            Some(0.0),
            "the oldest end is still the silence it started as"
        );
    }

    /// Half a column's audio moves nothing: the samples are held on the
    /// accumulator until enough of them arrive, which is what keeps the
    /// scroll even rather than stepping with the pump.
    #[test]
    fn a_partial_column_waits() {
        let per_col = frames_per_col(48_000.0, LIVE_SECS_DEFAULT, LIVE_COLS);
        let feed = feed_with(per_col / 2, 0.5);
        let mut trace = LiveTrace::new(LIVE_SECS_DEFAULT, LIVE_COLS);

        assert!(
            !trace.step(&feed, LIVE_SECS_DEFAULT, LIVE_COLS),
            "nothing finished"
        );
        assert_eq!(trace.frames, per_col / 2, "the frames are held");
        assert!(
            trace.cols.iter().all(|bin| bin.hi == 0.0),
            "and no column moved"
        );
    }

    /// A reset drops the station's trace and jumps the cursor to the tap's
    /// present, so the next station doesn't replay the tail of the last
    /// one. It costs nothing until something has actually run through.
    #[test]
    fn a_reset_forgets_the_station_and_skips_the_ring() {
        let feed = feed_with(4096, 0.5);
        let mut trace = LiveTrace::new(LIVE_SECS_DEFAULT, LIVE_COLS);

        trace.reset(&feed);
        assert_eq!(trace.cursor, 0, "an untouched trace is left alone");

        trace.step(&feed, LIVE_SECS_DEFAULT, LIVE_COLS);
        trace.reset(&feed);
        assert_eq!(trace.cursor, feed.written(), "caught up to the tap");
        assert_eq!(trace.secs, LIVE_SECS_DEFAULT, "the window survives a reset");
        assert!(trace.cols.iter().all(|bin| bin.hi == 0.0));
    }

    /// The columns held were cut at the old window, so a moved slider
    /// can't be applied to them. The trace starts over at the tap's
    /// present instead of stretching a history that never sounded that
    /// way across the new span.
    #[test]
    fn a_changed_window_starts_the_trace_over() {
        let per_col = frames_per_col(48_000.0, LIVE_SECS_DEFAULT, LIVE_COLS);
        let feed = feed_with(per_col * 2, 0.5);
        let mut trace = LiveTrace::new(LIVE_SECS_DEFAULT, LIVE_COLS);

        assert!(
            trace.step(&feed, LIVE_SECS_DEFAULT, LIVE_COLS),
            "columns landed"
        );
        assert!(
            trace.cols.iter().any(|bin| bin.hi > 0.0),
            "the trace has audio in it"
        );

        assert!(
            !trace.step(&feed, LIVE_SECS_MAX, LIVE_COLS),
            "the restart left nothing behind to finish a column with"
        );
        assert_eq!(trace.secs, LIVE_SECS_MAX, "the trace took the new window");
        assert_eq!(trace.shape.len(), LIVE_COLS, "the width never moves");
        assert!(
            trace.cols.iter().all(|bin| bin.hi == 0.0),
            "and the old columns are gone"
        );
        assert_eq!(trace.cursor, feed.written(), "caught up to the tap");
    }

    /// A strip that grew or shrank draws a different number of bars, and
    /// the trace follows it: the old columns can't be re-cut to the new
    /// width, so it starts over at the new one, one column per bar.
    #[test]
    fn a_changed_width_starts_the_trace_over_at_the_new_bar_count() {
        let per_col = frames_per_col(48_000.0, LIVE_SECS_DEFAULT, LIVE_COLS);
        let feed = feed_with(per_col * 4, 0.5);
        let mut trace = LiveTrace::new(LIVE_SECS_DEFAULT, LIVE_COLS);
        trace.step(&feed, LIVE_SECS_DEFAULT, LIVE_COLS);
        assert!(trace.cols.iter().any(|bin| bin.hi > 0.0));

        assert!(
            !trace.step(&feed, LIVE_SECS_DEFAULT, 120),
            "the restart left nothing behind to finish a column with"
        );
        assert_eq!(trace.width, 120);
        assert_eq!(trace.shape.len(), 120, "one column per bar");
        assert!(trace.cols.iter().all(|bin| bin.hi == 0.0));
        assert_eq!(
            frames_per_col(48_000.0, LIVE_SECS_DEFAULT, 120),
            (48_000.0 * LIVE_SECS_DEFAULT) as usize / 120,
            "and each column now covers a wider slice of the window"
        );
    }

    /// The column count is fixed, so the window alone sets how much time
    /// one column covers. That's what makes the slider a speed control:
    /// double the window and every column holds twice the audio, at any
    /// device rate.
    #[test]
    fn a_column_covers_the_window_cut_into_columns() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for secs in [LIVE_SECS_MIN, LIVE_SECS_DEFAULT, LIVE_SECS_MAX] {
                let covered = frames_per_col(rate, secs, LIVE_COLS) as f32 / rate;
                assert!(
                    (covered - secs / LIVE_COLS as f32).abs() < 1e-4,
                    "{secs}s at {rate} Hz gave {covered}s a column"
                );
            }
        }
    }

    /// A hand-edited layout is the one place these arrive broken. A NaN
    /// window casts to zero frames a column, and the floor under that
    /// would finish a column on every sample.
    #[test]
    fn config_accessors_swallow_junk() {
        for (set, want) in [
            (f32::NAN, LIVE_SECS_DEFAULT),
            (0.0, LIVE_SECS_MIN),
            (-30.0, LIVE_SECS_MIN),
            (600.0, LIVE_SECS_MAX),
            (f32::INFINITY, LIVE_SECS_MAX),
            (f32::NEG_INFINITY, LIVE_SECS_MIN),
        ] {
            let config = WaveformConfig {
                live_secs: set,
                ..WaveformConfig::default()
            };
            assert_eq!(config.live_secs(), want, "{set} landed wrong");
        }

        // The expression is the other field a hand-edit reaches, and an
        // empty one reads as never set: clearing the box puts the default
        // shape back rather than flattening the strip.
        for set in ["", "   ", "\n\t"] {
            let config = WaveformConfig {
                live_motion: set.into(),
                ..WaveformConfig::default()
            };
            assert_eq!(
                config.live_motion(),
                LIVE_MOTION_DEFAULT,
                "{set:?} landed wrong"
            );
        }

        let config = WaveformConfig {
            live_motion: "  x * 2  ".into(),
            ..WaveformConfig::default()
        };
        assert_eq!(config.live_motion(), "x * 2", "read trimmed");
    }

    /// A mode name nothing recognizes costs the panel that one field.
    /// Failing the whole config out instead would drop every other knob
    /// on the panel back to stock over one typo in a hand-edited layout.
    #[test]
    fn an_unknown_mode_takes_the_default_and_leaves_the_rest_alone() {
        let config: WaveformConfig = serde_json::from_str(r#"{"live":"sparkle","bar_gap":3.0}"#)
            .expect("the rest of the config still loads");
        assert_eq!(config.live, LiveMode::Motion);
        assert_eq!(config.bar_gap, 3.0);

        for (name, want) in [
            ("off", LiveMode::Off),
            ("trace", LiveMode::Trace),
            ("motion", LiveMode::Motion),
        ] {
            let config: WaveformConfig = serde_json::from_str(&format!(r#"{{"live":"{name}"}}"#))
                .expect("a name it knows loads");
            assert_eq!(config.live, want);
            assert_eq!(
                serde_json::to_value(config.live).ok(),
                Some(serde_json::Value::String(name.into())),
                "and goes back out as it came in"
            );
        }
    }

    /// An expression that doesn't parse is caught once, where the text
    /// arrives, and the strip goes on drawing the default shape. Two
    /// things depend on it: a half-typed field can't blank the panel, and
    /// paint is never the thing that finds out.
    #[test]
    fn a_bad_expression_falls_back_to_the_default() {
        let default = Motion::compile(LIVE_MOTION_DEFAULT);
        assert!(default.error.is_none(), "the default parses");

        let broken = Motion::compile("0.5 * sin(");
        let reason = broken.error.clone().expect("the row says what went wrong");
        assert!(reason.contains(" at "), "and where in the line: {reason}");
        for i in 0..16 {
            let x = i as f32 / 15.0;
            assert_eq!(
                broken.expr.eval(x, 0.5),
                default.expr.eval(x, 0.5),
                "the default shape is what's drawing"
            );
        }

        // And it recompiles when the field moves under it, which is what
        // keeps the parse off the paint path.
        let mut motion = broken;
        motion.sync("x");
        assert!(motion.error.is_none());
        assert_eq!(motion.expr.eval(0.25, 0.0), 0.25);
    }

    /// A typed expression is free to divide by zero or root a negative,
    /// and either one drawn straight would put a bar somewhere off the
    /// panel or draw nothing at all, silently.
    #[test]
    fn a_bar_off_a_wild_expression_stays_on_the_strip() {
        let wild = |src: &str| {
            let expr = Expr::parse(src).expect("parses");
            motion_bin(&expr, 0.5, 0.0)
        };

        assert_eq!(wild("1 / 0").hi, 1.0, "an infinity clamps to the edge");
        assert_eq!(wild("sqrt(0 - 1)").hi, 0.0, "a NaN draws flat");
        assert_eq!(wild("0 - 99").hi, 1.0, "and the envelope is symmetric");
        let bin = wild("0.5");
        assert_eq!((bin.lo, bin.hi, bin.rms), (-0.5, 0.5, 0.25));
    }

    /// The strip turned off over a station asks for nothing: no trace to
    /// scroll and no clock to follow means an idle panel, however long
    /// the radio stays on.
    #[test]
    fn the_strip_off_over_a_station_asks_for_no_frames() {
        // A station, playing, nothing settling.
        assert!(!wants_frames(LiveMode::Off, true, true, false));
        assert!(wants_frames(LiveMode::Trace, true, true, false));
        assert!(wants_frames(LiveMode::Motion, true, true, false));

        for mode in [LiveMode::Off, LiveMode::Trace, LiveMode::Motion] {
            assert!(
                !wants_frames(mode, false, true, false),
                "a playing file leaves the frames to the pump"
            );
            assert!(
                !wants_frames(mode, false, false, false),
                "and a settled paused strip parks"
            );
            // The windows the pump doesn't notify through are nobody's
            // mode: they happen with a file up as much as a station.
            assert!(wants_frames(mode, false, false, true));
        }
    }
}
