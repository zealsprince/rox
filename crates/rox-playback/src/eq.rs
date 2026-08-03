//! The parametric equalizer, the chain's first real node (ADR 19). Ten
//! peaking biquads per channel, each with its own center, gain, and width,
//! no lookahead and nothing allocated once it's built. The centers start on
//! the ISO octaves a graphic EQ uses, so it opens as the familiar thing and
//! becomes parametric the moment a band is dragged off its home.
//!
//! [`EqParams::response_db`] evaluates the same coefficients the node runs,
//! which is what lets a plot of the curve be the truth rather than an
//! artist's impression of it.
//!
//! Its parameters live in [`EqParams`], an Arc the UI and the decode thread
//! both hold: dragging a band is an atomic store, and the node picks the
//! change up on its next buffer. That's the ADR's split between parameters
//! and structure: only putting the node in a chain rides the engine's
//! command channel, everything after is a store.
//!
//! A band sitting at 0 dB is a bit-exact passthrough rather than an
//! approximate one. The cookbook's peaking coefficients collapse to b0 = 1
//! with b1 == a1 and b2 == a2, so the arithmetic cancels back to the input
//! sample and the filter state stays at zero. That's what lets the EQ sit
//! in the chain while it's flat without anyone having to trust it.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use crate::chain::Node;

/// Band centers in Hz: ISO octave spacing, the ten sliders a graphic EQ has
/// worn since the hi-fi rack. Their order is the order gains are stored and
/// drawn in.
pub const BAND_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// How many bands there are, for anything sizing itself to the set.
pub const BANDS: usize = BAND_HZ.len();

/// How far a band cuts or boosts, in dB either way. Past this a band stops
/// shaping and starts distorting the mix into the ceiling.
pub const GAIN_MAX_DB: f32 = 12.0;

/// The default width, one octave: neighbouring bands overlap at their
/// half-power points and a run of them adds up to a smooth curve rather
/// than a comb. Q = sqrt(2^BW) / (2^BW - 1) at BW = 1.
pub const Q_DEFAULT: f32 = std::f32::consts::SQRT_2;

/// How narrow and how wide a band can be pulled. Below the floor a band
/// stops being a bell and starts being a whistle; above the ceiling it
/// covers most of the spectrum and the neighbours stop meaning anything.
pub const Q_MIN: f32 = 0.2;
pub const Q_MAX: f32 = 12.0;

/// Where a band can sit. The bottom is under anything a speaker reproduces
/// and the top is past where most people hear, so the whole audible range
/// is reachable without the ends being useful places to park.
pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20000.0;

/// Where a band gives up. The cookbook's coefficients degenerate as the
/// center approaches Nyquist (alpha goes to zero, both poles land on the
/// unit circle), so a 16 kHz band at a 32 kHz device rate has to pass
/// through instead of ringing forever.
const NYQUIST_MARGIN: f64 = 0.45;

/// Filter state below this is a decay tail nobody can hear, and left alone
/// it walks down into subnormals where some CPUs fall off a cliff. Flushed
/// once per buffer rather than per sample.
const QUIET: f64 = 1e-30;

/// The equalizer's live parameters, shared between whatever UI draws the
/// bands and the node running on the decode thread. Every field is an
/// atomic because that's the contract: a knob write is a store, and the
/// change lands as soon as the ring drains past it.
pub struct EqParams {
    enabled: AtomicBool,
    /// Per-band gain in dB, as f32 bits. Same trick the volume atomic uses.
    gains_db: [AtomicU32; BANDS],
    /// Per-band center in Hz. Movable, which is what makes this parametric
    /// rather than a graphic EQ with the centers welded to the ISO octaves.
    freqs_hz: [AtomicU32; BANDS],
    /// Per-band Q. Higher is narrower; see [`Q_DEFAULT`].
    qs: [AtomicU32; BANDS],
}

impl EqParams {
    /// Build the shared parameters from persisted state. Each list folds to
    /// what's there, so a settings file written against a different set of
    /// bands, or one from before the centers were movable, loads instead of
    /// resetting the user's curve. A missing center falls back to that
    /// band's ISO octave, which is exactly where the graphic EQ had it.
    pub fn new(enabled: bool, gains_db: &[f32], freqs_hz: &[f32], qs: &[f32]) -> EqParams {
        EqParams {
            enabled: AtomicBool::new(enabled),
            gains_db: std::array::from_fn(|band| {
                let db = gains_db.get(band).copied().unwrap_or(0.0);
                AtomicU32::new(clamp_db(db).to_bits())
            }),
            freqs_hz: std::array::from_fn(|band| {
                let hz = freqs_hz.get(band).copied().unwrap_or(BAND_HZ[band]);
                AtomicU32::new(clamp_hz(hz, band).to_bits())
            }),
            qs: std::array::from_fn(|band| {
                let q = qs.get(band).copied().unwrap_or(Q_DEFAULT);
                AtomicU32::new(clamp_q(q).to_bits())
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// A band's gain in dB. Out-of-range bands read flat, so a caller
    /// iterating a stale band count can't panic the audio thread.
    pub fn gain(&self, band: usize) -> f32 {
        self.gains_db
            .get(band)
            .map(|g| f32::from_bits(g.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    /// Set a band's gain in dB, clamped to the range the sliders offer.
    pub fn set_gain(&self, band: usize, db: f32) {
        if let Some(slot) = self.gains_db.get(band) {
            slot.store(clamp_db(db).to_bits(), Ordering::Relaxed);
        }
    }

    /// A band's center in Hz.
    pub fn freq(&self, band: usize) -> f32 {
        self.freqs_hz
            .get(band)
            .map(|f| f32::from_bits(f.load(Ordering::Relaxed)))
            .unwrap_or_else(|| BAND_HZ.get(band).copied().unwrap_or(1000.0))
    }

    /// Move a band's center, clamped to the audible range.
    pub fn set_freq(&self, band: usize, hz: f32) {
        if let Some(slot) = self.freqs_hz.get(band) {
            slot.store(clamp_hz(hz, band).to_bits(), Ordering::Relaxed);
        }
    }

    /// A band's Q. Higher is narrower.
    pub fn q(&self, band: usize) -> f32 {
        self.qs
            .get(band)
            .map(|q| f32::from_bits(q.load(Ordering::Relaxed)))
            .unwrap_or(Q_DEFAULT)
    }

    /// Set a band's Q, clamped to the range the curve can draw usefully.
    pub fn set_q(&self, band: usize, q: f32) {
        if let Some(slot) = self.qs.get(band) {
            slot.store(clamp_q(q).to_bits(), Ordering::Relaxed);
        }
    }

    /// Every band back to 0 dB. Centers and widths stay put: flatten is
    /// about undoing the shaping, not throwing away where the bands were
    /// placed to do it.
    pub fn flatten(&self) {
        for band in 0..BANDS {
            self.set_gain(band, 0.0);
        }
    }

    /// Centers and widths back to the ISO octaves at one octave wide, the
    /// layout a graphic EQ has. The gains ride along untouched.
    pub fn reset_shape(&self) {
        for (band, hz) in BAND_HZ.iter().enumerate() {
            self.set_freq(band, *hz);
            self.set_q(band, Q_DEFAULT);
        }
    }

    /// The whole curve, in band order. What gets persisted.
    pub fn gains(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.gain(band)).collect()
    }

    /// Every center, in band order.
    pub fn freqs(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.freq(band)).collect()
    }

    /// Every width, in band order.
    pub fn qs(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.q(band)).collect()
    }

    /// The whole cascade's gain at one frequency, in dB: what a plot of the
    /// EQ draws, and the only honest way to show what a stack of overlapping
    /// bells actually does to the signal. Computed from the same
    /// coefficients the node runs, so the picture can't drift from the
    /// sound. Off, everything is flat.
    ///
    /// This is for whatever draws the curve, not the audio thread, which
    /// never needs to know its own response.
    pub fn response_db(&self, hz: f32, rate: u32) -> f32 {
        if !self.enabled() {
            return 0.0;
        }
        (0..BANDS)
            .map(|band| {
                let coeffs = coeffs(self.freq(band) as f64, rate, self.gain(band), self.q(band));
                coeffs.gain_db(hz as f64, rate)
            })
            .sum()
    }
}

/// A gain the filter math can live with: NaN out of a corrupt settings file
/// would poison the state and never come back.
fn clamp_db(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(-GAIN_MAX_DB, GAIN_MAX_DB)
    }
}

/// A center the filter math can live with. NaN falls back to the band's own
/// ISO octave rather than to a fixed value, so a corrupt file lands on the
/// layout the band would have had.
fn clamp_hz(hz: f32, band: usize) -> f32 {
    if hz.is_nan() {
        BAND_HZ.get(band).copied().unwrap_or(1000.0)
    } else {
        hz.clamp(FREQ_MIN, FREQ_MAX)
    }
}

/// A width the filter math can live with. Zero or negative Q divides by zero
/// in the cookbook's alpha, so this floor is load-bearing rather than taste.
fn clamp_q(q: f32) -> f32 {
    if q.is_nan() {
        Q_DEFAULT
    } else {
        q.clamp(Q_MIN, Q_MAX)
    }
}

/// The EQ as a chain node: the shared parameters plus one biquad per band
/// per channel. Everything it needs exists after [`Node::reset`], so
/// `process` only ever does arithmetic.
pub struct Eq {
    params: Arc<EqParams>,
    bands: [Band; BANDS],
    /// The rate the coefficients were built against, 0 before the first
    /// reset.
    rate: u32,
}

impl Eq {
    pub fn new(params: Arc<EqParams>) -> Eq {
        Eq {
            params,
            bands: [Band::PASSTHROUGH; BANDS],
            rate: 0,
        }
    }
}

impl Node for Eq {
    fn reset(&mut self, rate: u32) {
        self.rate = rate;
        for (i, band) in self.bands.iter_mut().enumerate() {
            band.clear();
            band.tune(
                self.params.freq(i),
                rate,
                self.params.gain(i),
                self.params.q(i),
            );
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        // Off, or reset hasn't happened yet: hand the buffer back untouched.
        // That's the bypass rule the ADR makes checkable, held here for a
        // node that's in the chain but idle. The history goes with it: the
        // samples that pass while the EQ is off never reached the filters,
        // so keeping the old state would have switching back on resume from
        // audio that's minutes gone.
        if self.rate == 0 || !self.params.enabled() {
            for band in &mut self.bands {
                band.clear();
            }
            return;
        }
        for (i, band) in self.bands.iter_mut().enumerate() {
            let shape = (self.params.freq(i), self.params.gain(i), self.params.q(i));
            if shape != band.shape {
                band.tune(shape.0, self.rate, shape.1, shape.2);
            }
            band.run(buf);
        }
    }
}

/// A biquad's five coefficients, a0 already divided out.
#[derive(Clone, Copy)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// The identity: input straight back out.
    const PASSTHROUGH: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// This section's gain at one frequency, in dB. The transfer function
    /// evaluated on the unit circle: |H(e^-jw)| as the ratio of the two
    /// quadratics' magnitudes.
    fn gain_db(&self, hz: f64, rate: u32) -> f32 {
        if rate == 0 {
            return 0.0;
        }
        let w = std::f64::consts::TAU * hz / rate as f64;
        let (sin1, cos1) = w.sin_cos();
        let (sin2, cos2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * cos1 + self.b2 * cos2;
        let num_im = -(self.b1 * sin1 + self.b2 * sin2);
        let den_re = 1.0 + self.a1 * cos1 + self.a2 * cos2;
        let den_im = -(self.a1 * sin1 + self.a2 * sin2);
        let num = num_re * num_re + num_im * num_im;
        let den = den_re * den_re + den_im * den_im;
        if den <= 0.0 || num <= 0.0 {
            return 0.0;
        }
        // 10 rather than 20 because these are already squared magnitudes.
        (10.0 * (num / den).log10()) as f32
    }
}

/// The audio EQ cookbook's peaking filter, the one place the coefficients
/// are worked out. The node runs them and the plot evaluates them, so a
/// curve on screen can't claim something the filter isn't doing.
///
/// `b1` and `a1` come out of the same expression on purpose: at 0 dB that
/// makes the difference in the state update exactly zero, so a flat band is
/// bit-exact rather than nearly so.
fn coeffs(hz: f64, rate: u32, db: f32, q: f32) -> Coeffs {
    if rate == 0 || hz >= rate as f64 * NYQUIST_MARGIN {
        return Coeffs::PASSTHROUGH;
    }
    let amp = 10f64.powf(db as f64 / 40.0);
    let w0 = std::f64::consts::TAU * hz / rate as f64;
    let alpha = w0.sin() / (2.0 * clamp_q(q) as f64);
    let a0 = 1.0 + alpha / amp;
    Coeffs {
        b0: (1.0 + alpha * amp) / a0,
        b1: (-2.0 * w0.cos()) / a0,
        b2: (1.0 - alpha * amp) / a0,
        a1: (-2.0 * w0.cos()) / a0,
        a2: (1.0 - alpha / amp) / a0,
    }
}

/// One band: a peaking biquad in transposed direct form II, run
/// independently over each of the two channels. TDF-II because the state
/// stays bounded by the signal rather than the intermediate, which is what
/// makes a coefficient swap mid-stream (a slider drag) settle instead of
/// jump. The state is f64: a 32 Hz biquad at 48 kHz sits close enough to
/// the unit circle that f32 accumulates audible noise in it.
#[derive(Clone, Copy)]
struct Band {
    coeffs: Coeffs,
    /// The shape these coefficients were built for, so a buffer where
    /// nothing moved skips the trig.
    shape: (f32, f32, f32),
    /// Per channel, left then right.
    s1: [f64; 2],
    s2: [f64; 2],
}

impl Band {
    /// A band that hands its input straight back, the shape one takes
    /// before the first reset and at every rate where its center is too
    /// close to Nyquist to filter.
    const PASSTHROUGH: Band = Band {
        coeffs: Coeffs::PASSTHROUGH,
        shape: (0.0, 0.0, 0.0),
        s1: [0.0; 2],
        s2: [0.0; 2],
    };

    fn clear(&mut self) {
        self.s1 = [0.0; 2];
        self.s2 = [0.0; 2];
    }

    /// Rebuild the coefficients for a center, a rate, a gain, and a width.
    /// The state stays where it is: this runs mid-stream on a drag, and
    /// zeroing here would click.
    fn tune(&mut self, hz: f32, rate: u32, db: f32, q: f32) {
        self.shape = (hz, db, q);
        self.coeffs = coeffs(hz as f64, rate, db, q);
    }

    /// Run one interleaved stereo buffer through the band in place. A
    /// trailing odd sample can't be half a frame, so it's left alone.
    fn run(&mut self, buf: &mut [f32]) {
        let Coeffs { b0, b1, b2, a1, a2 } = self.coeffs;
        for frame in buf.chunks_exact_mut(2) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let x = *sample as f64;
                let y = b0 * x + self.s1[ch];
                self.s1[ch] = b1 * x - a1 * y + self.s2[ch];
                self.s2[ch] = b2 * x - a2 * y;
                *sample = y as f32;
            }
        }
        for ch in 0..2 {
            if self.s1[ch].abs() < QUIET && self.s2[ch].abs() < QUIET {
                self.s1[ch] = 0.0;
                self.s2[ch] = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Chain;

    const RATE: u32 = 48000;

    /// A stereo ramp with both channels carrying something, so a channel
    /// leak or a swapped state shows up.
    fn signal(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let t = n as f32 / RATE as f32;
                let left = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                let right = (t * 110.0 * std::f32::consts::TAU).sin() * 0.25;
                [left, right]
            })
            .collect()
    }

    fn enabled_eq(gains: &[f32]) -> Eq {
        Eq::new(Arc::new(EqParams::new(true, gains, &[], &[])))
    }

    /// The bypass rule with the node actually in the chain: every band flat
    /// means the samples that reach the ring are the ones the decoder
    /// produced, bit for bit, not merely close.
    #[test]
    fn a_flat_eq_is_bit_exact_passthrough() {
        let mut chain = Chain::new();
        chain.push(Box::new(enabled_eq(&[])));
        chain.reset(RATE);
        let original = signal(2048);
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_eq!(buf, original, "a flat EQ changes nothing at all");
    }

    /// Same claim for a disabled EQ carrying a curve: the node is in the
    /// chain, its bands are anything but flat, and the buffer still comes
    /// out untouched.
    #[test]
    fn a_disabled_eq_is_bit_exact_passthrough() {
        let params = Arc::new(EqParams::new(false, &[12.0; BANDS], &[], &[]));
        let mut chain = Chain::new();
        chain.push(Box::new(Eq::new(params.clone())));
        chain.reset(RATE);
        let original = signal(1024);
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_eq!(buf, original);
        // And it's the flag doing it, not a dead node.
        params.set_enabled(true);
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_ne!(buf, original);
    }

    /// A boost lands and a cut lands, measured where the band actually
    /// sits: drive the band's own center and compare the settled amplitude
    /// against the same signal through a flat EQ.
    #[test]
    fn a_gain_change_lands() {
        // 1 kHz is band 5, far enough from its neighbours that their
        // skirts don't muddy the number.
        let band = 5;
        let hz = BAND_HZ[band];
        let frames = RATE as usize / 4;
        let tone: Vec<f32> = (0..frames)
            .flat_map(|n| {
                let s = (n as f32 / RATE as f32 * hz * std::f32::consts::TAU).sin() * 0.25;
                [s, s]
            })
            .collect();

        let peak = |db: f32| {
            let mut gains = [0.0f32; BANDS];
            gains[band] = db;
            let mut eq = enabled_eq(&gains);
            eq.reset(RATE);
            let mut buf = tone.clone();
            eq.process(&mut buf);
            // Skip the first half: the filter is still settling into the
            // tone, and the tail is what the ear hears as level.
            buf[buf.len() / 2..]
                .iter()
                .fold(0.0f32, |peak, s| peak.max(s.abs()))
        };

        let flat = peak(0.0);
        let boosted = peak(12.0);
        let cut = peak(-12.0);
        // 12 dB is a factor of 4; allow a wide window, the point is that
        // the gain is in the right place and roughly the right size.
        assert!(
            boosted > flat * 3.0 && boosted < flat * 4.5,
            "boost landed at {boosted} against flat {flat}"
        );
        assert!(
            cut < flat * 0.35 && cut > flat * 0.2,
            "cut landed at {cut} against flat {flat}"
        );
    }

    /// Filter history survives a buffer boundary: one pass over a whole
    /// signal and two passes over its halves have to produce the same
    /// samples, or the chunk size the decoder happens to hand over would be
    /// audible as a click on every boundary.
    #[test]
    fn history_carries_across_a_buffer_boundary() {
        let gains = [6.0, -6.0, 3.0, 0.0, -3.0, 9.0, 0.0, -9.0, 4.0, -4.0];
        let original = signal(1024);

        let mut whole = original.clone();
        let mut eq = enabled_eq(&gains);
        eq.reset(RATE);
        eq.process(&mut whole);

        let mut split = original.clone();
        let mut eq = enabled_eq(&gains);
        eq.reset(RATE);
        let (head, tail) = split.split_at_mut(original.len() / 2);
        eq.process(head);
        eq.process(tail);

        assert_eq!(whole, split, "the split at the buffer boundary is audible");
    }

    /// The same for the gapless splice: the engine resets the chain at
    /// stream open and on a flush, never between tracks of an album, so a
    /// track boundary has to look exactly like a buffer boundary to the
    /// filters. A reset in the middle breaks it, which is what makes the
    /// promise worth writing down.
    #[test]
    fn a_reset_is_what_drops_history_not_a_track_change() {
        let gains = [9.0; BANDS];
        let original = signal(512);

        let mut carried = original.clone();
        let mut eq = enabled_eq(&gains);
        eq.reset(RATE);
        let (head, tail) = carried.split_at_mut(original.len() / 2);
        eq.process(head);
        // The gapless boundary: the next track's samples arrive, no reset.
        eq.process(tail);

        let mut broken = original.clone();
        let mut eq = enabled_eq(&gains);
        eq.reset(RATE);
        let (head, tail) = broken.split_at_mut(original.len() / 2);
        eq.process(head);
        eq.reset(RATE);
        eq.process(tail);

        assert_ne!(carried, broken);
    }

    /// A band whose center sits at or past Nyquist passes through instead
    /// of ringing: the 16 kHz band at a 32 kHz device rate is the real
    /// case.
    #[test]
    fn bands_past_nyquist_pass_through() {
        let mut gains = [0.0f32; BANDS];
        gains[BANDS - 1] = 12.0;
        let mut eq = enabled_eq(&gains);
        eq.reset(32000);
        let original = signal(256);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_eq!(buf, original);
        // The same band at a rate that can carry it does shape the signal.
        let mut eq = enabled_eq(&gains);
        eq.reset(48000);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_ne!(buf, original);
    }

    /// Gains out of a settings file land clamped and NaN-free, so nothing
    /// a hand-edited file can say poisons the filter state.
    #[test]
    fn stored_gains_come_back_sane() {
        let params = EqParams::new(true, &[99.0, -99.0, f32::NAN, 3.5], &[], &[]);
        assert_eq!(params.gain(0), GAIN_MAX_DB);
        assert_eq!(params.gain(1), -GAIN_MAX_DB);
        assert_eq!(params.gain(2), 0.0);
        assert_eq!(params.gain(3), 3.5);
        // A short list pads out flat, and the rest of the set is there.
        assert_eq!(params.gains().len(), BANDS);
        assert_eq!(params.gain(BANDS - 1), 0.0);
        params.flatten();
        assert!(params.gains().iter().all(|db| *db == 0.0));
    }

    /// A settings file from before the centers moved still loads: the
    /// missing lists fall back to the ISO octaves at one octave wide, which
    /// is the graphic EQ the gains were dialed on.
    #[test]
    fn a_pre_parametric_file_loads_onto_the_iso_octaves() {
        let params = EqParams::new(true, &[3.0; BANDS], &[], &[]);
        assert_eq!(params.freqs(), BAND_HZ.to_vec());
        assert!(params.qs().iter().all(|q| *q == Q_DEFAULT));
    }

    /// Centers and widths clamp the same way gains do, and a zero Q (which
    /// would divide by zero in the cookbook's alpha) lands on the floor.
    #[test]
    fn stored_shape_comes_back_sane() {
        let params = EqParams::new(true, &[], &[1.0, 99_000.0, f32::NAN], &[0.0, -4.0, 99.0]);
        assert_eq!(params.freq(0), FREQ_MIN);
        assert_eq!(params.freq(1), FREQ_MAX);
        assert_eq!(params.freq(2), BAND_HZ[2]);
        assert_eq!(params.q(0), Q_MIN);
        assert_eq!(params.q(1), Q_MIN);
        assert_eq!(params.q(2), Q_MAX);
    }

    /// The plotted curve is the filter's own answer: a boosted band reads
    /// near its gain at its center and falls away either side of it.
    #[test]
    fn the_response_matches_the_band_at_its_center() {
        let params = EqParams::new(true, &[], &[], &[]);
        params.set_freq(0, 1000.0);
        params.set_gain(0, 6.0);
        let at_center = params.response_db(1000.0, RATE);
        assert!(
            (at_center - 6.0).abs() < 0.5,
            "a 6 dB band should read about 6 dB at its center, read {at_center}"
        );
        // Far enough away that neither this band nor its neighbours reach.
        let far = params.response_db(60.0, RATE);
        assert!(far < at_center, "the bell has to fall off, read {far}");
    }

    /// A narrower band reaches less far, which is the whole point of Q and
    /// the thing a curve has to show honestly.
    #[test]
    fn a_higher_q_narrows_the_bell() {
        let wide = EqParams::new(true, &[], &[], &[]);
        wide.set_freq(0, 1000.0);
        wide.set_gain(0, 12.0);
        wide.set_q(0, 0.5);
        let narrow = EqParams::new(true, &[], &[], &[]);
        narrow.set_freq(0, 1000.0);
        narrow.set_gain(0, 12.0);
        narrow.set_q(0, 8.0);
        // Off to the side, the wide one is still lifting and the narrow one
        // has let go.
        let (wide_off, narrow_off) = (
            wide.response_db(1400.0, RATE),
            narrow.response_db(1400.0, RATE),
        );
        assert!(
            wide_off > narrow_off,
            "wide {wide_off} should still be lifting where narrow {narrow_off} has let go"
        );
    }

    /// Off means flat, so the plot can't draw a curve the signal isn't
    /// getting.
    #[test]
    fn a_disabled_eq_plots_flat() {
        let params = EqParams::new(false, &[12.0; BANDS], &[], &[]);
        for hz in [50.0, 500.0, 5000.0] {
            assert_eq!(params.response_db(hz, RATE), 0.0);
        }
    }

    /// The node hears a center move, not just a gain move: the same store
    /// the UI makes on a drag has to retune on the next buffer.
    #[test]
    fn moving_a_center_retunes_the_node() {
        let params = Arc::new(EqParams::new(true, &[], &[], &[]));
        params.set_gain(0, 12.0);
        let mut eq = Eq::new(params.clone());
        eq.reset(RATE);
        let mut first = signal(256);
        eq.process(&mut first);
        params.set_freq(0, 900.0);
        let mut second = signal(256);
        eq.process(&mut second);
        assert!(
            first != second,
            "a center that moved has to change what comes out"
        );
    }
}
