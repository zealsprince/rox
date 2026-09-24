//! The shared modulation layer: named signals over the playback spectrum that
//! any parameter can bind to. A [`Signal`] is one source (band energy, mix
//! level, onset, threshold trigger, or a running total of another signal) with
//! its smoothing and gate; a [`Route`] attaches a signal to a host-defined
//! parameter with an output span. A [`SignalHub`] evaluates the pool off the
//! shared [`crate::AudioFeed`], once per frame, and reading it is what moves
//! it. Target ids and units belong to the host. Missing signals and unrouted
//! signals degrade quietly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::AudioFeed;
use crate::analysis::log_bands;

/// dB window signals normalize into (full-scale sine = 0 dB), the same one the
/// spectrum bars use.
pub const FLOOR_DB: f32 = -66.0;
pub const MAX_DB: f32 = -12.0;

pub const BAND_MIN_HZ: f32 = 20.0;
pub const BAND_MAX_HZ: f32 = 20_000.0;

/// Signals pool whole bands, so a short reactive window beats a fine one.
const HUB_FFT: usize = 2048;

/// Silence longer than this reads as stopped audio rather than the gap between
/// pump ticks, so signals don't strobe on high-refresh displays.
const SILENT_AFTER: f32 = 0.15;

/// Advances closer than this are one frame asking twice: several panels read
/// the hub from their paint, and only the first should move the clock.
const TICK_MIN: f32 = 0.003;

/// What a signal listens to.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum Source {
    /// Peak magnitude across a band, normalized into the dB window.
    Band { lo: f32, hi: f32 },
    /// The whole spectrum's peak.
    Level,
    /// A pulse when the band jumps past its own recent average: a hit, where Band is a swell.
    Onset { lo: f32, hi: f32 },
    /// A pulse when the band crosses the threshold, re-armed once it falls back
    /// under. Fires on a kick over sustained sub, where Onset arms once and goes quiet.
    Trigger { lo: f32, hi: f32 },
    /// A running total of another signal, wrapping at 1: a phase a shader reads
    /// as music-driven time, keeping its precision however long the app runs.
    Aggregate { of: u64, rate: f32 },
}

impl Source {
    /// Clamped so a hand-edited file can't invert the band or leave the spectrum.
    fn bins(&self, sample_rate: u32, half: usize) -> (usize, usize) {
        let (lo, hi) = match *self {
            Source::Band { lo, hi } | Source::Onset { lo, hi } | Source::Trigger { lo, hi } => {
                let lo = lo.clamp(BAND_MIN_HZ, BAND_MAX_HZ);
                (lo, hi.clamp(lo * 1.01, BAND_MAX_HZ))
            }
            // Aggregates never ask for bins.
            Source::Level | Source::Aggregate { .. } => (BAND_MIN_HZ, BAND_MAX_HZ),
        };
        log_bands(1, lo, hi, sample_rate, half)[0]
    }
}

/// Wraps per second at full input, so a hand-edited file can't make a phase
/// lap several times a frame.
pub const AGGREGATE_RATE_MAX: f32 = 8.0;

/// One shared signal in the pool.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Signal {
    pub id: u64,
    /// The user's name for it. Empty uses the label derived from the source.
    pub name: String,
    pub source: Source,
    /// Smoothing, 0 snaps to the music and 1 drifts. On onset and trigger, the pulse decay.
    pub smooth: f32,
    /// The gate, 0 to 1 (0 is off): output under it is nothing, and above it
    /// eases in through a smoothstep. On a trigger, the fire level instead.
    pub threshold: f32,
    /// Aggregates only: drain to zero on a track change.
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

    pub fn aggregate(&self) -> Option<(u64, f32)> {
        match self.source {
            Source::Aggregate { of, rate } => Some((of, rate.clamp(0.0, AGGREGATE_RATE_MAX))),
            _ => None,
        }
    }

    pub fn threshold(&self) -> f32 {
        self.threshold.clamp(0.0, 1.0)
    }

    /// The gate's transfer. Ungated passes exactly; gated remaps the span above
    /// the threshold through a smoothstep, so crossing it never jumps. Stateless.
    /// Triggers don't use it: their threshold is the fire level.
    pub fn gated(&self, value: f32) -> f32 {
        let threshold = self.threshold();
        if threshold <= 0.0 {
            return value;
        }
        // The floor keeps a threshold at 1.0 a switch, not a divide by zero.
        let span = (1.0 - threshold).max(1e-3);
        let x = ((value - threshold) / span).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }

    /// The given name, or a label derived from the source.
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
            // Can't name the followed signal without the pool; the rate tells two apart.
            Source::Aggregate { rate, .. } => format!("Aggregate {rate:.2}/s"),
        }
    }
}

/// One signal driving one parameter. Unknown targets and missing signals are skipped.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Route {
    /// Off keeps it in place, tuned, silent.
    pub enabled: bool,
    /// The pool signal this route reads.
    pub signal: u64,
    /// The parameter this drives, an id the host panel defines.
    pub target: String,
    /// The span, as fractions of the target's range: value at silence, value at
    /// full signal. Inverted modulates downward.
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

/// Easing rates the smoothing knob spans, interpolated exponentially.
const ATTACK_FAST: f32 = 50.0;
const ATTACK_SLOW: f32 = 3.0;
const RELEASE_FAST: f32 = 12.0;
const RELEASE_SLOW: f32 = 1.0;

/// Onset detector: pulse decay range (shared with the trigger), reference
/// envelope rates, the jump that counts as a hit, and the noise floor.
const ONSET_DECAY_FAST: f32 = 16.0;
const ONSET_DECAY_SLOW: f32 = 1.5;
const ONSET_REF_ATTACK: f32 = 2.5;
const ONSET_REF_RELEASE: f32 = 2.0;
const ONSET_MARGIN: f32 = 0.12;
const ONSET_FLOOR: f32 = 0.15;

/// Trigger hysteresis: the band must fall under this share of the fire level
/// before it can fire again, or a level rippling across the line machine-guns.
const TRIGGER_REARM: f32 = 0.75;

/// Where a release stops counting as motion; an exponential never reaches zero.
const SETTLED: f32 = 0.004;

/// Drain rate and end point for a flushed aggregate: fast, but no pop.
const FLUSH_DRAIN: f32 = 8.0;
const FLUSH_DONE: f32 = 0.002;

#[derive(Clone, Copy)]
struct Slot {
    value: f32,
    reference: f32,
    armed: bool,
    /// A flushed aggregate on its way to zero; accumulation pauses meanwhile.
    draining: bool,
    /// What leaves the slot: the gated value, or a trigger's ringing pulse.
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

/// One value per pool signal, keyed by id so edits never shuffle another's state.
pub struct Signals {
    slots: HashMap<u64, Slot>,
}

impl Signals {
    pub fn new() -> Self {
        Signals {
            slots: HashMap::new(),
        }
    }

    /// The value before the gate, `None` for an id the pool doesn't have.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.slots.get(&id).map(|slot| slot.value)
    }

    /// The value through its gate. Everything reads this; [`Signals::value`] is
    /// for the meter.
    pub fn output(&self, id: u64) -> Option<f32> {
        self.slots.get(&id).map(|slot| slot.output)
    }

    /// Whether anything is still releasing. A parked aggregate only counts while
    /// draining.
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

    /// Fold one frame in. `mags` is `None` between windows, where values hold;
    /// `stopped` releases everything once the feed has gone quiet.
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
                    // Test against the reference before it catches up, so a jump registers whole.
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
                    // The pulse lives in the output; the value stays the band, for the meter.
                    // No threshold, no trigger.
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
                // Second pass, below.
                Source::Aggregate { .. } => {}
            }
        }
        // Gates, ahead of the aggregates so a total integrates this frame's output.
        for signal in pool {
            let Some(slot) = self.slots.get_mut(&signal.id) else {
                continue;
            };
            // A trigger's threshold is its fire level, not a gate.
            if matches!(signal.source, Source::Trigger { .. }) {
                continue;
            }
            slot.output = signal.gated(slot.value);
        }
        // Aggregates last. One pointed at another aggregate reads last frame's
        // value, so chains and rings can't hang.
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
                // Wrapped, not grown, to keep precision.
                slot.value = (slot.value + input * rate * dt).fract();
            }
            slot.output = signal.gated(slot.value);
        }
    }

    /// Drain one aggregate to zero. A no-op for spectral sources.
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

/// The app-wide pool and its engine behind one lock, shared by `Arc` so a tray
/// adoption keeps it. Every read advances the engine off the feed, deduped to
/// once per frame by [`TICK_MIN`], so there's no tick for a consumer to
/// forget. [`SignalHub::unfed`] is the one hub without a feed, for a pool
/// editor with no player; its reads come back empty.
pub struct SignalHub {
    inner: Mutex<Hub>,
    feed: Option<Arc<AudioFeed>>,
}

struct Hub {
    pool: Vec<Signal>,
    engine: Signals,
    last_written: u64,
    last_fresh: Option<Instant>,
    last_tick: Option<Instant>,
    /// Only ever a real id: the gap between tracks isn't a change, or every
    /// advance would flush twice.
    last_track: Option<u64>,
}

impl SignalHub {
    /// Private so a hub that never moves only comes from [`SignalHub::unfed`].
    /// Tests use it to step the engine by hand.
    fn new(pool: Vec<Signal>) -> Self {
        SignalHub {
            inner: Mutex::new(Hub {
                pool,
                engine: Signals::new(),
                last_written: 0,
                last_fresh: None,
                last_tick: None,
                last_track: None,
            }),
            feed: None,
        }
    }

    /// Every read advances it. The app builds one per player.
    pub fn with_feed(pool: Vec<Signal>, feed: Arc<AudioFeed>) -> Self {
        SignalHub {
            feed: Some(feed),
            ..SignalHub::new(pool)
        }
    }

    /// For the signals window with no workspace up: the pool can be edited and
    /// persisted, but nothing ever steps, so every read is `None`.
    pub fn unfed(pool: Vec<Signal>) -> Self {
        SignalHub::new(pool)
    }

    fn advanced(&self) -> MutexGuard<'_, Hub> {
        let mut hub = self.inner.lock().unwrap();
        if let Some(feed) = &self.feed {
            hub.advance(feed);
        }
        hub
    }
}

impl Hub {
    /// Calls within the same frame window return at once.
    fn advance(&mut self, feed: &AudioFeed) {
        // The song-change edge, ahead of the throttle so a change never waits.
        if let Some(track) = feed.track()
            && self.last_track.replace(track) != Some(track)
        {
            let ids: Vec<u64> = self
                .pool
                .iter()
                .filter(|s| s.reset_on_track && s.aggregate().is_some())
                .map(|s| s.id)
                .collect();
            for id in ids {
                self.engine.flush(id);
            }
        }
        let now = Instant::now();
        let dt = match self.last_tick {
            Some(t) => {
                let dt = (now - t).as_secs_f32();
                if dt < TICK_MIN {
                    return;
                }
                dt.min(0.1)
            }
            None => 1.0 / 60.0,
        };
        self.last_tick = Some(now);

        let written = feed.written();
        let fresh = written != self.last_written;
        self.last_written = written;
        if fresh {
            self.last_fresh = Some(now);
        }
        let stopped = self
            .last_fresh
            .is_none_or(|t| (now - t).as_secs_f32() > SILENT_AFTER);

        // The feed's shared spectrum: the hub and a spectrum panel pay for one FFT.
        let rate = feed.sample_rate();
        let mags = if fresh {
            feed.magnitudes(HUB_FFT)
        } else {
            None
        };
        self.engine
            .step(mags.as_deref(), rate, stopped, dt, &self.pool);
    }
}

impl SignalHub {
    /// Gated, `None` for an unknown id. Routes, meters and shaders all read this.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.advanced().engine.output(id)
    }

    /// Before the gate, for the meter that draws the threshold across it.
    pub fn raw_value(&self, id: u64) -> Option<f32> {
        self.advanced().engine.value(id)
    }

    /// Drain an aggregate by hand. Drains rather than snaps so a shader doesn't tear.
    pub fn flush(&self, id: u64) {
        self.inner.lock().unwrap().engine.flush(id);
    }

    /// Whether a tail is still falling after the feed went quiet. Keep drawing
    /// on this, not `live`, or fades freeze partway down.
    pub fn settling(&self) -> bool {
        let hub = self.advanced();
        hub.engine.settling(&hub.pool)
    }

    pub fn live(&self) -> bool {
        self.advanced()
            .last_fresh
            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.3)
    }

    pub fn pool(&self) -> Vec<Signal> {
        self.inner.lock().unwrap().pool.clone()
    }

    /// Engine state for ids still in the pool is kept.
    pub fn set_pool(&self, pool: Vec<Signal>) {
        self.inner.lock().unwrap().pool = pool;
    }

    pub fn edit(&self, edit: impl FnOnce(&mut Vec<Signal>)) -> Vec<Signal> {
        let mut hub = self.inner.lock().unwrap();
        edit(&mut hub.pool);
        hub.pool.clone()
    }

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
        // Bin 100 is about 1.17 kHz.
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![band(1, 800.0, 2000.0), band(2, 8000.0, 16000.0)];
        for _ in 0..30 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert!(engine.value(1).unwrap() > 0.9);
        assert!(engine.value(2).unwrap() < 0.05);
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

    /// A falling signal keeps a surface drawing until it lands, or the fade
    /// freezes where the audio stopped.
    #[test]
    fn a_falling_signal_reads_as_settling_until_it_lands() {
        let mut engine = Signals::new();
        let mut mags = vec![0.0f32; 2048];
        mags[100] = 1.0;
        let pool = vec![Signal {
            smooth: 0.85,
            ..band(1, 800.0, 2000.0)
        }];
        for _ in 0..60 {
            engine.step(Some(&mags), 48_000, false, 0.016, &pool);
        }
        assert!(engine.settling(&pool), "a signal that's up is still motion");

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

        // A parked phase isn't motion, or the frames would never stop.
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

        // Pinned above the line: the pulse rings down and nothing refires.
        for _ in 0..60 {
            engine.step(Some(&loud), 48_000, false, 0.016, &pool);
        }
        assert!(
            engine.output(5).unwrap() < 0.1,
            "holding above the line should not hold the pulse"
        );

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
        // A level mid-window: readable, but under a gate set above it.
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

        // Gated above the value: nothing leaves, and the engine keeps its value.
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
        // A pinned band still comes out wide open.
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
        // Rate 2/s at full input: two laps a second, ending mid-ramp.
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
        let after = engine.value(2).unwrap();
        assert!(after > 0.0 && after < mid, "it should resume from zero");
    }

    #[test]
    fn an_aggregate_over_a_missing_or_circular_source_stays_put() {
        let mut engine = Signals::new();
        let mags = vec![0.0f32; 2048];
        // A missing source and a two-aggregate ring: no hang, nothing climbs.
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

    /// The Critters bundle ships a trigger, so this exact JSON must keep parsing.
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

    /// Routes must round-trip byte for byte or saved layouts drift on load.
    #[test]
    fn route_json_round_trips_unchanged() {
        let old = r#"{"enabled":true,"signal":7,"target":"slot3","from":0.25,"to":1.5}"#;
        let route: Route = serde_json::from_str(old).unwrap();
        assert!(route.enabled);
        assert_eq!(route.signal, 7);
        assert_eq!(route.target, "slot3");
        assert_eq!((route.from, route.to), (0.25, 1.5));
        assert_eq!(serde_json::to_string(&route).unwrap(), old);

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
        let named = hub.edit(|pool| pool[0].name = "Mix swell".to_string());
        assert_eq!(named[0].label(), "Mix swell");
    }

    fn tone(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect()
    }

    /// Waits out the frame window between reads so each one advances.
    fn play(feed: &AudioFeed, hub: &SignalHub, id: u64, frames: usize) -> Option<f32> {
        let mut value = None;
        for _ in 0..frames {
            feed.push(&tone(1024));
            std::thread::sleep(std::time::Duration::from_millis(5));
            value = hub.value(id);
        }
        value
    }

    #[test]
    fn a_bound_hub_moves_when_read_and_a_bare_one_never_does() {
        let level = Signal {
            id: 1,
            source: Source::Level,
            smooth: 0.0,
            ..Signal::default()
        };

        let feed = Arc::new(AudioFeed::new());
        let hub = SignalHub::with_feed(vec![level.clone()], feed.clone());
        let heard = play(&feed, &hub, 1, 10).expect("the signal has a slot once read");
        assert!(
            heard > 0.5,
            "a full-scale tone should read loud, got {heard}"
        );

        // No feed, no step, no slot.
        let bare = SignalHub::unfed(vec![level]);
        assert_eq!(play(&feed, &bare, 1, 3), None);
    }

    #[test]
    fn a_track_change_on_the_feed_resets_the_aggregates_that_ask() {
        let level = Signal {
            id: 1,
            source: Source::Level,
            smooth: 0.0,
            ..Signal::default()
        };
        let total = Signal {
            id: 2,
            source: Source::Aggregate { of: 1, rate: 1.0 },
            smooth: 0.0,
            reset_on_track: true,
            ..Signal::default()
        };
        let feed = Arc::new(AudioFeed::new());
        let hub = SignalHub::with_feed(vec![level, total], feed.clone());

        let draining = |hub: &SignalHub| hub.inner.lock().unwrap().engine.slots[&2].draining;

        feed.set_track(Some(0));
        let before = play(&feed, &hub, 2, 20).expect("the total has a slot");
        assert!(
            before > FLUSH_DONE,
            "a loud input should have run the total up"
        );
        assert!(!draining(&hub), "the first track is where it started");

        // A new entry on the feed starts the drain on the next read.
        feed.set_track(Some(1));
        hub.raw_value(1);
        assert!(draining(&hub), "the song change should start the drain");
    }
}
