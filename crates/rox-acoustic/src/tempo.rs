//! A track's tempo in BPM, off the novelty curve the acoustic sketch already
//! computes: rectified spectral flux (`novelty_split`), local-mean
//! subtraction, autocorrelation, and a comb over fractional lags scoring each
//! candidate by its multiples.
//!
//! The comb finds a repeat, not yet a tempo. `grid` divides the winner down
//! to the shortest repeat, `stretch` re-measures it against its furthest
//! multiple, and `fold_into_band` picks the octave, using a second novelty
//! curve of only the low band: kicks land there and hats don't, which tells a
//! fast beat from a subdivision. Two windows vote and `combine` needs
//! agreement.
//!
//! Good at steady grids (house, techno, most pop). Not a beat tracker: when
//! windows disagree the search widens a pair at a time until a majority
//! forms, and a track with none is refused. Half time still reads at its
//! grid, and triplet material folds by powers of two.
//!
//! [`estimate`] tells a refusal (`Ok(None)`, recorded so the next pass skips
//! the track) from [`Unreadable`] (nothing decoded; worth asking again).

use std::path::Path;

use crate::{HOP, RATE, novelty_split};

/// Thirty seconds is sixty beats at 120, enough for a lag to stand out; a
/// ten second window just votes on whatever fill it covered.
const WINDOW_SECS: f64 = 30.0;
/// A third and two thirds in: past the intro, before the outro, far enough
/// apart that a tempo change shows as disagreement.
const PROBES: [f64; 2] = [1.0 / 3.0, 2.0 / 3.0];
/// Shorter tracks get one window from the top; two would mostly overlap.
const SINGLE_SECS: f64 = 35.0;
/// Where a split vote widens to, a balanced pair at a time: sixths, then
/// quarters, stopping at the first majority. Six windows (three minutes of
/// decoding) is the limit.
///
/// More windows, not a longer one: a double-length middle window was measured
/// worse, because straddling two tempos invents a compromise (128 and 90 read
/// as 135). Pairs, because a lone extra window hands its half an unearned
/// majority; no middle, because that's where the seam is.
const WIDEN: [[f64; 2]; 2] = [[1.0 / 6.0, 5.0 / 6.0], [1.0 / 4.0, 3.0 / 4.0]];

const FPS: f32 = RATE as f32 / HOP as f32;

/// The band answers are folded into by octaves. The top sits above 200
/// because measurements land near a tempo: 200 reads back as 200.1, and a
/// cap at 200 would file it at 100. It stays under ~216, where the prior
/// starts beating [`OCTAVE_BIAS`].
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 210.0;

/// The fastest grid searched: not a tempo but the shortest repeat (sixteenth
/// hats run at four times the tempo). Finding it first and halving keeps a
/// 120 BPM house track off 80.
const FAST_BPM: f32 = 400.0;
/// 6.5 to 43.1 frames at 44.1 kHz.
const LAG_MIN: f32 = FPS * 60.0 / FAST_BPM;
const LAG_MAX: f32 = FPS * 60.0 / MIN_BPM;

/// Divisions tried on the winning lag, longest first, and the share of the
/// winner's correlation a division must keep. The best repeat is often a
/// grouping (a bar, three beats of kick-hat) and the longest division that
/// holds is the grid. Two fifths because a backbeat makes the bar correlate
/// far better than the beat: measured on kit patterns the beat reaches only
/// half to two thirds, and stricter reads them at half speed.
const DIVISORS: [f32; 4] = [5.0, 4.0, 3.0, 2.0];
const SUPPORT: f32 = 0.4;

/// Three covers a sixteenth grid at the top down to the bottom of the band.
const OCTAVES: u32 = 3;
/// What each halving costs against the prior, so 174 doesn't read as 87 just
/// for being nearer 120. The prior's octave ratio tops out near 0.75, so the
/// prior alone never halves an in-band grid. At 0.85 the crossover was
/// ~186 BPM, halving happy hardcore.
const OCTAVE_BIAS: f32 = 0.65;

/// A fiftieth of a frame: beats fall between integer lags (120 BPM is 21.53
/// frames), and multiples only line up for a fractional candidate. Well
/// under a peak's width, since [`refine`] reads its shape.
const STEP: f32 = 0.02;
const STEPS: usize = ((LAG_MAX - LAG_MIN) / STEP) as usize + 1;

/// Three times the slowest lag: scores read the third multiple.
const LAGS: usize = (LAG_MAX * 3.0) as usize + 2;

/// Eight beats at the slowest tempo in the band.
const MIN_SECS: f32 = 8.0;
const MIN_FRAMES: usize = (FPS * MIN_SECS) as usize;

/// About 370 ms: longer than a beat's rise, shorter than any in-band beat,
/// so a swell flattens and the beat doesn't.
const LOCAL_MEAN: usize = 16;

/// A hit is one frame of flux, so a 21.53-frame beat hits 21 or 22 and both
/// lags read half strength while four beats reads full: a quarter-speed
/// answer. Spreading over five frames fixes that.
const SMEAR: [f32; 5] = [1.0 / 9.0, 2.0 / 9.0, 3.0 / 9.0, 2.0 / 9.0, 1.0 / 9.0];

/// A real period peaks again at its multiples. Half tempo does too, which
/// [`HALF_PENALTY`] handles.
const HARMONIC_2: f32 = 0.5;
const HARMONIC_3: f32 = 0.25;
/// A peak at half the lag means a faster beat. Gentle, or offbeat hats would
/// push tracks to double time.
const HALF_PENALTY: f32 = 0.4;
/// Confidence is a fraction of this.
const FULL_SCORE: f32 = 1.0 + HARMONIC_2 + HARMONIC_3;

/// Gaussian on log2 BPM, 0.9 octaves wide: 70-180 all weigh above three
/// quarters, so it only picks among halvings.
const PRIOR_CENTRE: f32 = 120.0;
const PRIOR_OCTAVES: f32 = 0.9;

/// Four percent, 5 BPM at 128: wider than the grid resolves, narrower than a
/// real tempo change.
const AGREE: f32 = 0.04;
/// How far above the median lag the winner must be. Measured: noise ~0.06,
/// a held tone ~0.09, random clicks ~0.15, synthesized kits 0.45-1.0. A
/// quarter sits on the loose side of the gap.
const CONFIDENCE_FLOOR: f32 = 0.25;

/// Wider than the search: this is the contract with storage.
const OUT_MIN: f32 = 40.0;
const OUT_MAX: f32 = 300.0;

/// Nothing decoded: missing, truncated, or undecodable. Not a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

#[derive(Clone, Copy, Debug)]
struct Vote {
    bpm: f32,
    confidence: f32,
}

/// `Ok(Some)` a tempo, `Ok(None)` heard but no straight answer,
/// [`Unreadable`] nothing decoded (try again later).
///
/// Two 30 s windows at 44.1 kHz, a minute of decoding; splits cost a minute
/// per widening pair. A cue subsong is probed from the top of its image, the
/// same as [`crate::extract`]: both read through
/// [`rox_playback::engine::decode_window`], which has no span, so they
/// change together.
pub fn estimate(path: &Path, duration_ms: u32) -> Result<Option<f32>, Unreadable> {
    let duration = duration_ms as f64 / 1000.0;
    let path = path.to_path_buf();
    let single = duration <= SINGLE_SECS;
    let span = (duration - WINDOW_SECS).max(0.0);

    // One decoded window makes the outcome a verdict.
    let mut decoded = false;
    let mut votes = Vec::with_capacity(PROBES.len() + WIDEN.len() * 2);
    for probe in PROBES {
        let at = if single { 0.0 } else { span * probe };
        if let Ok(vote) = probe_window(&path, at) {
            decoded = true;
            votes.extend(vote);
        }
        if single {
            break;
        }
    }
    let mut answer = combine(&votes);
    // Confident windows that disagree widen the search until a majority forms
    // or [`WIDEN`] runs out. Fewer than two confident votes isn't a
    // disagreement more windows could settle.
    let split = |votes: &[Vote]| {
        votes
            .iter()
            .filter(|v| v.confidence >= CONFIDENCE_FLOOR)
            .count()
            >= 2
    };
    let mut widened = false;
    if !single {
        for pair in WIDEN {
            if answer.is_some() || !split(&votes) {
                break;
            }
            widened = true;
            for probe in pair {
                if let Ok(vote) = probe_window(&path, span * probe) {
                    decoded = true;
                    votes.extend(vote);
                }
            }
            answer = combine(&votes);
        }
    }
    if widened && answer.is_none() {
        log::debug!("tempo: {}: windows disagree, {:?}", path.display(), votes);
    }
    // Unreadable, not refused: a refusal mark sticks.
    if !decoded {
        return Err(Unreadable);
    }
    Ok(answer)
}

/// `Ok(None)` for a window with no tempo, [`Unreadable`] (logged) for one
/// that won't decode.
fn probe_window(path: &Path, at: f64) -> Result<Option<Vote>, Unreadable> {
    let frames = (WINDOW_SECS * RATE as f64) as usize;
    let locator = rox_library::locator::Locator::Local(path.to_path_buf());
    match rox_playback::engine::decode_window(&locator, at, RATE, frames) {
        Ok(stereo) => {
            let mono: Vec<f32> = stereo
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (c[0] + c[1]) * 0.5)
                .collect();
            Ok(window(&mono))
        }
        Err(e) => {
            log::debug!("tempo: {}: {e}", path.display());
            Err(Unreadable)
        }
    }
}

fn window(mono: &[f32]) -> Option<Vote> {
    let (curve, drums) = novelty_split(mono);
    vote(&curve, &drums)
}

/// The search runs on the full band; the drum curve informs the octave.
fn vote(curve: &[f32], drums: &[f32]) -> Option<Vote> {
    if curve.len() < MIN_FRAMES {
        return None;
    }
    let peaks = sharpen(curve);
    // Silence and held tones get here.
    let r = correlate(&peaks, LAGS.min(peaks.len() - 1))?;
    // No drums means an empty low curve, which reads zero everywhere and leaves
    // the full band deciding.
    let low =
        correlate(&sharpen(drums), LAGS.min(drums.len().saturating_sub(1))).unwrap_or_default();

    let scores: Vec<f32> = (0..STEPS)
        .map(|i| score(&r, LAG_MIN + i as f32 * STEP))
        .collect();
    let mut best = 0;
    for (i, value) in scores.iter().enumerate() {
        if *value > scores[best] {
            best = i;
        }
    }
    let top = scores[best];
    // No positive correlation, or NaN samples.
    if !top.is_finite() || top <= 0.0 {
        return None;
    }
    // Parabolic interpolation between scan steps. Dividing the long lag down is
    // more precise than measuring the grid: four beats has a quarter the error
    // per beat.
    let lag = LAG_MIN + (best as f32 + refine(&scores, best)) * STEP;

    let bpm = fold_into_band(&r, &low, stretch(&r, grid(&r, &low, lag)))?;
    // How much better than the median lag the winner is; a periodless curve
    // still has a best lag.
    let mut band = scores;
    band.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let confidence = ((top - band[band.len() / 2]) / FULL_SCORE).clamp(0.0, 1.0);
    Some(Vote { bpm, confidence })
}

/// The longest division of `lag` either curve still repeats at nearly as
/// strongly, else `lag`: keeps a house track off three quarters of its tempo.
/// Each curve is judged against its own strength; the drums matter at fast
/// tempos, where a backbeat fails the full-band test but the kicks pass.
fn grid(r: &[f32], drums: &[f32], lag: f32) -> f32 {
    let strength = at(r, lag);
    let drum_strength = at(drums, lag);
    let holds = |curve: &[f32], strength: f32, shorter: f32| {
        let held = at(curve, shorter);
        strength > 0.0 && held > 0.0 && held >= SUPPORT * strength
    };
    for divisor in DIVISORS {
        let shorter = lag / divisor;
        if shorter >= LAG_MIN
            && (holds(r, strength, shorter) || holds(drums, drum_strength, shorter))
        {
            return shorter;
        }
    }
    lag
}

/// Re-measure `lag` at its furthest multiple in reach, dividing the 23 ms
/// frame error by the beats spanned. Searched within one beat of the
/// expected multiple.
fn stretch(r: &[f32], lag: f32) -> f32 {
    if r.len() < 4 || lag < 2.0 {
        return lag;
    }
    let reach = ((r.len() - 2) as f32 / lag).floor();
    if reach < 2.0 {
        return lag;
    }
    let target = lag * reach;
    let window = (lag * 0.4) as usize;
    let lo = (target as usize).saturating_sub(window).max(1);
    let hi = ((target as usize) + window).min(r.len() - 2);
    let peak = (lo..=hi).fold(lo, |best, i| if r[i] > r[best] { i } else { best });
    let curvature = r[peak - 1] - 2.0 * r[peak] + r[peak + 1];
    let offset = if curvature.abs() < f32::EPSILON {
        0.0
    } else {
        (0.5 * (r[peak - 1] - r[peak + 1]) / curvature).clamp(-0.5, 0.5)
    };
    (peak as f32 + offset) / reach
}

/// The grid as an in-band tempo, halving until it fits, never doubling.
/// Prior, halving cost, and correlation on either curve decide together; the
/// low band tells a kick-snare backbeat at 200 from a kick-hat pattern an
/// octave too fast.
fn fold_into_band(r: &[f32], drums: &[f32], lag: f32) -> Option<f32> {
    let mut best: Option<(f32, f32)> = None;
    for octave in 0..=OCTAVES {
        let lag = lag * (1 << octave) as f32;
        let bpm = FPS * 60.0 / lag;
        if bpm < MIN_BPM {
            break;
        }
        // The better curve's correlation, less active refutation from the drums: a
        // negative drum correlation means drums land between these beats.
        let drum = at(drums, lag);
        let heard = at(r, lag).max(drum) + drum.min(0.0);
        // A halving neither curve correlates at is arithmetic: 60 stays 60.
        if bpm > MAX_BPM || (octave > 0 && heard <= 0.0) {
            continue;
        }
        let weight = prior(bpm) * OCTAVE_BIAS.powi(octave as i32) * heard.max(0.0);
        if best.is_none_or(|(top, _)| weight > top) {
            best = Some((weight, bpm));
        }
    }
    best.map(|(_, bpm)| bpm)
}

/// Each frame over its local mean, rectified, then centred. Without the local
/// mean a loud passage correlates with itself; without centring every lag
/// correlates.
fn sharpen(curve: &[f32]) -> Vec<f32> {
    let n = curve.len();
    let mut running = 0f64;
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0f64);
    for &v in curve {
        running += v as f64;
        prefix.push(running);
    }
    let rectified: Vec<f32> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(LOCAL_MEAN);
            let hi = (i + LOCAL_MEAN + 1).min(n);
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (curve[i] as f64 - mean).max(0.0) as f32
        })
        .collect();
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            SMEAR
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    let j = i + k;
                    let j = j.checked_sub(SMEAR.len() / 2).filter(|j| *j < n);
                    w * j.map_or(0.0, |j| rectified[j])
                })
                .sum()
        })
        .collect();
    let mean = (out.iter().map(|v| *v as f64).sum::<f64>() / n as f64) as f32;
    for v in &mut out {
        *v -= mean;
    }
    out
}

/// Autocorrelation up to `max_lag`, each lag averaged over its products and
/// normalized by lag zero. None for a curve with no energy.
fn correlate(x: &[f32], max_lag: usize) -> Option<Vec<f32>> {
    let n = x.len();
    let mut r: Vec<f32> = (0..=max_lag)
        .map(|lag| {
            let sum: f64 = x[..n - lag]
                .iter()
                .zip(&x[lag..])
                .map(|(a, b)| (*a as f64) * (*b as f64))
                .sum();
            (sum / (n - lag) as f64) as f32
        })
        .collect();
    let energy = r[0];
    if !energy.is_finite() || energy <= 0.0 {
        return None;
    }
    for v in &mut r {
        *v /= energy;
    }
    Some(r)
}

/// Linear between integer lags, zero past the end.
fn at(r: &[f32], lag: f32) -> f32 {
    if lag <= 0.0 {
        return 0.0;
    }
    let lo = lag.floor() as usize;
    if lo + 1 >= r.len() {
        return 0.0;
    }
    let frac = lag - lo as f32;
    r[lo] * (1.0 - frac) + r[lo + 1] * frac
}

/// Own correlation, plus multiples, less a halfway peak.
fn score(r: &[f32], lag: f32) -> f32 {
    let own = at(r, lag);
    // Multiples only reinforce a period the lag itself correlates at, or the
    // lag 1.5 beats long scores off its double.
    if own <= 0.0 {
        return own;
    }
    let support = own + HARMONIC_2 * at(r, lag * 2.0) + HARMONIC_3 * at(r, lag * 3.0);
    support - HALF_PENALTY * at(r, lag / 2.0).max(0.0)
}

fn prior(bpm: f32) -> f32 {
    let octaves = (bpm / PRIOR_CENTRE).log2() / PRIOR_OCTAVES;
    (-0.5 * octaves * octaves).exp()
}

/// Clamped to half a frame: a parabola through noise can overshoot to a neighbour.
fn refine(scores: &[f32], best: usize) -> f32 {
    if best == 0 || best + 1 >= scores.len() {
        return 0.0;
    }
    let (a, b, c) = (scores[best - 1], scores[best], scores[best + 1]);
    let curvature = a - 2.0 * b + c;
    if curvature.abs() < f32::EPSILON {
        return 0.0;
    }
    (0.5 * (a - c) / curvature).clamp(-0.5, 0.5)
}

/// Doubled or halved into `toward`'s octave.
fn fold(bpm: f32, toward: f32) -> f32 {
    if bpm <= 0.0 || toward <= 0.0 {
        return bpm;
    }
    bpm * (toward / bpm).log2().round().exp2()
}

/// Every vote anchors a reading of all votes within [`AGREE`] once folded;
/// the most confident reading wins as their weighted mean, so 87 joins 174.
/// Sub-floor votes count for nothing; confident votes outside count against,
/// and the answer needs a majority. One against one widens the search;
/// three different answers stay refused.
fn combine(votes: &[Vote]) -> Option<f32> {
    let agrees = |anchor: &Vote, v: &Vote| {
        (fold(v.bpm, anchor.bpm) - anchor.bpm).abs() <= anchor.bpm * AGREE
    };
    let weight_of = |anchor: &Vote| -> f32 {
        votes
            .iter()
            .filter(|v| agrees(anchor, v))
            .map(|v| v.confidence)
            .sum()
    };
    let anchor = votes
        .iter()
        .copied()
        .reduce(|a, b| if weight_of(&b) > weight_of(&a) { b } else { a })?;
    let mut sum = 0.0;
    let mut weight = 0.0;
    let mut inside = 0usize;
    let mut outside = 0usize;
    for vote in votes {
        if agrees(&anchor, vote) {
            sum += fold(vote.bpm, anchor.bpm) * vote.confidence;
            weight += vote.confidence;
            if vote.confidence >= CONFIDENCE_FLOOR {
                inside += 1;
            }
        } else if vote.confidence >= CONFIDENCE_FLOOR {
            outside += 1;
        }
    }
    let bpm = if weight > 0.0 {
        sum / weight
    } else {
        anchor.bpm
    };
    // No confident majority, no answer: this also floors the anchor's confidence.
    if inside <= outside || !(OUT_MIN..=OUT_MAX).contains(&bpm) {
        return None;
    }
    Some(bpm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// Added in, so hits overlap like a kit's.
    fn hit(buf: &mut [f32], at: usize, hz: f32, gain: f32, len: usize) {
        for i in 0..len {
            let Some(slot) = buf.get_mut(at + i) else {
                break;
            };
            let decay = 1.0 - i as f32 / len as f32;
            *slot += decay * decay * gain * (TAU * hz * i as f32 / RATE as f32).sin();
        }
    }

    /// Fractional in frames at every tempo tested.
    fn clicks(bpm: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let period = 60.0 / bpm * RATE as f32;
        let mut beat = 0usize;
        while ((beat as f32 * period) as usize) < n {
            hit(&mut buf, (beat as f32 * period) as usize, 2000.0, 0.8, 900);
            beat += 1;
        }
        buf
    }

    /// Kick on the beat and, when `offbeat` > 0, a hat between.
    fn kit(bpm: f32, secs: f32, offbeat: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let period = 60.0 / bpm * RATE as f32;
        let mut beat = 0usize;
        while ((beat as f32 * period) as usize) < n {
            let at = (beat as f32 * period) as usize;
            hit(&mut buf, at, 70.0, 0.9, 4000);
            if offbeat > 0.0 {
                hit(&mut buf, at + (period / 2.0) as usize, 7000.0, offbeat, 700);
            }
            beat += 1;
        }
        buf
    }

    /// Kick, backbeat snare, offbeat hats, bass: the bar out-correlates the
    /// beat, so the tempo comes only from dividing.
    fn band(bpm: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let beat = 60.0 / bpm * RATE as f32;
        let mut count = 0usize;
        while ((count as f32 * beat) as usize) < n {
            let at = (count as f32 * beat) as usize;
            hit(&mut buf, at, 70.0, 0.9, 4000);
            hit(&mut buf, at, 110.0, 0.4, 8000);
            if count % 2 == 1 {
                hit(&mut buf, at, 900.0, 0.7, 2500);
                hit(&mut buf, at, 5500.0, 0.5, 1800);
            }
            hit(&mut buf, at + (beat / 2.0) as usize, 8000.0, 0.25, 500);
            count += 1;
        }
        buf
    }

    /// Half time: kick on one, snare on three, hats in eighths.
    fn halftime(bpm: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let beat = 60.0 / bpm * RATE as f32;
        let mut count = 0usize;
        while ((count as f32 * beat) as usize) < n {
            let at = (count as f32 * beat) as usize;
            match count % 4 {
                0 => hit(&mut buf, at, 60.0, 1.0, 5000),
                2 => {
                    hit(&mut buf, at, 900.0, 0.8, 2500);
                    hit(&mut buf, at, 5500.0, 0.6, 1800);
                }
                _ => {}
            }
            hit(&mut buf, at, 8000.0, 0.3, 500);
            hit(&mut buf, at + (beat / 2.0) as usize, 8000.0, 0.3, 500);
            count += 1;
        }
        buf
    }

    fn ramp(from: f32, to: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let mut at = 0f32;
        while (at as usize) < n {
            hit(&mut buf, at as usize, 2000.0, 0.8, 900);
            at += 60.0 / (from + (to - from) * at / n as f32) * RATE as f32;
        }
        buf
    }

    /// Random intervals of 0.1 to 0.8 s: onsets, no grid.
    fn scatter(secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut at = 0usize;
        while at < n {
            hit(&mut buf, at, 2000.0, 0.8, 900);
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            at += 4410 + ((state >> 40) as usize % 30_870);
        }
        buf
    }

    fn noise(secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / 8388608.0 - 1.0) * 0.5
            })
            .collect()
    }

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| (TAU * hz * i as f32 / RATE as f32).sin() * 0.5)
            .collect()
    }

    /// One window through [`estimate`]'s acceptance.
    fn answer(mono: &[f32]) -> Option<f32> {
        combine(&[window(mono)?])
    }

    fn error(got: f32, want: f32) -> f32 {
        (got - want).abs() / want
    }

    /// Kick on one and three, snare (with low body) on two and four, strummed
    /// eighths at `strum`. The tempo is the kit's.
    fn rock(bpm: f32, secs: f32, strum: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let mut buf = vec![0.0; n];
        let beat = 60.0 / bpm * RATE as f32;
        let mut count = 0usize;
        while ((count as f32 * beat) as usize) < n {
            let at = (count as f32 * beat) as usize;
            match count % 2 {
                0 => hit(&mut buf, at, 70.0, 0.9, 4000),
                _ => {
                    hit(&mut buf, at, 200.0, 0.7, 2500);
                    hit(&mut buf, at, 5500.0, 0.5, 1800);
                }
            }
            hit(&mut buf, at, 4000.0, strum, 900);
            hit(&mut buf, at + (beat / 2.0) as usize, 4000.0, strum, 900);
            count += 1;
        }
        buf
    }

    /// Across the band, 200 included, none a whole number of frames.
    #[test]
    fn a_click_track_reads_back_at_the_tempo_it_was_written_at() {
        for bpm in [85.0, 100.0, 120.0, 128.0, 140.0, 174.0, 200.0] {
            let got = answer(&clicks(bpm, 20.0)).expect("a click track has a tempo");
            assert!(
                error(got, bpm) < 0.01,
                "clicks at {bpm} read as {got:.2}, which is {:.1}% out",
                error(got, bpm) * 100.0
            );
        }
    }

    /// Hats brighter than the kick: a direct plausible-tempo search reads three quarters.
    #[test]
    fn hats_between_the_kicks_dont_move_the_tempo() {
        for bpm in [120.0, 174.0, 200.0] {
            for offbeat in [0.0, 0.5] {
                let got = answer(&kit(bpm, 20.0, offbeat)).expect("a kit pattern has a tempo");
                assert!(
                    error(got, bpm) < 0.02,
                    "a kit at {bpm} with a {offbeat} hat read as {got:.2}"
                );
            }
        }
    }

    /// The bar is the loudest repeat. Past ~185 the kicks carry the division and
    /// the fold must hold the octave against the prior.
    #[test]
    fn a_full_kit_reads_the_beat_and_not_the_bar() {
        for bpm in [124.0, 140.0, 174.0, 190.0, 195.0, 200.0] {
            let got = answer(&band(bpm, 20.0)).expect("a kit pattern has a tempo");
            assert!(
                error(got, bpm) < 0.01,
                "a band playing {bpm} read as {got:.2}"
            );
        }
    }

    /// A hat as loud as the kick: only the low band says which grid is the beat.
    #[test]
    fn hats_as_loud_as_the_kicks_still_read_the_kicks_tempo() {
        let got = answer(&kit(85.0, 20.0, 0.9)).expect("a kit pattern has a tempo");
        assert!(
            error(got, 85.0) < 0.02,
            "85 with hats as loud as the kicks read as {got:.2}"
        );
    }

    /// The Creedence case: loud strummed eighths win the full-band comb, and the
    /// drums fold it back.
    #[test]
    fn strummed_eighths_dont_double_a_backbeat() {
        for strum in [0.6, 0.9] {
            let got = answer(&rock(93.0, 20.0, strum)).expect("a rock pattern has a tempo");
            assert!(
                error(got, 93.0) < 0.02,
                "93 under strummed eighths at {strum} read as {got:.2}"
            );
        }
    }

    /// Unreadable octave: at 85 half-time kicks repeat past the longest lag, so
    /// the hats' grid gives 170. It must be an octave, never something between.
    #[test]
    fn a_halftime_pattern_is_read_an_octave_out() {
        let got = answer(&halftime(85.0, 20.0)).expect("a kit pattern has a tempo");
        assert!(
            error(fold(got, 85.0), 85.0) < 0.02,
            "half time at 85 read as {got:.2}, which is not an octave of it"
        );
        assert!(got > 150.0, "and today it is the double, {got:.2}");
    }

    /// Silence, noise, a tone, and random clicks are refused.
    #[test]
    fn silence_and_noise_and_scattered_hits_are_refused() {
        assert_eq!(answer(&vec![0.0; 20 * RATE as usize]), None, "silence");
        assert_eq!(answer(&noise(20.0)), None, "steady noise");
        assert_eq!(answer(&tone(440.0, 20.0)), None, "a held tone");
        assert_eq!(answer(&scatter(20.0)), None, "clicks at random intervals");
    }

    /// Two seconds is four beats: visible, not believable.
    #[test]
    fn too_short_a_window_is_refused() {
        assert!(window(&clicks(120.0, 2.0)).is_none());
    }

    /// A ramp still answers from inside it. Refusing music that moves is
    /// [`combine`]'s job across windows.
    #[test]
    fn a_tempo_that_ramps_answers_from_inside_the_ramp() {
        let got = answer(&ramp(120.0, 132.0, 20.0)).expect("a ramp still correlates");
        assert!(
            (120.0..=132.0).contains(&got),
            "a 120 to 132 ramp read as {got:.2}"
        );
    }

    /// The only test through real files: probe placement and decode.
    #[test]
    fn a_file_on_disk_reads_back_at_its_tempo() {
        let dir = std::env::temp_dir().join(format!("rox-tempo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        let secs = 70.0;
        std::fs::write(&path, wav(&band(128.0, secs))).unwrap();

        let got = estimate(&path, (secs * 1000.0) as u32);
        let _ = std::fs::remove_dir_all(&dir);
        let got = got
            .expect("a readable file")
            .expect("a click track on disk has a tempo");
        assert!(error(got, 128.0) < 0.01, "128 on disk read as {got:.2}");
    }

    /// A 30 s bridge at 90 under one probe of a 128 track: the first widening
    /// pair outvotes it.
    #[test]
    fn a_bridge_under_one_probe_is_outvoted() {
        let dir = std::env::temp_dir().join(format!("rox-tempo-bridge-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bridge.wav");
        let mut audio = band(128.0, 85.0);
        audio.extend(band(90.0, 30.0));
        audio.extend(band(128.0, 45.0));
        let secs = audio.len() as f32 / RATE as f32;
        std::fs::write(&path, wav(&audio)).unwrap();

        let got = estimate(&path, (secs * 1000.0) as u32);
        let _ = std::fs::remove_dir_all(&dir);
        let got = got
            .expect("a readable file")
            .expect("a majority should settle the split");
        assert!(
            error(got, 128.0) < 0.02,
            "the track runs at 128 around its bridge, read {got:.2}"
        );
    }

    /// A real tempo change splits every pair and stays refused.
    #[test]
    fn a_track_that_changes_tempo_splits_every_vote_and_refuses() {
        let dir = std::env::temp_dir().join(format!("rox-tempo-seam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seam.wav");
        let mut audio = band(128.0, 75.0);
        audio.extend(band(90.0, 85.0));
        let secs = audio.len() as f32 / RATE as f32;
        std::fs::write(&path, wav(&audio)).unwrap();

        let got = estimate(&path, (secs * 1000.0) as u32);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            got,
            Ok(None),
            "an even split is a track with two tempos, and the file read fine"
        );
    }

    /// Unreadable (no file) versus refused (noise): only a refusal gets marked.
    #[test]
    fn a_file_that_wont_decode_is_told_apart_from_one_with_no_tempo() {
        let dir = std::env::temp_dir().join(format!("rox-tempo-unread-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            estimate(&dir.join("nothing-here.wav"), 200_000),
            Err(Unreadable),
            "a file that isn't there was never listened to"
        );

        let path = dir.join("noise.wav");
        let secs = 70.0;
        std::fs::write(&path, wav(&noise(secs))).unwrap();
        let got = estimate(&path, (secs * 1000.0) as u32);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Ok(None), "noise was heard and has no tempo in it");
    }

    fn wav(mono: &[f32]) -> Vec<u8> {
        let bytes = mono.len() as u32 * 2;
        let mut out = Vec::with_capacity(bytes as usize + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + bytes).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&(RATE * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&bytes.to_le_bytes());
        for sample in mono {
            let clipped = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            out.extend_from_slice(&clipped.to_le_bytes());
        }
        out
    }

    /// Agreement averages, one against one refuses, and 87 and 174 are one track.
    #[test]
    fn windows_agree_across_an_octave_and_a_majority_settles_a_split() {
        let vote = |bpm, confidence| Vote { bpm, confidence };

        let mean = combine(&[vote(128.0, 0.8), vote(129.0, 0.4)]).unwrap();
        assert!(
            (128.3..128.4).contains(&mean),
            "two windows either side of 128 gave {mean}"
        );

        let octave = combine(&[vote(174.0, 0.9), vote(87.0, 0.5)]).unwrap();
        assert!(
            error(octave, 174.0) < 0.001,
            "87 should fold onto 174, gave {octave}"
        );

        assert_eq!(
            combine(&[vote(128.0, 0.4), vote(90.0, 0.9)]),
            None,
            "one against one has no majority, however confident either side"
        );

        let majority = combine(&[vote(128.0, 0.5), vote(90.0, 0.9), vote(128.5, 0.45)]).unwrap();
        assert!(
            (128.0..128.5).contains(&majority),
            "two windows against one settle on the two, gave {majority}"
        );

        assert_eq!(
            combine(&[vote(128.0, 0.5), vote(90.0, 0.9), vote(150.0, 0.45)]),
            None,
            "three windows that heard three tempos is a track that moves"
        );

        let noisy = combine(&[vote(128.0, CONFIDENCE_FLOOR - 0.01), vote(90.0, 0.9)]).unwrap();
        assert_eq!(
            noisy, 90.0,
            "a window that couldn't hear a tempo doesn't veto one that could"
        );

        assert_eq!(
            combine(&[vote(128.0, CONFIDENCE_FLOOR - 0.01)]),
            None,
            "under the floor nothing is stored"
        );
        assert_eq!(combine(&[]), None, "and neither window decoded");
    }

    /// A folded answer always lands in the band.
    #[test]
    fn an_answer_is_always_inside_the_band() {
        for bpm in [60.0, 85.0, 120.0, 174.0, 200.0] {
            let got = combine(&[Vote {
                bpm,
                confidence: 0.9,
            }])
            .unwrap();
            assert!((OUT_MIN..=OUT_MAX).contains(&got), "{bpm} came back {got}");
        }
        assert_eq!(fold(43.5, 174.0), 174.0, "two doublings");
        assert_eq!(fold(348.0, 174.0), 174.0, "one halving");
        assert_eq!(fold(174.0, 174.0), 174.0, "and nothing to do");
    }
}
