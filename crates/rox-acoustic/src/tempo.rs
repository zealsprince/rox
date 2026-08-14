//! How fast a track runs, in beats per minute, off the same novelty curve
//! the acoustic sketch already computes.
//!
//! The method is the standard one and makes no apology for it: half-wave
//! rectified spectral flux (the crate's own `novelty_split`), local-mean
//! subtraction so the curve is peaks rather than loudness,
//! autocorrelation, and a comb over fractional lags that scores each
//! candidate with what its own multiples are worth.
//!
//! What the comb finds is a repeat, which is not yet a tempo. Three steps
//! turn it into one: `grid` divides the winner down to the shortest thing
//! the track repeats at, `stretch` measures that grid again against the
//! furthest multiple of itself the correlation reaches, and
//! `fold_into_band` picks which octave of it the range a tempo is quoted
//! in gets. The octave is the one question the full-band curve can't
//! answer, since a hat between two kicks makes as much flux as a third
//! kick would, so a second novelty curve of just the low band rides along:
//! kick and snare land in it, hats and strums don't, and whether the drums
//! repeat at a candidate lag or between it is what tells a fast beat from
//! a subdivision. Two windows a third and two thirds into the track do all
//! of that separately, and `combine` wants them to agree.
//!
//! ## What it's good at, and what it isn't
//!
//! A steady grid: house, techno, drum and bass, most pop and hip hop, the
//! whole body of music produced against a click. There the answer is the
//! tempo the producer typed into the DAW, give or take the grid this
//! transform can resolve.
//!
//! It is not a beat tracker and it doesn't pretend to follow music that
//! moves. Rubato, a ritardando, a live drummer drifting, a track that
//! changes tempo halfway: when the two windows disagree, the search
//! widens a pair of windows at a time until a majority settles it or the
//! widening reaches its limit. A majority is stored as the track's tempo,
//! and a track whose windows split evenly or scatter to the end is
//! refused, since a number that describes one window of a track that
//! moves isn't the track's tempo.
//!
//! The octave is the error this class of estimator makes constantly, and
//! the drum band reads most of it: a backbeat under strummed eighths folds
//! to the beat the drums are on, and a fast four on the floor keeps its
//! own tempo instead of the half the prior would rather hear. What's left
//! is genuinely unreadable. Half time, where the whole kit marks every
//! fourth grid unit, still reads at the grid the track actually runs on.
//! Triplet and waltz material has its own version: the fold into the band
//! only ever halves, so a grid counted in threes lands wherever a power of
//! two from it lands.
//!
//! Nothing here decides what a missing answer means. [`estimate`] returns
//! None and the caller stores NULL, which is how a track that was measured
//! and refused stays distinguishable from one nobody has looked at.

use std::path::Path;

use crate::{novelty_split, HOP, RATE};

/// How much audio one window reads. Thirty seconds is sixty beats at 120
/// BPM, enough for a lag to repeat often enough to stand out of the noise;
/// a ten second window, which is all the sketch needs, votes on whatever
/// fill it happened to land in.
const WINDOW_SECS: f64 = 30.0;
/// Where the windows sit across the span a window can still start in. A
/// third and two thirds in: past the intro, before the outro, and far
/// enough apart that a tempo change between them shows up as disagreement.
const PROBES: [f64; 2] = [1.0 / 3.0, 2.0 / 3.0];
/// A track with less than this in it gets one window from the top instead
/// of two. Below about a window and a bit, the two probes would overlap so
/// heavily that the second one is the first one's opinion again.
const SINGLE_SECS: f64 = 35.0;
/// Where the search widens to when the probes disagree: a balanced pair
/// at a time, one window from each side of the track, first at the sixths
/// and then at the quarters. Widening stops at the first majority, so
/// most splits are settled one pair in, and the second pair is only ever
/// read by a track still split after four windows. The list running out
/// is the limit: six windows is three minutes of decoding, and a track
/// that hasn't found a majority by then doesn't have one.
///
/// A split vote means a thirty-second sample wasn't representative
/// somewhere, so the search widens: more windows, not a longer one. One
/// double-length window over the middle was tried first and measured
/// worse, because a window straddling two tempos doesn't vote for either,
/// it invents a compromise between them (75 seconds of 128 against 85 of
/// 90 read back as a confident-enough 135). Separate windows each land in
/// one section and vote for what's actually there, and the majority
/// decides. The pairing and the missing middle follow from the same two
/// facts: a lone extra window would hand whichever half it landed in a
/// majority a two-tempo track didn't earn, and the middle of a track
/// whose halves disagree is the seam itself, the one place a window is
/// guaranteed to straddle.
const WIDEN: [[f64; 2]; 2] = [[1.0 / 6.0, 5.0 / 6.0], [1.0 / 4.0, 3.0 / 4.0]];

/// Frames of novelty per second: one per hop.
const FPS: f32 = RATE as f32 / HOP as f32;

/// The band an answer comes out in. Anything slower than 60 is heard as
/// half time and anything much faster than 200 as double, so the answer is
/// folded into here by octaves rather than reported outside it.
///
/// The top sits above 200 rather than at it because a measurement lands
/// near a tempo, not on it: happy hardcore written at 200 reads back at
/// 200.1, and a cap at exactly 200 would rule the true octave out and file
/// the track at 100. The headroom stays under the ~216 where the prior's
/// pull across one octave catches up with [`OCTAVE_BIAS`], so everywhere
/// inside the band a grid keeps its own octave unless the curves argue
/// otherwise.
const MIN_BPM: f32 = 60.0;
const MAX_BPM: f32 = 210.0;

/// The fastest grid the search looks for. Not a tempo: the shortest thing
/// the track repeats at, which for a track with sixteenth hats is four
/// times its tempo. Finding that first and folding it into the band by
/// halving is what keeps a 120 BPM house track off 80, which is where a
/// search that only looked at plausible tempos lands it (the lag one and a
/// half beats long has a real peak two beats out to lean on, and no beat
/// halfway through it to give it away).
const FAST_BPM: f32 = 400.0;
/// The lags that band is, in frames: 6.5 to 43.1 at 44.1 kHz.
const LAG_MIN: f32 = FPS * 60.0 / FAST_BPM;
const LAG_MAX: f32 = FPS * 60.0 / MIN_BPM;

/// What the winning lag is divided by on the way down to the grid, longest
/// division first, and how strongly the curve has to repeat at the result
/// for the division to be taken.
///
/// The lag that scores best is whatever repeat the correlation reads most
/// cleanly, and that is regularly a grouping rather than a beat: two bars,
/// or three beats of a house track whose kick and hat alternate. Dividing
/// it back down is what finds the grid, and taking the longest division
/// that still holds finds the smallest grid unit rather than an arbitrary
/// grouping of it. The threshold is a share of the winner's own
/// correlation, so it asks whether the shorter lag is nearly as good a
/// repeat rather than whether it clears some absolute bar.
///
/// Two fifths, which is lower than it looks. A backbeat makes every second
/// beat carry a snare, so a bar correlates with itself far better than a
/// beat does with the beat after it: measured over synthesized kit patterns
/// the beat only reaches half to two thirds of the bar's correlation, and a
/// stricter threshold reads every one of them at half speed. What the
/// threshold is really there to reject is a division with nothing at it at
/// all, which is what the sign test above it catches.
const DIVISORS: [f32; 4] = [5.0, 4.0, 3.0, 2.0];
const SUPPORT: f32 = 0.4;

/// How many times the grid may be halved on its way into the band. Three
/// covers a sixteenth-note grid at the top of the search coming down to a
/// tempo at the bottom of the band.
const OCTAVES: u32 = 3;
/// What each halving costs against the prior. A tempo the track actually
/// repeats at is the answer unless half of it is a great deal better
/// supported, and this is what stops a 174 BPM track reading 87 because 87
/// is nearer 120.
///
/// Strong enough that the prior alone can never halve an in-band grid: the
/// prior's ratio across one octave tops out around 0.75 at the band's
/// edges, so a halving has to bring more correlation with it to win, which
/// a subdivision's halving does and a real beat's doesn't. At 0.85, where
/// this sat first, the arithmetic crossed over around 186 BPM and every
/// happy hardcore track in the band's top stripe read at half itself.
const OCTAVE_BIAS: f32 = 0.65;

/// How finely the lag range is walked, in frames.
///
/// A fiftieth of a frame, and the reason the comb is walked at all rather
/// than read off the integer lags. A beat at 120 BPM repeats every 21.53
/// frames, so a candidate at 21.5 has its second multiple at 43.07 and a
/// candidate at 21 has its at 42, which is between two beats: the multiples
/// are what separate one candidate from its neighbour, and they only line
/// up if the candidate can be a fraction. The step is well under the width
/// of a peak, because the peak's shape is what [`refine`] reads.
const STEP: f32 = 0.02;
const STEPS: usize = ((LAG_MAX - LAG_MIN) / STEP) as usize + 1;

/// How far the correlation is computed: three times the slowest lag, since
/// a candidate's score reads its own third multiple.
const LAGS: usize = (LAG_MAX * 3.0) as usize + 2;

/// A window shorter than this describes nothing. Eight seconds is eight
/// beats at the slowest tempo in the band, which is the floor for a lag
/// repeating often enough to mean anything.
const MIN_SECS: f32 = 8.0;
const MIN_FRAMES: usize = (FPS * MIN_SECS) as usize;

/// How far either side of a frame the local mean is taken, in frames.
/// Sixteen is about 370 ms, longer than a beat's own rise and shorter than
/// a beat at any tempo in the band, so subtracting it flattens a swell
/// without flattening the beat that rides on it.
const LOCAL_MEAN: usize = 16;

/// How a peak is spread across its neighbours before anything correlates
/// it.
///
/// A hit shows up in exactly one frame of flux, since only the frame where
/// the magnitude first rises counts as a rise, and one-frame spikes make a
/// correlation that is all or nothing: a beat every 21.53 frames lands on
/// frame 21 sometimes and 22 others, so both lags read half strength while
/// the lag at four beats, 43.07, reads nearly full, and the track is read
/// at a quarter speed. Spreading each peak over five frames costs timing
/// precision the grid never had, and it buys a correlation whose shape
/// between two lags means what it looks like it means.
const SMEAR: [f32; 5] = [1.0 / 9.0, 2.0 / 9.0, 3.0 / 9.0, 2.0 / 9.0, 1.0 / 9.0];

/// What a candidate lag's own multiples are worth to it. A real beat period
/// repeats: the correlation peaks at the period, then again at twice and
/// three times it. Half tempo scores just as well on that test, which is
/// what [`HALF_PENALTY`] is for.
const HARMONIC_2: f32 = 0.5;
const HARMONIC_3: f32 = 0.25;
/// What a peak halfway through the candidate period costs it. If the
/// novelty repeats at half the lag as well, the candidate is the half-time
/// reading of a faster beat. Set gently: a track with real offbeat energy,
/// hats between the kicks, would be pushed to double time by a hard one.
const HALF_PENALTY: f32 = 0.4;
/// What a candidate scores when it and both its multiples correlate
/// perfectly, which is what a confidence is a fraction of.
const FULL_SCORE: f32 = 1.0 + HARMONIC_2 + HARMONIC_3;

/// The prior over tempo, as a Gaussian on log2 BPM: where it's centred, and
/// how many octaves wide. Ninety percent of an octave is deliberately
/// loose, since its only job is picking which halving of a measured grid to
/// report. 70 to 180 BPM all sit above three quarters weight, so inside
/// that range it barely leans at all.
const PRIOR_CENTRE: f32 = 120.0;
const PRIOR_OCTAVES: f32 = 0.9;

/// How close two windows have to be, once folded to a common octave, to be
/// treated as the same answer. Four percent is 5 BPM at 128, wider than the
/// grid this transform resolves and narrower than any tempo change a
/// listener would call the same tempo.
const AGREE: f32 = 0.04;
/// How much better than the average lag the winner has to be before the
/// answer is worth storing.
///
/// Measured against the cases it exists to refuse: steady noise scores
/// about 0.06, a held tone about 0.09, and clicks at random intervals,
/// which have real onsets and no grid at all, about 0.15. The kit patterns
/// the tests synthesize score between 0.45 and 1.0, the busier and more
/// syncopated ones at the bottom of that. A quarter sits in the gap, and
/// real music is messier than any of this, so it's the loose side of the
/// gap rather than the middle.
const CONFIDENCE_FLOOR: f32 = 0.25;

/// The band an answer is allowed out in. Wider than the search, because the
/// search band is a decision about where to look and this is the contract
/// with whatever stores the number.
const OUT_MIN: f32 = 40.0;
const OUT_MAX: f32 = 300.0;

/// One window's answer and how much the window believes it.
#[derive(Clone, Copy, Debug)]
struct Vote {
    bpm: f32,
    confidence: f32,
}

/// One track's tempo, or None if the track didn't give a straight answer.
///
/// Two windows of thirty seconds are decoded at 44.1 kHz and downmixed the
/// same way the sketch does it, so this costs a minute of decoding per
/// track on top of whatever else the pass reads. A track whose windows
/// split pays for the widening a minute-long pair at a time, most often
/// one before a majority lands and [`WIDEN`]'s whole list only for a
/// track that never finds one. Nothing is cached between the two passes
/// today: they run at different times over different windows.
///
/// `duration_ms` is the track's length as the library knows it, used only
/// to place the probes. A cue subsong is measured from the top of the image
/// it lives in, exactly as [`crate::extract`] probes it, so a subsong's
/// tempo is really the image's tempo at the same offsets. The acoustic
/// pass has always described cue tracks that way, and fixing it is one
/// change to [`rox_playback::engine::decode_window`]'s span argument, for
/// both callers at once.
pub fn estimate(path: &Path, duration_ms: u32) -> Option<f32> {
    let duration = duration_ms as f64 / 1000.0;
    let path = path.to_path_buf();
    let single = duration <= SINGLE_SECS;
    let span = (duration - WINDOW_SECS).max(0.0);

    let mut votes = Vec::with_capacity(PROBES.len() + WIDEN.len() * 2);
    for probe in PROBES {
        let at = if single { 0.0 } else { span * probe };
        if let Some(vote) = probe_window(&path, at) {
            votes.push(vote);
        }
        if single {
            break;
        }
    }
    let mut answer = combine(&votes);
    // Windows sure of different tempos are windows short of a verdict: a
    // fill or a bridge under one probe reads differently from the track,
    // and refusing over it files a steady track as unreadable. The search
    // widens a pair at a time until a majority settles it or [`WIDEN`]
    // runs out, and it stops the moment fewer than two votes are worth
    // arguing over: windows that couldn't hear a tempo aren't a
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
                if let Some(vote) = probe_window(&path, span * probe) {
                    votes.push(vote);
                }
            }
            answer = combine(&votes);
        }
    }
    if widened && answer.is_none() {
        log::debug!("tempo: {}: windows disagree, {:?}", path.display(), votes);
    }
    answer
}

/// One probe's vote: a window decoded off the track at `at` seconds,
/// downmixed, and measured. None for a window that won't decode or won't
/// answer; the decode failure goes to the log, since a track this pass
/// couldn't read isn't a track without a tempo.
fn probe_window(path: &Path, at: f64) -> Option<Vote> {
    let frames = (WINDOW_SECS * RATE as f64) as usize;
    match rox_playback::engine::decode_window(&path.to_path_buf(), at, RATE, frames) {
        Ok(stereo) => {
            let mono: Vec<f32> = stereo
                .chunks_exact(2)
                .map(|c| (c[0] + c[1]) * 0.5)
                .collect();
            window(&mono)
        }
        Err(e) => {
            log::debug!("tempo: {}: {e}", path.display());
            None
        }
    }
}

/// One decoded window's vote.
fn window(mono: &[f32]) -> Option<Vote> {
    let (curve, drums) = novelty_split(mono);
    vote(&curve, &drums)
}

/// One novelty curve's vote: the lag it repeats at, folded into a tempo.
/// The drum curve rides along for the octave decisions; the search itself
/// runs on the full band, which is the one that always has something in it.
fn vote(curve: &[f32], drums: &[f32]) -> Option<Vote> {
    if curve.len() < MIN_FRAMES {
        return None;
    }
    let peaks = sharpen(curve);
    // A curve that never moves has nothing to correlate. Digital silence
    // gets here, and so does a window of one held tone.
    let r = correlate(&peaks, LAGS.min(peaks.len() - 1))?;
    // A window can have drums in it or not; a track that's all strings and
    // voice has a low band with no beat in it. Empty stands for that, and
    // reads zero at every lag, so everything the drum curve informs falls
    // back to the full band on its own.
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
    // No lag at all correlated positively, so there is no period to divide
    // down. A window of NaN samples arrives here too, since nothing
    // compares true against it.
    if !top.is_finite() || top <= 0.0 {
        return None;
    }
    // Parabolic interpolation through the winner and its neighbours, for
    // the part of the answer that falls between two steps of the walk.
    // Dividing that down to the grid rather than measuring the grid
    // directly is the more precise way around: a lag measured at four beats
    // carries a quarter of the error per beat.
    let lag = LAG_MIN + (best as f32 + refine(&scores, best)) * STEP;

    let bpm = fold_into_band(&r, &low, stretch(&r, grid(&r, &low, lag)))?;
    // How much the winner explains, over what a lag in this band explains
    // on average. A curve with no period in it has a best lag too; it just
    // isn't any better than the lags either side of it.
    let mut band = scores;
    band.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let confidence = ((top - band[band.len() / 2]) / FULL_SCORE).clamp(0.0, 1.0);
    Some(Vote { bpm, confidence })
}

/// The grid under a repeat: the longest division of `lag` either curve
/// still repeats at nearly as strongly, or `lag` itself if nothing repeats
/// at anything shorter.
///
/// This is what keeps a house track off three quarters of its tempo. The
/// lag three beats long is a real repeat and an unusually clean one, since
/// nothing sits halfway through it to give it away, but a third of it is a
/// repeat too, and that third is the beat.
///
/// Each curve is measured against its own strength at the winner rather
/// than against the other's. The drum curve earns its say at fast tempos:
/// a backbeat two 200 BPM beats apart correlates so much better than one
/// beat that the division fails the full-band test, while the kicks under
/// it repeat at every beat and pass their own.
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

/// `lag` measured again against the furthest multiple of itself the
/// correlation reaches, which is where the same error is spread over the
/// most beats.
///
/// A grid read at one beat is only as good as the 23 ms frame it was
/// sampled on. The same grid read at eight beats divides that error by
/// eight, and the correlation out there is a real peak as long as the track
/// holds its tempo across the three seconds the lags cover, which is the
/// case this estimator is for in the first place. The search stays inside
/// one beat of where it expects the multiple, so it can't wander onto a
/// neighbouring one.
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

/// The grid at `lag` read as a tempo inside the band: itself if it's
/// already there, otherwise halved until it is.
///
/// Only halving, never doubling. The lag that won is the shortest period
/// the curve repeats at, so there's nothing underneath it to find; a tempo
/// faster than it would be a beat with nothing on it.
///
/// Which halving wins is decided by three things together: how likely the
/// tempo is at all, what each halving costs, and how strongly the track
/// repeats at that candidate on either curve. The last one is where the
/// drums come in. A kick-and-snare alternation at 200 correlates weakly at
/// one beat in the full band, exactly like a kick-and-hat alternation an
/// octave too fast does, and the low band is what tells them apart: the
/// drums repeat at every beat of the first and only at every other event
/// of the second.
fn fold_into_band(r: &[f32], drums: &[f32], lag: f32) -> Option<f32> {
    let mut best: Option<(f32, f32)> = None;
    for octave in 0..=OCTAVES {
        let lag = lag * (1 << octave) as f32;
        let bpm = FPS * 60.0 / lag;
        if bpm < MIN_BPM {
            break;
        }
        // What this candidate is worth as evidence: the better of the two
        // curves' correlations, less any active refutation from the drums.
        // A negative drum correlation isn't a gap in the evidence, it's the
        // drums landing between this lag's beats, which is what the low
        // band looks like at the strum grid of a track whose drums are on
        // the backbeat.
        let drum = at(drums, lag);
        let heard = at(r, lag).max(drum) + drum.min(0.0);
        // A halving the curves don't correlate at is arithmetic, not a
        // tempo: a track at 60 stays at 60 rather than reading 120 off a
        // beat that isn't there.
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

/// The novelty curve as peaks: each frame over its own neighbourhood's
/// mean, rectified, then centred on zero.
///
/// Both halves earn their place. Without the local mean a loud passage
/// correlates with itself and the answer becomes "the track is 30 seconds
/// long"; without centring afterwards, a curve that's positive everywhere
/// correlates well at every lag and the peaks stop standing out.
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

/// Autocorrelation at every lag up to `max_lag`, each divided by how many
/// products went into it so a long lag isn't penalized for reaching past
/// the end of the window, and the whole thing divided by lag zero so a
/// value is a correlation rather than a level. None for a curve with no
/// energy at all, which is the one case that division can't survive.
///
/// Cost is `max_lag` passes over the curve, which at 30 seconds and 131
/// lags is under two hundred thousand multiplies, nothing next to the
/// transforms that produced the curve.
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

/// The correlation at a fractional lag, straight-line between the two
/// integer lags around it, and zero past the end.
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

/// What one candidate lag is worth: its own correlation, plus a share of
/// its multiples, less what a peak halfway through it says about it.
fn score(r: &[f32], lag: f32) -> f32 {
    let own = at(r, lag);
    // Support reinforces a period, it doesn't stand in for one. A lag the
    // curve doesn't correlate at isn't a grid however well its multiples
    // land, and without saying so the lag one and a half beats long scores
    // on its double, which is a real peak three beats out.
    if own <= 0.0 {
        return own;
    }
    let support = own + HARMONIC_2 * at(r, lag * 2.0) + HARMONIC_3 * at(r, lag * 3.0);
    support - HALF_PENALTY * at(r, lag / 2.0).max(0.0)
}

/// How likely a tempo is before anything has been heard.
fn prior(bpm: f32) -> f32 {
    let octaves = (bpm / PRIOR_CENTRE).log2() / PRIOR_OCTAVES;
    (-0.5 * octaves * octaves).exp()
}

/// Where the peak really sits, as an offset in frames from the sampled
/// winner. Clamped to half a frame either way, since a parabola through
/// three points of a noisy curve can otherwise claim the peak is at a
/// neighbour it scored below.
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

/// `bpm` moved into `toward`'s octave: doubled or halved as many times as
/// it takes to get the ratio as close to one as doubling can.
fn fold(bpm: f32, toward: f32) -> f32 {
    if bpm <= 0.0 || toward <= 0.0 {
        return bpm;
    }
    bpm * (toward / bpm).log2().round().exp2()
}

/// The windows' votes as one answer.
///
/// Every vote anchors a candidate reading: itself plus every other vote
/// that lands within [`AGREE`] of it once folded to its octave. The
/// reading carrying the most confidence wins, and comes out as the
/// confidence-weighted mean of its members, so two windows either side of
/// 128 answer between them and a window that heard 87 joins one that heard
/// 174 where it belongs.
///
/// Refusal is a count of confident votes, not a veto. A vote under the
/// confidence floor is a window that couldn't really hear a tempo and
/// counts for nothing either way; confident votes outside the winning
/// reading count against it, and the answer stands only while the reading
/// outnumbers them. One against one refuses, which is what sends
/// [`estimate`] for its third window; two against one is a majority and a
/// track's tempo; and windows that all heard something different stay
/// refused, since a number that describes one window of a track that moves
/// isn't the track's tempo. That last case is what keeps a symphony's
/// windows from filing whichever pseudo-beat scored highest.
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
    // No confident majority is no answer, and a reading of nothing but
    // sub-floor votes has no majority to have: the old anchor-confidence
    // floor falls out of the same count.
    if inside <= outside || !(OUT_MIN..=OUT_MAX).contains(&bpm) {
        return None;
    }
    Some(bpm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// One hit: a tone that decays over `len` samples, added in so hits can
    /// overlap the way a kit's do.
    fn hit(buf: &mut [f32], at: usize, hz: f32, gain: f32, len: usize) {
        for i in 0..len {
            let Some(slot) = buf.get_mut(at + i) else {
                break;
            };
            let decay = 1.0 - i as f32 / len as f32;
            *slot += decay * decay * gain * (TAU * hz * i as f32 / RATE as f32).sin();
        }
    }

    /// A click every beat and nothing else. The period is fractional in
    /// frames at every one of these tempos, which is the whole difficulty.
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

    /// A kick on the beat and, if `offbeat` is more than nothing, a hat
    /// halfway between: a low band-limited thump and a bright short one,
    /// which is the shape of most of the music this is for.
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

    /// A whole kit: kick on every beat, a snare and a bright hit on two and
    /// four, a hat on every offbeat, and a bass note under each beat. The
    /// backbeat is what makes this different from [`kit`] - a bar
    /// correlates with itself better than a beat does with the next beat,
    /// so the strongest repeat in this signal is two beats long and the
    /// tempo is only found by dividing it.
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

    /// Half time: kick on one, snare on three, hats through the eighths.
    /// The pattern a lot of hip hop is, and the octave trap that comes with
    /// it.
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

    /// Clicks that speed up from `from` to `to` across the clip.
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

    /// Clicks at intervals between 0.1 and 0.8 seconds: real onsets, no
    /// grid. Deterministic, off the same shape of generator the rest of the
    /// crate's tests synthesize noise with.
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

    /// Deterministic white-ish noise, the same generator [`crate`]'s own
    /// tests use.
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

    /// What one window of audio would be stored as: the window's vote put
    /// through the same acceptance [`estimate`] ends with.
    fn answer(mono: &[f32]) -> Option<f32> {
        combine(&[window(mono)?])
    }

    /// How far off, as a share of the tempo asked for.
    fn error(got: f32, want: f32) -> f32 {
        (got - want).abs() / want
    }

    /// Rock: kick on one and three, snare on two and four, and strummed
    /// eighths over the top at `strum`'s gain. The snare gets a low body
    /// beside its crack, since a snare is a drum and lands in the low band.
    /// The eighth grid is real, but the tempo is the kick and snare's.
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

    /// The straight case, tempos across the whole band, 200 included: a
    /// track written at the top of the band has to read back at itself
    /// rather than at the half the prior finds more plausible. None of
    /// these periods is a whole number of frames (174 BPM is 14.85 of
    /// them), so passing is a claim about the fractional lag walk and the
    /// multiple it's measured against, not just about finding a peak.
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

    /// A kick with a hat between every pair of kicks, which is what a house
    /// or techno track is. The hat is the trap: it's brighter than the
    /// kick, so it makes more flux than the kick does, and an estimator
    /// that goes looking for a plausible tempo directly finds three
    /// quarters of this one.
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

    /// A backbeat is the case the divisions exist for: the loudest repeat
    /// in this signal is the two-beat bar, and the tempo is a division of
    /// it that only correlates about half as well. The fast end is the
    /// happy hardcore case twice over: past about 185 the beat's own
    /// correlation gets too weak for the full-band division and the kicks
    /// have to carry it, and the fold has to keep the answer's octave
    /// where the prior alone would halve it.
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

    /// An offbeat as loud as the beat, resolved by what the offbeat is
    /// made of. In the full band a hat as loud as the kick makes the
    /// eighth grid as strong as the beat and nothing says which is which;
    /// in the low band the kicks repeat at 85 and the hats aren't there at
    /// all, so the answer is the kicks'.
    #[test]
    fn hats_as_loud_as_the_kicks_still_read_the_kicks_tempo() {
        let got = answer(&kit(85.0, 20.0, 0.9)).expect("a kit pattern has a tempo");
        assert!(
            error(got, 85.0) < 0.02,
            "85 with hats as loud as the kicks read as {got:.2}"
        );
    }

    /// The Creedence case: a rock backbeat with strummed eighths riding
    /// over it. The strums put a real grid at double the tempo, and at the
    /// loud end that grid outright wins the full-band comb; the drums
    /// repeating at the beat and landing between the strum grid's units is
    /// what folds the answer back to the kit's tempo.
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

    /// The octave the estimator still cannot read, stated as what it does
    /// about it. Half time puts the kick four beats from the next kick,
    /// which at 85 is past the longest lag the search reads, so the drums
    /// have no repeat inside the band to vouch with and the eighth grid
    /// the hats ride carries the answer: 85 comes back as 170. What's
    /// asserted is that it's an octave and not something in between, since
    /// 170 is a reading a listener would recognize and 113 is a wrong
    /// answer.
    #[test]
    fn a_halftime_pattern_is_read_an_octave_out() {
        let got = answer(&halftime(85.0, 20.0)).expect("a kit pattern has a tempo");
        assert!(
            error(fold(got, 85.0), 85.0) < 0.02,
            "half time at 85 read as {got:.2}, which is not an octave of it"
        );
        assert!(got > 150.0, "and today it is the double, {got:.2}");
    }

    /// Nothing to measure, or nothing periodic to measure, is refused
    /// rather than guessed at. The scattered clicks are the interesting one:
    /// they have as many onsets as a beat does, they just aren't a grid.
    #[test]
    fn silence_and_noise_and_scattered_hits_are_refused() {
        assert_eq!(answer(&vec![0.0; 20 * RATE as usize]), None, "silence");
        assert_eq!(answer(&noise(20.0)), None, "steady noise");
        assert_eq!(answer(&tone(440.0, 20.0)), None, "a held tone");
        assert_eq!(answer(&scatter(20.0)), None, "clicks at random intervals");
    }

    /// A window shorter than [`MIN_SECS`] is refused too. Two seconds of a
    /// 120 BPM click is four beats, which is enough to see a lag and not
    /// enough to believe it.
    #[test]
    fn too_short_a_window_is_refused() {
        assert!(window(&clicks(120.0, 2.0)).is_none());
    }

    /// A tempo that ramps has no answer, and this documents which one it
    /// gives anyway: a number from inside the ramp, near the middle of it,
    /// with the confidence still high because the correlation is genuinely
    /// strong at every lag the track passed through. The refusal for music
    /// that moves is [`combine`]'s, over two windows that disagree, and not
    /// this.
    #[test]
    fn a_tempo_that_ramps_answers_from_inside_the_ramp() {
        let got = answer(&ramp(120.0, 132.0, 20.0)).expect("a ramp still correlates");
        assert!(
            (120.0..=132.0).contains(&got),
            "a 120 to 132 ramp read as {got:.2}"
        );
    }

    /// The whole path on a real file: two windows decoded out of a track
    /// long enough for the probes to land in different places, downmixed,
    /// measured and combined. Everything else here starts from samples
    /// already in memory, so this is the only test that says the probe
    /// arithmetic and the decode agree with what [`window`] expects.
    #[test]
    fn a_file_on_disk_reads_back_at_its_tempo() {
        let dir = std::env::temp_dir().join(format!("rox-tempo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        let secs = 70.0;
        std::fs::write(&path, wav(&band(128.0, secs))).unwrap();

        let got = estimate(&path, (secs * 1000.0) as u32);
        let _ = std::fs::remove_dir_all(&dir);
        let got = got.expect("a click track on disk has a tempo");
        assert!(error(got, 128.0) < 0.01, "128 on disk read as {got:.2}");
    }

    /// A bridge under one probe, outvoted by the widened search. 160
    /// seconds of 128 with thirty seconds of 90 laid over the second
    /// probe's window: the first two windows split one against one, the
    /// first widening pair at a sixth and five sixths both land back on
    /// the 128, and the majority stores the track's real tempo instead of
    /// refusing over the bridge, without the second pair ever decoding.
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
        let got = got.expect("a majority should settle the split");
        assert!(
            error(got, 128.0) < 0.02,
            "the track runs at 128 around its bridge, read {got:.2}"
        );
    }

    /// A track that genuinely changes tempo splits the widened search too,
    /// and stays refused. 160 seconds with the seam at 75: the two probes
    /// split one against one, both widening pairs land one window on each
    /// side of the seam, and the search runs to its limit without a
    /// majority ever forming.
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
        assert_eq!(got, None, "an even split is a track with two tempos");
    }

    /// Mono 16-bit PCM at [`RATE`], which is the least a decoder needs to
    /// be handed a file.
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

    /// What the windows do with each other: agree and the answer is their
    /// weighted mean, split one against one and there is no answer, which
    /// is what sends [`estimate`] for a third whose majority decides. The
    /// octave case is the point of the fold: a window that heard 87 and a
    /// window that heard 174 heard the same track.
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

    /// The answer is always a tempo, whatever the windows said. Two votes
    /// an octave apart that both survive the fold still can't average into
    /// something outside the band.
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
