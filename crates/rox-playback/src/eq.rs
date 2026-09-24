//! The parametric equalizer, a chain node (ADR 19): ten peaking biquads per
//! channel, starting on the ISO octaves. [`EqParams`] is shared with the UI,
//! so a drag is an atomic store picked up on the next buffer.
//! [`EqParams::response_db`] evaluates the node's own coefficients, so the
//! plotted curve is the real one.
//!
//! A band at 0 dB is bit-exact: the cookbook coefficients collapse to b0 = 1,
//! b1 == a1, b2 == a2, so a flat EQ can sit in the chain.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::chain::Node;

/// ISO octave centers. Their order is the order gains are stored and drawn in.
pub const BAND_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

pub const BANDS: usize = BAND_HZ.len();

/// Past this a band distorts rather than shapes.
pub const GAIN_MAX_DB: f32 = 12.0;

/// One octave: Q = sqrt(2^BW) / (2^BW - 1) at BW = 1.
pub const Q_DEFAULT: f32 = std::f32::consts::SQRT_2;

pub const Q_MIN: f32 = 0.2;
pub const Q_MAX: f32 = 12.0;

pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20000.0;

/// Cookbook coefficients degenerate near Nyquist, so a band past this share
/// of the rate passes through (a 16 kHz band at 32 kHz).
const NYQUIST_MARGIN: f64 = 0.45;

/// Flushed to zero once per buffer so the decay tail never reaches subnormals.
const QUIET: f64 = 1e-30;

/// Live parameters shared between the UI and the node on the decode thread.
/// All atomics, so a knob write is a store.
pub struct EqParams {
    enabled: AtomicBool,
    /// f32 bits, like the volume atomic.
    gains_db: [AtomicU32; BANDS],
    freqs_hz: [AtomicU32; BANDS],
    qs: [AtomicU32; BANDS],
}

impl EqParams {
    /// Each list folds to what's there, so a file with fewer bands or no centers
    /// loads instead of resetting the curve. Missing centers take their ISO octave.
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

    /// Out-of-range bands read flat rather than panicking the audio thread.
    pub fn gain(&self, band: usize) -> f32 {
        self.gains_db
            .get(band)
            .map(|g| f32::from_bits(g.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    pub fn set_gain(&self, band: usize, db: f32) {
        if let Some(slot) = self.gains_db.get(band) {
            slot.store(clamp_db(db).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn freq(&self, band: usize) -> f32 {
        self.freqs_hz
            .get(band)
            .map(|f| f32::from_bits(f.load(Ordering::Relaxed)))
            .unwrap_or_else(|| BAND_HZ.get(band).copied().unwrap_or(1000.0))
    }

    pub fn set_freq(&self, band: usize, hz: f32) {
        if let Some(slot) = self.freqs_hz.get(band) {
            slot.store(clamp_hz(hz, band).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn q(&self, band: usize) -> f32 {
        self.qs
            .get(band)
            .map(|q| f32::from_bits(q.load(Ordering::Relaxed)))
            .unwrap_or(Q_DEFAULT)
    }

    pub fn set_q(&self, band: usize, q: f32) {
        if let Some(slot) = self.qs.get(band) {
            slot.store(clamp_q(q).to_bits(), Ordering::Relaxed);
        }
    }

    /// Centers and widths stay put.
    pub fn flatten(&self) {
        for band in 0..BANDS {
            self.set_gain(band, 0.0);
        }
    }

    /// Gains untouched.
    pub fn reset_shape(&self) {
        for (band, hz) in BAND_HZ.iter().enumerate() {
            self.set_freq(band, *hz);
            self.set_q(band, Q_DEFAULT);
        }
    }

    pub fn apply_graphic_curve(&self, gains: &[f32; BANDS]) {
        self.reset_shape();
        for (band, &db) in gains.iter().enumerate() {
            self.set_gain(band, db);
        }
    }

    /// What gets persisted.
    pub fn gains(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.gain(band)).collect()
    }

    pub fn freqs(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.freq(band)).collect()
    }

    pub fn qs(&self) -> Vec<f32> {
        (0..BANDS).map(|band| self.q(band)).collect()
    }

    /// The whole cascade's gain at `hz` in dB, from the node's own coefficients.
    /// Flat when off. For drawing only.
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

/// NaN from a corrupt file would poison the filter state for good.
fn clamp_db(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(-GAIN_MAX_DB, GAIN_MAX_DB)
    }
}

/// NaN takes the band's own ISO octave.
fn clamp_hz(hz: f32, band: usize) -> f32 {
    if hz.is_nan() {
        BAND_HZ.get(band).copied().unwrap_or(1000.0)
    } else {
        hz.clamp(FREQ_MIN, FREQ_MAX)
    }
}

/// Q at or under zero divides by zero in the cookbook's alpha.
fn clamp_q(q: f32) -> f32 {
    if q.is_nan() {
        Q_DEFAULT
    } else {
        q.clamp(Q_MIN, Q_MAX)
    }
}

/// Everything it needs exists after [`Node::reset`], so `process` is pure arithmetic.
pub struct Eq {
    params: Arc<EqParams>,
    bands: [Band; BANDS],
    /// 0 before the first reset.
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
        // Off or not yet reset: untouched, the bypass rule. Clear history too, or
        // switching back on would resume from audio that's long gone.
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

/// a0 already divided out.
#[derive(Clone, Copy)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    const PASSTHROUGH: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// |H(e^-jw)| in dB.
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
        // Already squared magnitudes.
        (10.0 * (num / den).log10()) as f32
    }
}

/// The one place coefficients are worked out; the node runs them and the plot
/// evaluates them. `b1` and `a1` share an expression on purpose, so a flat
/// band is bit-exact.
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

/// Transposed direct form II, so a mid-stream coefficient swap settles
/// instead of jumping. f64 state: a 32 Hz band at 48 kHz accumulates audible
/// noise in f32.
#[derive(Clone, Copy)]
struct Band {
    coeffs: Coeffs,
    /// The shape these coefficients were built for, so an unchanged buffer skips the trig.
    shape: (f32, f32, f32),
    /// Left, right.
    s1: [f64; 2],
    s2: [f64; 2],
}

impl Band {
    /// Before the first reset, and wherever the center is too close to Nyquist.
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

    /// Keeps the state: zeroing it mid-drag would click.
    fn tune(&mut self, hz: f32, rate: u32, db: f32, q: f32) {
        self.shape = (hz, db, q);
        self.coeffs = coeffs(hz as f64, rate, db, q);
    }

    /// A trailing odd sample is left alone.
    fn run(&mut self, buf: &mut [f32]) {
        let Coeffs { b0, b1, b2, a1, a2 } = self.coeffs;
        for frame in buf.as_chunks_mut::<2>().0 {
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

    /// Different on each channel, so a leak or swapped state shows.
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

    /// The bypass rule with the node in the chain: flat is bit for bit.
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

    /// Disabled with a curve set is bit-exact too.
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
        params.set_enabled(true);
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_ne!(buf, original);
    }

    #[test]
    fn a_gain_change_lands() {
        // Band 5 (1 kHz) sits clear of its neighbours' skirts.
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
            // Skip the first half while the filter settles.
            buf[buf.len() / 2..]
                .iter()
                .fold(0.0f32, |peak, s| peak.max(s.abs()))
        };

        let flat = peak(0.0);
        let boosted = peak(12.0);
        let cut = peak(-12.0);
        // 12 dB is a factor of 4; the window is wide on purpose.
        assert!(
            boosted > flat * 3.0 && boosted < flat * 4.5,
            "boost landed at {boosted} against flat {flat}"
        );
        assert!(
            cut < flat * 0.35 && cut > flat * 0.2,
            "cut landed at {cut} against flat {flat}"
        );
    }

    /// One pass and two half passes must match, or every buffer boundary clicks.
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

    /// The engine never resets the chain between album tracks, so a track
    /// boundary must look like a buffer boundary to the filters.
    #[test]
    fn a_reset_is_what_drops_history_not_a_track_change() {
        let gains = [9.0; BANDS];
        let original = signal(512);

        let mut carried = original.clone();
        let mut eq = enabled_eq(&gains);
        eq.reset(RATE);
        let (head, tail) = carried.split_at_mut(original.len() / 2);
        eq.process(head);
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

    /// The 16 kHz band at a 32 kHz rate passes through.
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
        let mut eq = enabled_eq(&gains);
        eq.reset(48000);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_ne!(buf, original);
    }

    /// Clamped and NaN-free.
    #[test]
    fn stored_gains_come_back_sane() {
        let params = EqParams::new(true, &[99.0, -99.0, f32::NAN, 3.5], &[], &[]);
        assert_eq!(params.gain(0), GAIN_MAX_DB);
        assert_eq!(params.gain(1), -GAIN_MAX_DB);
        assert_eq!(params.gain(2), 0.0);
        assert_eq!(params.gain(3), 3.5);
        assert_eq!(params.gains().len(), BANDS);
        assert_eq!(params.gain(BANDS - 1), 0.0);
        params.flatten();
        assert!(params.gains().iter().all(|db| *db == 0.0));
    }

    /// A file with gains only loads onto the ISO octaves at one octave wide.
    #[test]
    fn a_pre_parametric_file_loads_onto_the_iso_octaves() {
        let params = EqParams::new(true, &[3.0; BANDS], &[], &[]);
        assert_eq!(params.freqs(), BAND_HZ.to_vec());
        assert!(params.qs().iter().all(|q| *q == Q_DEFAULT));
    }

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

    /// The plot uses the filter's own coefficients.
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
        // Beyond this band's and its neighbours' reach.
        let far = params.response_db(60.0, RATE);
        assert!(far < at_center, "the bell has to fall off, read {far}");
    }

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
        let (wide_off, narrow_off) = (
            wide.response_db(1400.0, RATE),
            narrow.response_db(1400.0, RATE),
        );
        assert!(
            wide_off > narrow_off,
            "wide {wide_off} should still be lifting where narrow {narrow_off} has let go"
        );
    }

    #[test]
    fn a_disabled_eq_plots_flat() {
        let params = EqParams::new(false, &[12.0; BANDS], &[], &[]);
        for hz in [50.0, 500.0, 5000.0] {
            assert_eq!(params.response_db(hz, RATE), 0.0);
        }
    }

    /// A center move retunes on the next buffer, like a gain move.
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

    #[test]
    fn apply_graphic_curve_resets_shape_and_sets_gains() {
        let params = EqParams::new(true, &[], &[], &[]);
        params.set_freq(0, 50.0);
        params.set_q(0, 4.0);
        let gains = [1.0, 2.0, 3.0, 4.0, 5.0, -1.0, -2.0, -3.0, -4.0, -5.0];
        params.apply_graphic_curve(&gains);
        assert_eq!(params.freq(0), BAND_HZ[0]);
        assert_eq!(params.q(0), Q_DEFAULT);
        for band in 0..BANDS {
            assert_eq!(params.gain(band), gains[band]);
        }
    }
}
