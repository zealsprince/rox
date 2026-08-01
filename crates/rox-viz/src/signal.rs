//! The app's shared modulation layer: named signals over the playback
//! spectrum that any parameter anywhere can ride. A [`Signal`] is one
//! source (a frequency band's energy, the whole mix's level, or a
//! transient detector) with its response smoothing; a [`Route`] attaches
//! one signal to one host-defined parameter with an output span. The pool
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
}

impl Source {
    /// The watched bin span for this source, clamped so a hand-edited file
    /// can't invert the band or walk off the spectrum.
    fn bins(&self, sample_rate: u32, half: usize) -> (usize, usize) {
        let (lo, hi) = match *self {
            Source::Band { lo, hi } | Source::Onset { lo, hi } => {
                let lo = lo.clamp(BAND_MIN_HZ, BAND_MAX_HZ);
                (lo, hi.clamp(lo * 1.01, BAND_MAX_HZ))
            }
            Source::Level => (BAND_MIN_HZ, BAND_MAX_HZ),
        };
        log_bands(1, lo, hi, sample_rate, half)[0]
    }
}

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
    /// it. On an onset source this is the pulse's decay instead.
    pub smooth: f32,
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
        }
    }
}

impl Signal {
    fn smooth(&self) -> f32 {
        self.smooth.clamp(0.0, 1.0)
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
            Source::Level => "Level".to_string(),
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
/// spans, how fast the reference envelope chases the band, how far past
/// the reference the band must jump to read as a hit, and the level below
/// which nothing counts, so the noise floor can't fire it.
const ONSET_DECAY_FAST: f32 = 16.0;
const ONSET_DECAY_SLOW: f32 = 1.5;
const ONSET_REF_ATTACK: f32 = 2.5;
const ONSET_REF_RELEASE: f32 = 2.0;
const ONSET_MARGIN: f32 = 0.12;
const ONSET_FLOOR: f32 = 0.15;

/// One signal's running state in the engine.
#[derive(Clone, Copy)]
struct Slot {
    value: f32,
    /// The onset detector's slow reference envelope; idle otherwise.
    reference: f32,
    /// Whether an onset slot is ready to fire again.
    armed: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            value: 0.0,
            reference: 0.0,
            armed: true,
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

    /// The signal's current value, `None` for an id the pool doesn't
    /// carry, which is what lets routes to deleted signals skip quietly.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.slots.get(&id).map(|slot| slot.value)
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
            }),
        }
    }

    /// Advance the engine one frame off the feed. Cheap to call from every
    /// consumer: calls landing within the same frame window return
    /// immediately, so the clock only moves once however many panels ask.
    pub fn tick(&self, feed: &AudioFeed) {
        let mut hub = self.inner.lock().unwrap();
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

    /// The signal's current value, `None` for an id the pool doesn't
    /// carry.
    pub fn value(&self, id: u64) -> Option<f32> {
        self.inner.lock().unwrap().engine.value(id)
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
    fn inverted_band_from_a_hand_edited_file_does_not_panic() {
        let mut engine = Signals::new();
        let mags = vec![0.0f32; 1024];
        let pool = vec![band(1, 5000.0, 40.0), band(2, -10.0, 1e9)];
        engine.step(Some(&mags), 48_000, false, 0.016, &pool);
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
