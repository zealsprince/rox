//! The app's shared modulation layer: named signals over the playback
//! spectrum that any parameter anywhere can ride. A [`Signal`] is one
//! source (a frequency band's energy, the whole mix's level, a transient
//! detector, a threshold trigger, or a running total of another signal) with its response
//! smoothing and its gate; a [`Route`] attaches one signal to one
//! host-defined parameter with an output span. The pool
//! lives in a [`SignalHub`] evaluated once per frame off the shared
//! [`crate::AudioFeed`], so ten panels riding the same kick read the same
//! value from one FFT. What a target id means, and how a span fraction
//! maps into a parameter's native units, stays with the host.
//!
//! Everything degrades quietly: a route whose signal is gone contributes
//! nothing, and a signal nobody routes just idles.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::analysis::{log_bands, Analyzer};
use crate::AudioFeed;

/// dB window signals normalize into, on magnitudes where a full-scale sine
/// sits at 0 dB. The same window the spectrum's bars draw against, so a
/// signal level means the same loudness everywhere.
pub const FLOOR_DB: f32 = -66.0;
pub const MAX_DB: f32 = -12.0;

/// The band a source may watch. Matches the hearing-range span the audio
/// panels' sliders cover.
pub const BAND_MIN_HZ: f32 = 20.0;
pub const BAND_MAX_HZ: f32 = 20_000.0;

/// The hub's analysis window. Signals pool whole bands rather than
/// resolving single bins, so a short reactive window beats a fine one.
const HUB_FFT: usize = 2048;

/// How long the feed may sit still before it reads as stopped audio rather
/// than the gap between pump ticks; the same reasoning as the spectrum's
/// hold, so signals never strobe between ticks on high-refresh displays.
const SILENT_AFTER: f32 = 0.15;

/// Two hub ticks closer together than this are one frame asking twice:
/// several panels step the hub from their own paint, and only the first
/// per frame should advance the clock.
const TICK_MIN: f32 = 0.003;

/// What a signal listens to.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    /// Peak magnitude across a frequency band, normalized into the dB
    /// window: a kick band, a hat band, wherever the ear points.
    Band { lo: f32, hi: f32 },
    /// The whole spectrum's peak: the mix's loudness at a glance.
    Level,
    /// A pulse on each transient in the band: 1 the moment the band jumps
    /// past its own recent average, decaying at the response rate. The
    /// signal a hit rides, where Band is the signal a swell rides.
    Onset { lo: f32, hi: f32 },
    /// A pulse when the band reaches a line the user drew: 1 the moment it
    /// crosses the signal's threshold, decaying at the response rate,
    /// armed again once the band falls back under. Onset with the
    /// judgment moved from a moving reference to a fixed level, for
    /// material where the reference never gets to drop - a kick over
    /// sustained sub fires here where Onset arms once and goes quiet.
    Trigger { lo: f32, hi: f32 },
    /// A running total of another signal's output: music-driven time. It
    /// climbs by `of`'s value times `rate` each second and wraps at 1, so
    /// a shader reads it as a phase (`sin(TAU * s)` runs straight through
    /// the wrap) and it keeps its precision however long the app is up,
    /// which an unbounded float would not.
    ///
    /// Its own signal rather than a second channel on `of`: a route, a
    /// meter and a shader slot all address one id and read one number, and
    /// a signal carrying two values would break that everywhere at once.
    /// Wanting both the level and its total is two pool entries.
    Aggregate { of: u64, rate: f32 },
}

impl Source {
    /// The watched bin span for this source, clamped so a hand-edited file
    /// can't invert the band or walk off the spectrum.
    fn bins(&self, sample_rate: u32, half: usize) -> (usize, usize) {
        let (lo, hi) = match *self {
            Source::Band { lo, hi } | Source::Onset { lo, hi } | Source::Trigger { lo, hi } => {
                let lo = lo.clamp(BAND_MIN_HZ, BAND_MAX_HZ);
                (lo, hi.clamp(lo * 1.01, BAND_MAX_HZ))
            }
            // An aggregate watches a signal rather than a spectrum, so it
            // never asks for bins; the arm is here for the match.
            Source::Level | Source::Aggregate { .. } => (BAND_MIN_HZ, BAND_MAX_HZ),
        };
        log_bands(1, lo, hi, sample_rate, half)[0]
    }
}

/// The fastest an aggregate may climb, wraps per second at full input. A
/// hand-edited file can't ask for a phase that laps several times a frame,
/// which would read as noise rather than motion.
pub const AGGREGATE_RATE_MAX: f32 = 8.0;

/// One shared signal in the pool: a stable id routes point at, the source,
/// and the response smoothing every route off it shares.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Signal {
    pub id: u64,
    /// A name the user gave it, "Kick", "Mix swell". Empty follows the
    /// label derived from the source.
    pub name: String,
    pub source: Source,
    /// Response smoothing, 0 to 1: 0 snaps to the music, 1 drifts after
    /// it. On an onset or trigger source this is the pulse's decay
    /// instead.
    pub smooth: f32,
    /// The gate: anything under this reads as nothing, 0 to 1 against the
    /// signal's own output, 0 for no gate. What it buys is silence between
    /// the hits - a band riding room tone holds every knob on it slightly
    /// off its slider forever, and a gate is how the quiet parts get to be
    /// quiet. Above the threshold the output isn't the raw value but a
    /// smoothstep of it across what's left of the range, 0 at the cross
    /// and 1 at full scale, so clearing the gate hands over nothing rather
    /// than a jump, and a level hovering right on it ripples instead of
    /// strobing. On a trigger source this is the fire level instead of a
    /// gate: the pulse fires the moment the band reaches it.
    pub threshold: f32,
    /// Aggregates only: drain back to zero when the track changes, so a
    /// phase doesn't carry a song's worth of accumulation into the next
    /// one. A drain rather than a snap, since a shader riding the phase
    /// would pop on a jump.
    pub reset_on_track: bool,
}

impl Default for Signal {
    fn default() -> Self {
        Signal {
            id: 0,
            name: String::new(),
            source: Source::Band {
                lo: 30.0,
                hi: 120.0,
            },
            smooth: 0.3,
            threshold: 0.0,
            reset_on_track: false,
        }
    }
}

impl Signal {
    fn smooth(&self) -> f32 {
        self.smooth.clamp(0.0, 1.0)
    }

    /// What this aggregates and how fast, or None for a spectral source.
    pub fn aggregate(&self) -> Option<(u64, f32)> {
        match self.source {
            Source::Aggregate { of, rate } => Some((of, rate.clamp(0.0, AGGREGATE_RATE_MAX))),
            _ => None,
        }
    }

    pub fn threshold(&self) -> f32 {
        self.threshold.clamp(0.0, 1.0)
    }

    /// The gate's transfer: what leaves for a running value. Ungated
    /// passes exactly, so a signal nobody thresholded costs nothing and
    /// loses nothing. Gated remaps the span above the threshold to the
    /// whole output, through a smoothstep so both ends land flat: the
    /// cross hands over zero rather than a jump, and full scale still
    /// reads as full. Triggers step around it entirely: their threshold
    /// is the fire level and their pulse leaves whole. A pure curve of
    /// the value, no state, which is what
    /// lets the value's own smoothing be the only clock involved.
    pub fn gated(&self, value: f32) -> f32 {
        let threshold = self.threshold();
        if threshold <= 0.0 {
            return value;
        }
        // The span floor keeps a threshold parked at 1.0 a switch rather
        // than a divide by zero.
        let span = (1.0 - threshold).max(1e-3);
        let x = ((value - threshold) / span).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }

    /// The picker's face for this signal: the given name, or a label
    /// derived from the source when none was given, so the pool needs no
    /// naming ceremony and an unnamed signal still reads as itself.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        fn hz(v: f32) -> String {
            if v >= 1000.0 {
                format!("{:.1}k", v / 1000.0)
            } else {
                format!("{v:.0}")
            }
        }
        match self.source {
            Source::Band { lo, hi } => format!("Band {} - {} Hz", hz(lo), hz(hi)),
            Source::Onset { lo, hi } => format!("Onset {} - {} Hz", hz(lo), hz(hi)),
            Source::Trigger { lo, hi } => format!("Trigger {} - {} Hz", hz(lo), hz(hi)),
            Source::Level => "Level".to_string(),
            // What it follows can't be named from here without the pool,
            // so the rate is what distinguishes two of them at a glance.
            // Anything more wants a name typed in, which is what names are
            // for.
            Source::Aggregate { rate, .. } => format!("Aggregate {rate:.2}/s"),
        }
    }
}

/// One attachment of a signal to a parameter. `from`/`to` are fractions of
/// the target parameter's own range - the value at silence and the value
/// at full signal - so a route sweeps exactly what a hand on the slider
/// could, and an inverted span modulates downward. Unknown target ids and
/// missing signals are skipped, so configs degrade quietly.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Route {
    /// Whether the route applies. Off keeps it in place, tuned, silent.
    pub enabled: bool,
    /// The pool signal this rides, by id.
    pub signal: u64,
    /// The parameter this drives: an id the host panel defines.
    pub target: String,
    /// The output span, fractions of the target's range.
    pub from: f32,
    pub to: f32,
}

impl Default for Route {
    fn default() -> Self {
        Route {
            enabled: true,
            signal: 0,
            target: String::new(),
            from: 0.0,
            to: 1.0,
        }
    }
}

/// The per-second easing rates the smoothing knob spans, snappy end and
/// floaty end, interpolated exponentially so the knob's travel feels even.
const ATTACK_FAST: f32 = 50.0;
const ATTACK_SLOW: f32 = 3.0;
const RELEASE_FAST: f32 = 12.0;
const RELEASE_SLOW: f32 = 1.0;

/// The onset detector's shape: the pulse decay rates the response knob
/// spans (shared with the trigger, whose pulse rings the same way), how
/// fast the reference envelope chases the band, how far past the
/// reference the band must jump to read as a hit, and the level below
/// which nothing counts, so the noise floor can't fire it.
const ONSET_DECAY_FAST: f32 = 16.0;
const ONSET_DECAY_SLOW: f32 = 1.5;
const ONSET_REF_ATTACK: f32 = 2.5;
const ONSET_REF_RELEASE: f32 = 2.0;
const ONSET_MARGIN: f32 = 0.12;
const ONSET_FLOOR: f32 = 0.15;

/// The trigger's hysteresis: the fraction of the fire level the band must
/// fall back under before the trigger can fire again. Without it a level
/// rippling across the line machine-guns; with it each hit is one pulse,
/// because the band dips between hits relative to the line the user drew.
const TRIGGER_REARM: f32 = 0.75;

/// Where a falling value stops counting as motion. An exponential release
/// never actually lands on zero, so the tail needs a floor to end at or a
/// surface drawing it out would never park.
const SETTLED: f32 = 0.004;

/// How fast a flushed aggregate falls back to zero, and how near zero ends
/// the fall. Quick enough to read as the cycle collapsing rather than as a
/// slow fade, slow enough that a shader riding the phase doesn't pop.
const FLUSH_DRAIN: f32 = 8.0;
const FLUSH_DONE: f32 = 0.002;

/// One signal's running state in the engine.
#[derive(Clone, Copy)]
struct Slot {
    value: f32,
    /// The onset detector's slow reference envelope; idle otherwise.
    reference: f32,
    /// Whether an onset or trigger slot is ready to fire again.
    armed: bool,
    /// An aggregate on its way back to zero: set by a flush or a track
    /// change, cleared when it lands. Accumulation pauses while it drains,
    /// so a flush during a loud passage still gets there.
    draining: bool,
    /// What actually leaves the slot: the value through its signal's gate
    /// curve, or on a trigger the pulse itself, where the value is the
    /// band the fire level judges. Written on the tick rather than
    /// derived at read time only because the readers don't carry the
    /// pool; for a trigger it's real state, the ringing pulse.
    output: f32,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            value: 0.0,
            reference: 0.0,
            armed: true,
            draining: false,
            output: 0.0,
        }
    }
}

/// The engine: one smoothed value per pool signal, keyed by id so edits,
/// insertions, and removals never shuffle another signal's state.
pub struct Signals {
    slots: HashMap<u64, Slot>,
}

impl Signals {
    pub fn new() -> Self {
        Signals {
            slots: HashMap::new(),
        }
    }

    /// The signal's running value before its gate, `None` for an id the
    /// pool doesn't carry, which is what lets routes to deleted signals
    /// skip quietly.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.slots.get(&id).map(|slot| slot.value)
    }

    /// What actually leaves the signal: the running value through its
    /// gate curve. Everything downstream reads this; [`Signals::value`]
    /// is for the meter, which draws what the gate is holding back.
    pub fn output(&self, id: u64) -> Option<f32> {
        self.slots.get(&id).map(|slot| slot.output)
    }

    /// Whether anything in the pool is still on its way down. An aggregate
    /// parks wherever its phase stopped and never falls, so it only counts
    /// while it's draining; everything else releases toward zero, and the
    /// release is exactly what a consumer has to keep drawing through.
    pub fn settling(&self, pool: &[Signal]) -> bool {
        pool.iter().any(|signal| {
            let Some(slot) = self.slots.get(&signal.id) else {
                return false;
            };
            if signal.aggregate().is_some() {
                return slot.draining;
            }
            slot.value > SETTLED || slot.output > SETTLED
        })
    }

    /// Fold one frame into the signals. `mags` is the newest half-spectrum
    /// when a fresh window landed this frame, `None` between windows, where
    /// values hold rather than dip; `stopped` releases everything toward
    /// zero once the feed has actually gone quiet.
    pub fn step(
        &mut self,
        mags: Option<&[f32]>,
        sample_rate: u32,
        stopped: bool,
        dt: f32,
        pool: &[Signal],
    ) {
        self.slots.retain(|id, _| pool.iter().any(|s| s.id == *id));
        for signal in pool {
            if signal.aggregate().is_some() {
                continue;
            }
            let slot = self.slots.entry(signal.id).or_default();
            let smooth = signal.smooth();
            let raw = mags.map(|mags| {
                let (lo, hi) = signal.source.bins(sample_rate, mags.len());
                let mut peak = 0.0f32;
                for &m in &mags[lo..hi] {
                    peak = peak.max(m);
                }
                let db = 20.0 * (peak + 1e-9).log10();
                ((db - FLOOR_DB) / (MAX_DB - FLOOR_DB)).clamp(0.0, 1.0)
            });
            match signal.source {
                Source::Band { .. } | Source::Level => {
                    let attack = ATTACK_FAST * (ATTACK_SLOW / ATTACK_FAST).powf(smooth);
                    let release = RELEASE_FAST * (RELEASE_SLOW / RELEASE_FAST).powf(smooth);
                    if let Some(raw) = raw {
                        let ease = if raw > slot.value { attack } else { release };
                        slot.value += (raw - slot.value) * (ease * dt).min(1.0);
                    } else if stopped {
                        slot.value += (0.0 - slot.value) * (release * dt).min(1.0);
                    }
                }
                Source::Onset { .. } => {
                    // The pulse decays on every frame; the trigger reads
                    // against the reference before the reference catches
                    // up, so a jump lands whole.
                    let decay =
                        ONSET_DECAY_FAST * (ONSET_DECAY_SLOW / ONSET_DECAY_FAST).powf(smooth);
                    slot.value -= slot.value * (decay * dt).min(1.0);
                    if let Some(raw) = raw {
                        let reference = slot.reference;
                        if slot.armed && raw > ONSET_FLOOR && raw > reference + ONSET_MARGIN {
                            slot.value = 1.0;
                            slot.armed = false;
                        } else if !slot.armed && raw < reference + ONSET_MARGIN * 0.5 {
                            slot.armed = true;
                        }
                        let ease = if raw > reference {
                            ONSET_REF_ATTACK
                        } else {
                            ONSET_REF_RELEASE
                        };
                        slot.reference += (raw - reference) * (ease * dt).min(1.0);
                    } else if stopped {
                        slot.reference -= slot.reference * (ONSET_REF_RELEASE * dt).min(1.0);
                        slot.armed = true;
                    }
                }
                Source::Trigger { .. } => {
                    // The pulse lives in the output and rings down every
                    // frame; the value stays the band itself, so the
                    // meter shows the level the fire line is judging and
                    // the mark reads as "fires here". No threshold set
                    // means nothing to cross, so the trigger idles.
                    let decay =
                        ONSET_DECAY_FAST * (ONSET_DECAY_SLOW / ONSET_DECAY_FAST).powf(smooth);
                    slot.output -= slot.output * (decay * dt).min(1.0);
                    if let Some(raw) = raw {
                        slot.value = raw;
                        let threshold = signal.threshold();
                        if threshold > 0.0 {
                            if slot.armed && raw >= threshold {
                                slot.output = 1.0;
                                slot.armed = false;
                            } else if !slot.armed && raw < threshold * TRIGGER_REARM {
                                slot.armed = true;
                            }
                        }
                    } else if stopped {
                        slot.value -= slot.value * (RELEASE_FAST * dt).min(1.0);
                        slot.armed = true;
                    }
                }
                // Handled in the second pass, which needs the values the
                // first one just wrote.
                Source::Aggregate { .. } => {}
            }
        }
        // Second pass: the gates, over every slot the first pass just
        // moved. Ahead of the aggregates so a total integrates what its
        // source is actually putting out this frame rather than last
        // frame's opening.
        for signal in pool {
            let Some(slot) = self.slots.get_mut(&signal.id) else {
                continue;
            };
            // A trigger's output is its pulse, written in the first pass;
            // its threshold is the fire level, not a gate, so the curve
            // would eat the ringing tail the moment it dropped under.
            if matches!(signal.source, Source::Trigger { .. }) {
                continue;
            }
            slot.output = signal.gated(slot.value);
        }
        // Third pass: the aggregates, reading what the sources landed on
        // this frame. An aggregate pointed at another aggregate reads last
        // frame's value instead, which is what keeps a chain (or a ring)
        // from being an ordering problem or a hang.
        for signal in pool {
            let Some((of, rate)) = signal.aggregate() else {
                continue;
            };
            let input = self
                .slots
                .get(&of)
                .filter(|_| of != signal.id)
                .map_or(0.0, |slot| slot.output);
            let slot = self.slots.entry(signal.id).or_default();
            if slot.draining {
                slot.value -= slot.value * (FLUSH_DRAIN * dt).min(1.0);
                if slot.value <= FLUSH_DONE {
                    slot.value = 0.0;
                    slot.draining = false;
                }
            } else {
                // Wrapped rather than grown: a phase keeps every bit of
                // its precision however long the app is up, and a shader
                // reading it through a sine runs straight across the seam.
                slot.value = (slot.value + input * rate * dt).fract();
            }
            // Behind the value move, so the total's own output is this
            // frame's rather than the gate pass's stale read.
            slot.output = signal.gated(slot.value);
        }
    }

    /// Send one aggregate back to zero, over the drain rather than at
    /// once. A signal that's already there, or isn't an aggregate at all,
    /// takes it as a no-op: the spectral sources rewrite their value every
    /// frame regardless.
    pub fn flush(&mut self, id: u64) {
        if let Some(slot) = self.slots.get_mut(&id) {
            slot.draining = slot.value > FLUSH_DONE;
            if !slot.draining {
                slot.value = 0.0;
            }
        }
    }
}

impl Default for Signals {
    fn default() -> Self {
        Self::new()
    }
}

/// The app-wide pool and its engine behind one lock: panels tick it from
/// their paint (the first call per frame does the work, the rest read),
/// edit it from their settings surfaces, and the app persists whatever
/// [`SignalHub::pool`] returns. Shared by `Arc` in the app state, so a
/// tray adoption carries it the way it carries the player.
pub struct SignalHub {
    inner: Mutex<Hub>,
}

struct Hub {
    pool: Vec<Signal>,
    engine: Signals,
    analyzer: Option<Analyzer>,
    mono: Vec<f32>,
    last_written: u64,
    last_fresh: Option<Instant>,
    last_tick: Option<Instant>,
    /// The last track the tickers reported, for the aggregates that reset
    /// between songs. Only ever holds a real id: the gap between two
    /// tracks reads as nothing playing, and treating that as a change
    /// would flush twice on every advance.
    last_track: Option<u64>,
}

impl SignalHub {
    pub fn new(pool: Vec<Signal>) -> Self {
        SignalHub {
            inner: Mutex::new(Hub {
                pool,
                engine: Signals::new(),
                analyzer: None,
                mono: Vec::new(),
                last_written: 0,
                last_fresh: None,
                last_tick: None,
                last_track: None,
            }),
        }
    }

    /// Advance the engine one frame off the feed. Cheap to call from every
    /// consumer: calls landing within the same frame window return
    /// immediately, so the clock only moves once however many panels ask.
    ///
    /// `track` is what's playing, so the hub can see a song change for the
    /// aggregates that reset on one. Every ticker passes it rather than
    /// one privileged caller owning the edge: whichever surface happens to
    /// be painting has to be the one that notices.
    pub fn tick(&self, feed: &AudioFeed, track: Option<u64>) {
        let mut hub = self.inner.lock().unwrap();
        // Ahead of the throttle below, so a change never rides on which
        // caller won the frame.
        if let Some(track) = track {
            if hub.last_track.replace(track) != Some(track) {
                let ids: Vec<u64> = hub
                    .pool
                    .iter()
                    .filter(|s| s.reset_on_track && s.aggregate().is_some())
                    .map(|s| s.id)
                    .collect();
                for id in ids {
                    hub.engine.flush(id);
                }
            }
        }
        let now = Instant::now();
        let dt = match hub.last_tick {
            Some(t) => {
                let dt = (now - t).as_secs_f32();
                if dt < TICK_MIN {
                    return;
                }
                dt.min(0.1)
            }
            None => 1.0 / 60.0,
        };
        hub.last_tick = Some(now);

        let written = feed.written();
        let fresh = written != hub.last_written;
        hub.last_written = written;
        if fresh {
            hub.last_fresh = Some(now);
        }
        let stopped = hub
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        if hub.analyzer.is_none() {
            hub.analyzer = Some(Analyzer::new(HUB_FFT));
            hub.mono = vec![0.0; HUB_FFT];
        }
        let rate = feed.sample_rate();
        let Hub {
            pool,
            engine,
            analyzer,
            mono,
            ..
        } = &mut *hub;
        let analyzer = analyzer.as_mut().expect("analyzer built above");
        let mags: Option<&[f32]> = if fresh && feed.latest_mono(mono) == mono.len() {
            Some(analyzer.magnitudes(mono))
        } else {
            None
        };
        engine.step(mags, rate, stopped, dt, pool);
    }

    /// The signal's current value with its gate applied, `None` for an id
    /// the pool doesn't carry. Everything riding a signal reads it through
    /// here, so the gate lands on routes, meters and the shader alike.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.inner.lock().unwrap().engine.output(id)
    }

    /// The value before the gate, for the meter that draws the threshold as
    /// a mark across it: a readout that only ever showed the gated value
    /// would sit at nothing under the mark, which is the one place the gate
    /// wants watching.
    pub fn raw_value(&self, id: u64) -> Option<f32> {
        self.inner.lock().unwrap().engine.value(id)
    }

    /// Send one aggregate back to zero by hand, the debugging way out of
    /// "what is this phase actually at". Drains rather than snaps, so
    /// pressing it while a shader rides the phase doesn't tear the frame.
    pub fn flush(&self, id: u64) {
        self.inner.lock().unwrap().engine.flush(id);
    }

    /// Whether the pool still has a falling tail in it once the feed has
    /// gone quiet. [`SignalHub::live`] goes false the moment the audio
    /// stops, which is well before a smoothed signal has finished
    /// releasing, so anything that stops drawing on `!live` freezes the
    /// fade partway down instead of playing it out.
    pub fn settling(&self) -> bool {
        let hub = self.inner.lock().unwrap();
        hub.engine.settling(&hub.pool)
    }

    /// Whether audio has moved recently enough that meters reading the hub
    /// should keep asking for frames.
    pub fn live(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .last_fresh
            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.3)
    }

    /// A copy of the pool, for pickers and for persisting.
    pub fn pool(&self) -> Vec<Signal> {
        self.inner.lock().unwrap().pool.clone()
    }

    /// Replace the pool wholesale: what a workspace apply or a settings
    /// load does. Engine state for surviving ids carries over.
    pub fn set_pool(&self, pool: Vec<Signal>) {
        self.inner.lock().unwrap().pool = pool;
    }

    /// Edit the pool in place and get the result back for persisting.
    pub fn edit(&self, edit: impl FnOnce(&mut Vec<Signal>)) -> Vec<Signal> {
        let mut hub = self.inner.lock().unwrap();
        edit(&mut hub.pool);
        hub.pool.clone()
    }

    /// Add a signal and return its fresh id along with the pool to
    /// persist.
    pub fn add(&self, source: Source, smooth: f32) -> (u64, Vec<Signal>) {
        let mut hub = self.inner.lock().unwrap();
        let id = hub.pool.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        hub.pool.push(Signal {
            id,
            source,
            smooth,
            ..Signal::default()
        });
        (id, hub.pool.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(id: u64, lo: f32, hi: f32) -> Signal {
        Signal {
            id,
            source: Source::Band { lo, hi },
            smooth: 0.0,
            ..Signal::default()
        }
    }

    #[test]
    fn loud_band_rises_quiet_band_stays_down() {
        let mut engine = Signals::new();
        // Energy in bin 100 of a 2048-bin half-spectrum at 48 kHz: about
        // 1.17 kHz. A midrange signal should light up, a top-end one not.
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![band(1, 800.0, 2000.0), band(2, 8000.0, 16000.0)];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert!(engine.value(1).unwrap() > 0.9);
        assert!(engine.value(2).unwrap() < 0.05);
        // An id the pool never carried resolves to nothing.
        assert!(engine.value(99).is_none());
    }

    #[test]
    fn holds_between_windows_and_releases_when_stopped() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![band(1, 800.0, 2000.0)];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        engine.step(None, 48_000, false, 0.016, &pool);
        assert!(engine.value(1).unwrap() > 0.9, "gap should hold");
        for _ in 0..120 {
            engine.step(None, 48_000, true, 0.016, &pool);
        }
        assert!(engine.value(1).unwrap() < 0.05, "stop should release");
    }

    /// The release is motion, and a surface riding a signal has to keep
    /// drawing for as long as it lasts. Without this the fade a smoothed
    /// signal exists to give you is the one thing nobody ever sees: the
    /// audio stops, the frames stop with it, and the effect freezes at
    /// whatever the last live push happened to carry.
    #[test]
    fn a_falling_signal_reads_as_settling_until_it_lands() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        // Smoothed hard, the way a presence envelope is: a long tail is
        // exactly the case where parking early shows.
        let pool = vec![Signal {
            smooth: 0.85,
            ..band(1, 800.0, 2000.0)
        }];
        for _ in 0..60 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert!(engine.settling(&pool), "a signal that's up is still motion");

        // A second in, the tail is well under way and nowhere near done.
        for _ in 0..60 {
            engine.step(None, 48_000, true, 0.016, &pool);
        }
        assert!(
            engine.value(1).unwrap() < 0.5,
            "the release should be falling"
        );
        assert!(engine.settling(&pool), "and it isn't down yet");

        for _ in 0..600 {
            engine.step(None, 48_000, true, 0.016, &pool);
        }
        assert!(!engine.settling(&pool), "a landed signal parks");

        // An aggregate holds its phase wherever the music left it, so it
        // never reads as motion; treating a parked phase as a falling tail
        // would keep the frames coming for as long as the app is up.
        let pool = vec![
            Signal {
                smooth: 0.85,
                ..band(1, 800.0, 2000.0)
            },
            Signal {
                id: 2,
                source: Source::Aggregate { of: 1, rate: 0.5 },
                ..Signal::default()
            },
        ];
        for _ in 0..60 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        for _ in 0..600 {
            engine.step(None, 48_000, true, 0.016, &pool);
        }
        assert!(
            engine.value(2).unwrap() > 0.0,
            "the phase stays where it stopped"
        );
        assert!(!engine.settling(&pool), "and a parked phase isn't motion");
    }

    #[test]
    fn onset_fires_once_decays_and_rearms() {
        let mut engine = Signals::new();
        let pool = vec![Signal {
            id: 7,
            source: Source::Onset {
                lo: 800.0,
                hi: 2000.0,
            },
            smooth: 0.0,
            ..Signal::default()
        }];
        let quiet = vec![0.0f32; 2048];
        let mut loud = vec![0.0f32; 2048];
        loud[100] = 1.0;

        engine.step(Some(&quiet), 48_000, false, 0.016, &pool);
        engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        assert!(engine.value(7).unwrap() > 0.9, "onset should pulse");

        for _ in 0..60 {
            engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        }
        assert!(
            engine.value(7).unwrap() < 0.1,
            "sustain should not hold the pulse"
        );

        for _ in 0..120 {
            engine.step(Some(&quiet), 48_000, false, 0.016, &pool);
        }
        engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        assert!(engine.value(7).unwrap() > 0.9, "onset should re-arm");
    }

    fn trigger(id: u64, threshold: f32) -> Signal {
        Signal {
            id,
            source: Source::Trigger {
                lo: 800.0,
                hi: 2000.0,
            },
            smooth: 0.0,
            threshold,
            ..Signal::default()
        }
    }

    #[test]
    fn a_trigger_fires_at_its_line_holds_fire_above_it_and_rearms_under_it() {
        let mut engine = Signals::new();
        let pool = vec![trigger(5, 0.5)];
        let quiet = vec![0.0f32; 2048];
        let mut loud = vec![0.0f32; 2048];
        loud[100] = 1.0;

        engine.step(Some(&quiet), 48_000, false, 0.016, &pool);
        engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        assert!(
            engine.output(5).unwrap() > 0.9,
            "crossing the line should fire the pulse"
        );
        assert!(
            engine.value(5).unwrap() > 0.9,
            "the value stays the band, for the meter the line is drawn on"
        );

        // Pinned above the line: the pulse rings down and nothing refires,
        // which is the whole difference from a gate.
        for _ in 0..60 {
            engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        }
        assert!(
            engine.output(5).unwrap() < 0.1,
            "holding above the line should not hold the pulse"
        );

        // Back under the line rearms it, and the next cross fires again.
        for _ in 0..10 {
            engine.step(Some(&quiet), 48_000, false, 0.016, &pool);
        }
        engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        assert!(
            engine.output(5).unwrap() > 0.9,
            "dipping under the line should rearm the trigger"
        );
    }

    #[test]
    fn a_trigger_without_a_line_stays_silent() {
        let mut engine = Signals::new();
        let pool = vec![trigger(5, 0.0)];
        let mut loud = vec![0.0f32; 2048];
        loud[100] = 1.0;
        for _ in 0..30 {
            engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        }
        assert_eq!(
            engine.output(5).unwrap(),
            0.0,
            "no threshold means nothing to cross"
        );
        assert!(
            engine.value(5).unwrap() > 0.9,
            "the band still shows on the meter"
        );
    }

    #[test]
    fn removed_signal_drops_its_state_survivors_keep_theirs() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![band(1, 800.0, 2000.0), band(2, 800.0, 2000.0)];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        let survivor = vec![band(2, 800.0, 2000.0)];
        engine.step(Some(&mags), 48_000, false, 0.016, &survivor);
        assert!(
            engine.value(1).is_none(),
            "removed id should resolve to nothing"
        );
        assert!(engine.value(2).unwrap() > 0.9, "survivor keeps its value");
    }

    #[test]
    fn the_gate_silences_what_sits_under_it_and_leaves_the_engine_alone() {
        // A band with energy well down the dB window: enough to read, not
        // enough to clear a gate set above it.
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 0.02;
        let mut quiet = band(1, 800.0, 2000.0);
        let hub = SignalHub::new(vec![quiet.clone()]);
        let run = |frames: usize| {
            let mut inner = hub.inner.lock().unwrap();
            let Hub { engine, pool, .. } = &mut *inner;
            for _ in 0..frames {
                engine.step(Some(&mags), 48_000, false, 0.016, pool);
            }
        };
        run(30);
        let ungated = hub.raw_value(1).expect("signal is in the pool");
        assert!(
            (0.05..0.6).contains(&ungated),
            "test tone should land mid-window, got {ungated}"
        );
        assert_eq!(hub.value(1), Some(ungated), "no gate lets it all through");

        // Gated above where it sits: nothing leaves, and what the engine
        // holds is untouched, so lifting the gate restores it at once.
        // The curve is stateless, so one frame is the whole story.
        quiet.threshold = ungated + 0.1;
        hub.set_pool(vec![quiet.clone()]);
        run(1);
        assert_eq!(hub.value(1), Some(0.0), "under the gate reads as nothing");
        assert!(
            (hub.raw_value(1).unwrap() - ungated).abs() < 1e-4,
            "the engine keeps its value"
        );

        quiet.threshold = ungated - 0.01;
        hub.set_pool(vec![quiet]);
        run(1);
        let out = hub.value(1).unwrap();
        assert!(
            out > 0.0 && out < ungated * 0.5,
            "just over the gate leaves a whisper, not the whole value, got {out}"
        );
    }

    #[test]
    fn the_gate_ramps_from_nothing_at_the_cross_to_whole_at_full_scale() {
        let signal = Signal {
            threshold: 0.5,
            ..band(1, 800.0, 2000.0)
        };
        assert_eq!(signal.gated(0.5), 0.0, "the cross hands over nothing");
        assert_eq!(signal.gated(1.0), 1.0, "full scale still reads as full");
        let mid = signal.gated(0.75);
        assert!(
            (mid - 0.5).abs() < 1e-4,
            "halfway up the span is halfway out, got {mid}"
        );
        let low = signal.gated(0.55);
        assert!(
            low > 0.0 && low < 0.1,
            "just over the cross eases in rather than jumping, got {low}"
        );
        // And through the engine: a loud band over a mid gate still lands
        // wide open, since the remap tops out where the value does.
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![signal];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        let open = engine.output(1).unwrap();
        assert!(
            open > 0.9,
            "a pinned band should clear the whole ramp, got {open}"
        );
    }

    fn aggregate(id: u64, of: u64, rate: f32) -> Signal {
        Signal {
            id,
            source: Source::Aggregate { of, rate },
            ..Signal::default()
        }
    }

    #[test]
    fn an_aggregate_climbs_with_its_source_and_wraps_instead_of_growing() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        // Rate 2/s over a source pinned near 1: a full wrap every half
        // second, so a second of frames laps twice and lands mid-ramp
        // rather than at 2.0.
        let pool = vec![band(1, 800.0, 2000.0), aggregate(2, 1, 2.0)];
        for _ in 0..10 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        let early = engine.value(2).unwrap();
        assert!(early > 0.0, "an aggregate over a live source should climb");
        for _ in 0..60 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        let later = engine.value(2).unwrap();
        assert!(
            (0.0..1.0).contains(&later),
            "a wrapped phase never leaves 0..1, got {later}"
        );
        // Silence stalls it: the source releases to nothing and the total
        // stops moving rather than drifting on.
        let quiet = vec![0.0f32; 2048];
        for _ in 0..120 {
            engine.step(Some(&quiet), 48_000, true, 0.016, &pool);
        }
        let parked = engine.value(2).unwrap();
        for _ in 0..60 {
            engine.step(Some(&quiet), 48_000, true, 0.016, &pool);
        }
        assert!(
            (engine.value(2).unwrap() - parked).abs() < 1e-4,
            "silence should stall the total, not advance it"
        );
    }

    #[test]
    fn a_flushed_aggregate_drains_to_zero_rather_than_snapping() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![band(1, 800.0, 2000.0), aggregate(2, 1, 1.0)];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert!(engine.value(2).unwrap() > 0.1, "something to flush");
        engine.flush(2);
        engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        let mid = engine.value(2).unwrap();
        assert!(
            mid > 0.0,
            "the first frame after a flush should still be on its way down"
        );
        for _ in 0..60 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        // Landed, and accumulating again: the drain releases the slot
        // rather than pinning it at zero.
        let after = engine.value(2).unwrap();
        assert!(after > 0.0 && after < mid, "it should resume from zero");
    }

    #[test]
    fn an_aggregate_over_a_missing_or_circular_source_stays_put() {
        let mut engine = Signals::new();
        let mags = vec![0.0f32; 2048];
        // One pointed at an id nobody carries, and a pair pointed at each
        // other. Neither should hang or panic; the ring reads last frame's
        // values, which are zero, so nothing climbs.
        let pool = vec![
            aggregate(1, 99, 1.0),
            aggregate(2, 3, 1.0),
            aggregate(3, 2, 1.0),
        ];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert_eq!(engine.value(1), Some(0.0));
        assert_eq!(engine.value(2), Some(0.0));
        assert_eq!(engine.value(3), Some(0.0));
    }

    #[test]
    fn inverted_band_from_a_hand_edited_file_does_not_panic() {
        let mut engine = Signals::new();
        let mags = vec![0.0f32; 1024];
        let pool = vec![band(1, 5000.0, 40.0), band(2, -10.0, 1e9)];
        engine.step(Some(&mags), 48_000, false, 0.016, &pool);
    }

    /// The Critters bundle ships a trigger, so the tag on disk is a
    /// contract: this exact JSON has to keep parsing as a trigger.
    #[test]
    fn trigger_json_round_trips_unchanged() {
        let old = r#"{"kind":"trigger","lo":35.0,"hi":130.0}"#;
        let source: Source = serde_json::from_str(old).unwrap();
        assert!(matches!(
            source,
            Source::Trigger { lo, hi } if lo == 35.0 && hi == 130.0
        ));
        assert_eq!(serde_json::to_string(&source).unwrap(), old);
    }

    /// A route on disk has to come back as exactly what it was, byte for
    /// byte, or every saved layout and settings file drifts on load.
    #[test]
    fn route_json_round_trips_unchanged() {
        let old = r#"{"enabled":true,"signal":7,"target":"slot3","from":0.25,"to":1.5}"#;
        let route: Route = serde_json::from_str(old).unwrap();
        assert!(route.enabled);
        assert_eq!(route.signal, 7);
        assert_eq!(route.target, "slot3");
        assert_eq!((route.from, route.to), (0.25, 1.5));
        assert_eq!(serde_json::to_string(&route).unwrap(), old);

        // A partial route, the other shape a hand-edited file takes.
        let sparse: Route = serde_json::from_str(r#"{"target":"slot0"}"#).unwrap();
        assert_eq!(sparse.signal, 0);
    }

    #[test]
    fn hub_add_allocates_fresh_ids_and_labels_derive() {
        let hub = SignalHub::new(Vec::new());
        let (a, _) = hub.add(Source::Level, 0.3);
        let (b, pool) = hub.add(
            Source::Band {
                lo: 30.0,
                hi: 1500.0,
            },
            0.3,
        );
        assert_ne!(a, b);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool[0].label(), "Level");
        assert_eq!(pool[1].label(), "Band 30 - 1.5k Hz");
        // A given name takes over; clearing it falls back to the derived
        // label.
        let named = hub.edit(|pool| pool[0].name = "Mix swell".to_string());
        assert_eq!(named[0].label(), "Mix swell");
    }
}
