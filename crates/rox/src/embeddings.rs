//! The acoustic analysis pass: give every track a vector describing how it
//! sounds, so the library can answer "more like this" without asking anyone
//! on the internet.
//!
//! ## Two extractors
//!
//! [`MODEL`], the built-in one, is a timbre and rhythm sketch rather than a
//! neural model: log-band energy statistics, spectral centre of mass,
//! rolloff, flux, and an onset rate, all off DSP already in the tree
//! ([`rox_viz::analysis`] does the transform,
//! [`rox_playback::engine::decode_window`] the decoding). It's deliberately
//! modest, and it earns its place by needing no download, no native
//! dependency, and no network.
//!
//! [`panns`] is the model-based one that header used to promise. It runs
//! PANNs CNN10 through candle over weights the user downloads
//! ([`models`]), and it stores its 512 values under its own name, so both
//! models' vectors sit in the library at once and switching between them
//! costs nothing already analyzed. Which one the pass runs, and which one
//! the similarity queries read, is [`crate::settings::acoustic_model`].
//!
//! Adding a third means a [`models::CATALOG`] entry and an [`Extractor`]
//! arm. Nothing about the storage underneath ([`rox_library::embeddings`])
//! changes at all.
//!
//! The pass itself is the ReplayGain measurement's shape (`replaygain_job`):
//! app-global rather than owned by a window, blocking work on the background
//! executor, an `Arc<Progress>` the UI samples on a timer. It differs in
//! working through a bounded pool rather than one file at a time, because
//! every track here is independent and there are thirty seconds of decoding
//! in each.
//!
//! Gated behind the Development page's acoustic switch while the feature
//! vector is still being tuned.

// The mel module's config vocabulary is deliberately complete rather than
// trimmed to what today's one model asks for: the enums are how the module
// states which conventions exist and which this recipe picked, and the
// filterbank and the log branch on them. Trimming to the used arms would
// turn "Slaney rather than HTK" from a decision the code makes into a
// comment, which is exactly the class of mistake that module exists to stop.
#[allow(dead_code)]
pub mod mel;
pub mod models;
pub mod panns;
pub mod resample;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::embeddings::{self, Pending};
use rox_library::store;
use rox_viz::analysis::{self, Analyzer};

use crate::catalog::Library;
use crate::embeddings::models::Model;
use crate::settings::Settings;

/// What produced the vectors, and the name they're stored under. Change the
/// features below and this changes with them: the old vectors stay readable
/// under the old name, and nothing compares across the two.
pub const MODEL: &str = "dsp-timbre-1";

/// Log-spaced bands the spectrum is reduced to. Each contributes a mean and
/// a spread, which is most of the vector.
const BANDS: usize = 28;

/// The width of one track's vector: the bands twice over, then centroid,
/// rolloff, flux and energy statistics with the onset rate.
pub const DIM: usize = BANDS * 2 + 8;

/// Everything is decoded to this rate so two files at different rates
/// produce comparable numbers.
const RATE: u32 = 44_100;
/// How much audio each probe reads.
const WINDOW_SECS: f64 = 10.0;
/// Where the probes sit across the part of the track a window can start in.
/// Three of them, because one window of a quiet intro describes the intro.
const PROBES: [f64; 3] = [0.25, 0.5, 0.75];

/// Short enough to follow a beat, long enough to resolve the low bands.
const FFT: usize = 2048;
const HOP: usize = FFT / 2;
const LO_HZ: f32 = 40.0;
const HI_HZ: f32 = 16_000.0;
/// Where the rolloff sits: the frequency under which this share of the
/// frame's energy falls.
const ROLLOFF: f32 = 0.85;

// The onset trigger, the shape rox-viz's onset signal uses: a reference that
// chases the flux, a jump counted when the flux clears it by a margin, and a
// re-arm once things settle back down.
const ONSET_RATIO: f32 = 1.6;
const ONSET_REARM: f32 = 1.15;
const ONSET_FLOOR: f32 = 1e-5;
const ONSET_ATTACK: f32 = 0.4;
const ONSET_RELEASE: f32 = 0.1;

/// Tracks per transaction. Big enough that committing isn't the cost, small
/// enough that a cancel or a crash doesn't throw much decoding away.
const BATCH: usize = 32;
/// Ceiling on decode threads. Past a handful this is waiting on the disk
/// rather than the CPU, and the machine is meant to stay usable while a
/// library analyzes in the background.
const MAX_WORKERS: usize = 4;

/// Whichever extractor a pass is running.
///
/// The built-in arm carries nothing, because [`extract`] is a free function
/// with no state to hold; the model arm carries the loaded network, which is
/// 24 MB of weights and gets built once per pass rather than once per track.
pub enum Extractor {
    Dsp,
    // Boxed because the loaded network is a few hundred bytes of tensor
    // handles against the DSP arm's nothing, and every `Extractor` moved
    // around would otherwise carry the larger footprint.
    Panns(Box<panns::Cnn10>),
}

impl Extractor {
    /// Build the extractor for a model, or say why it can't run. This is
    /// where a missing or corrupt weights file is caught, so a pass either
    /// starts with a working model or doesn't start at all.
    pub fn load(model: &Model) -> Result<Self, String> {
        match model.id {
            MODEL => Ok(Extractor::Dsp),
            models::PANNS_CNN10 => {
                let net = panns::Cnn10::load(model)?;
                log::info!("acoustic: {} loaded, running on {}", model.id, net.device());
                Ok(Extractor::Panns(Box::new(net)))
            }
            other => Err(format!("no extractor is built for {other}")),
        }
    }

    /// One track's vector.
    fn describe(&self, path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
        match self {
            Extractor::Dsp => extract(path, duration_ms),
            Extractor::Panns(net) => net.extract(path, duration_ms),
        }
    }

    /// How many tracks this extractor wants analyzed at once.
    ///
    /// The DSP one is a decode and an FFT, so it scales across cores the way
    /// the pool was built for. The network is the opposite: candle's matmuls
    /// already spread across every core inside one forward pass, and running
    /// four of those against each other just makes them queue for the same
    /// cores while holding four tracks of spectrogram in memory.
    fn workers(&self) -> usize {
        match self {
            Extractor::Dsp => MAX_WORKERS,
            Extractor::Panns(_) => 1,
        }
    }
}

/// Live progress of a pass: the workers write it per file, the UI polls it.
/// Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    /// Which model this pass is running, so a readout can name it even after
    /// the selection has moved on.
    model: Mutex<String>,
    done: AtomicUsize,
    total: AtomicUsize,
    /// Files that would not decode into a vector, so the readout can own up
    /// to a pass that skipped some.
    failed: AtomicUsize,
    /// Full path of a file being analyzed. Whichever worker wrote last, so
    /// it reads as a sample of the work rather than a queue position.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the workers drop out at the next
    /// file.
    cancel: AtomicBool,
}

impl Progress {
    /// The model this pass is describing tracks with.
    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    /// Files analyzed or given up on so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Files the pass set out to analyze. Zero while the work list is still
    /// being built.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Files that would not decode.
    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    /// One of the files under the cursor.
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    /// Whether a stop has been asked for and the pass is winding down.
    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// The running pass, or nothing. App-global so it outlives the settings
/// window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last pass's failure, kept after the pass is gone so the settings
/// page can still explain why nothing happened. A model whose weights won't
/// load is the case this exists for: without it the button would flash and
/// the coverage line would be unchanged, with the reason only in the log.
#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

/// The running pass's progress, for a UI that wants to show it. None when
/// nothing is analyzing.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// Why the last pass stopped early, if it did.
pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Ask the running pass to stop at the next file. What it already wrote
/// stays; a no-op when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Analyze every track with no vector for the selected model. A no-op while
/// a pass is already running, and while the feature is switched off.
///
/// Which model runs is [`crate::settings::acoustic_model`], resolved here
/// rather than passed in: it's the same pick the similarity queries read, and
/// a caller that could hand in a different one would be able to fill the
/// table under a name nothing reads.
pub fn start(library: Entity<Library>, cx: &mut App) {
    if progress(cx).is_some() || !Settings::load().acoustic_analysis {
        return;
    }
    let model = crate::settings::acoustic_model();
    let db_path = library.read(cx).db_path();
    let progress = Arc::new(Progress::default());
    *progress.model.lock().unwrap() = model.id.to_string();
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Quitting mid-pass raises the same flag the stop button does, so the
    // workers land on a batch boundary instead of being killed mid-write.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(model, &db_path, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            match result {
                Ok(written) => log::info!("acoustic: {written} tracks analyzed with {}", model.id),
                Err(e) => {
                    log::error!("acoustic: {e}");
                    cx.set_global(LastFailure(Some(e)));
                }
            }
        })
        .ok();
    })
    .detach();
}

/// The blocking half: walk what's missing, analyze in batches, write each
/// batch in one transaction. Resumes by construction, since the work list is
/// whatever has no vector yet.
fn run(model: &'static Model, db_path: &Path, progress: &Progress) -> Result<usize, String> {
    // Before the work list, so a model whose weights went missing between
    // the settings page reading them and the pass starting says so instead
    // of counting a library's worth of work it can't do.
    let extractor = Extractor::load(model)?;
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let pending = embeddings::missing(&conn, model.id).map_err(|e| e.to_string())?;
    progress.total.store(pending.len(), Ordering::Relaxed);

    let mut written = 0;
    for batch in pending.chunks(BATCH) {
        if !progress.keep_going() {
            break;
        }
        let vectors = analyze_batch(&extractor, batch, progress);
        written += vectors.len();
        embeddings::upsert_many(&mut conn, model.id, &vectors).map_err(|e| e.to_string())?;
    }
    Ok(written)
}

/// One batch through a bounded pool. Every track is independent, so the
/// workers just race a cursor down the slice; the order results come back in
/// doesn't matter, they're keyed by id.
fn analyze_batch(
    extractor: &Extractor,
    batch: &[Pending],
    progress: &Progress,
) -> Vec<(i64, Vec<f32>)> {
    let cursor = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(batch.len()));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
        .min(extractor.workers())
        .min(batch.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if !progress.keep_going() {
                    break;
                }
                let Some(item) = batch.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                *progress.current.lock().unwrap() = item.path.clone();
                match extractor.describe(Path::new(&item.path), item.duration_ms) {
                    Ok(vector) => out.lock().unwrap().push((item.id, vector)),
                    Err(e) => {
                        log::warn!("acoustic: {}: {e}", item.path);
                        progress.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    out.into_inner().unwrap()
}

/// One track's vector: three windows decoded, described, and averaged.
///
/// Averaging rather than concatenating is on purpose. A vector per window
/// would make the same song at two tempos look like two songs depending on
/// where the probes landed; the mean of three describes the track.
pub fn extract(path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
    let duration = duration_ms as f64 / 1000.0;
    let frames = (WINDOW_SECS * RATE as f64) as usize;
    let path = path.to_path_buf();
    // A track no longer than one window has one window in it, read from the
    // top. Anything longer spreads the probes across the range a window can
    // still start in, so none of them runs off the end.
    let single = duration <= WINDOW_SECS;
    let span = (duration - WINDOW_SECS).max(0.0);

    let mut sum = vec![0f64; DIM];
    let mut taken = 0usize;
    let mut last_err = String::new();
    for probe in PROBES {
        let stereo = match rox_playback::engine::decode_window(&path, span * probe, RATE, frames) {
            Ok(stereo) => stereo,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let mono: Vec<f32> = stereo
            .chunks_exact(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();
        match features(&mono) {
            Some(vector) => {
                for (acc, v) in sum.iter_mut().zip(&vector) {
                    *acc += *v as f64;
                }
                taken += 1;
            }
            None => last_err = "window too short to analyze".into(),
        }
        if single {
            break;
        }
    }
    if taken == 0 {
        return Err(if last_err.is_empty() {
            "nothing decodable".into()
        } else {
            last_err
        });
    }
    Ok(sum.iter().map(|v| (v / taken as f64) as f32).collect())
}

/// Describe one mono window: what it sounds like as [`DIM`] numbers.
///
/// Everything here is a raw statistic, on whatever scale it naturally lands.
/// Putting the dimensions on equal footing is the query's job, and doing it
/// there rather than here means the weighting can be retuned without
/// re-analyzing a library (see [`rox_library::embeddings::nearest`]).
fn features(mono: &[f32]) -> Option<Vec<f32>> {
    // Four hops is the floor for a spread to mean anything.
    if mono.len() < FFT + HOP * 3 {
        return None;
    }
    let mut analyzer = Analyzer::new(FFT);
    let half = FFT / 2;
    let bands = analysis::log_bands(BANDS, LO_HZ, HI_HZ, RATE, half);
    let bin_hz = RATE as f32 / FFT as f32;

    let mut per_band: Vec<Vec<f32>> = (0..BANDS).map(|_| Vec::new()).collect();
    let mut centroid = Vec::new();
    let mut rolloff = Vec::new();
    let mut flux = Vec::new();
    let mut energy = Vec::new();
    // None until the second frame: the first has nothing behind it, and
    // measuring it against silence would score the window's own opening edge
    // as the loudest onset in the track.
    let mut previous: Option<Vec<f32>> = None;

    let mut start = 0;
    while start + FFT <= mono.len() {
        let frame = &mono[start..start + FFT];
        start += HOP;
        energy.push((frame.iter().map(|s| s * s).sum::<f32>() / FFT as f32).sqrt());
        let mags = analyzer.magnitudes(frame).to_vec();

        for (values, &(lo, hi)) in per_band.iter_mut().zip(&bands) {
            let sum: f32 = mags[lo..hi].iter().sum();
            // Log energy: loudness is perceived that way, and it keeps a
            // quiet band from reading as a rounding error next to a loud one.
            values.push((sum / (hi - lo) as f32 + 1e-9).ln());
        }

        let total: f32 = mags.iter().sum();
        if total > 1e-9 {
            let weighted: f32 = mags
                .iter()
                .enumerate()
                .map(|(k, m)| k as f32 * bin_hz * m)
                .sum();
            centroid.push((weighted / total + 1.0).ln());
            let mut running = 0.0;
            let mut edge = half - 1;
            for (k, m) in mags.iter().enumerate() {
                running += m;
                if running >= total * ROLLOFF {
                    edge = k;
                    break;
                }
            }
            rolloff.push((edge as f32 * bin_hz + 1.0).ln());
        }

        // Half-wave rectified: what appeared since the last frame, not what
        // faded out. A note starting is an onset, a note ending isn't.
        if let Some(previous) = &previous {
            let rise: f32 = mags
                .iter()
                .zip(previous)
                .map(|(m, p)| (m - p).max(0.0))
                .sum();
            flux.push(rise / half as f32);
        }
        previous = Some(mags);
    }
    if flux.len() < 2 {
        return None;
    }

    let secs = mono.len() as f32 / RATE as f32;
    let mut out = Vec::with_capacity(DIM);
    // Means first, then spreads, so a slice of the vector is one statistic
    // across the spectrum rather than an interleaving.
    let band_stats: Vec<(f32, f32)> = per_band.iter().map(|v| mean_std(v)).collect();
    out.extend(band_stats.iter().map(|(mean, _)| *mean));
    out.extend(band_stats.iter().map(|(_, std)| *std));
    for values in [&centroid, &rolloff, &flux] {
        let (mean, std) = mean_std(values);
        out.push(mean);
        out.push(std);
    }
    out.push(onset_rate(&flux, secs));
    // How much the level moves across the window, the one dynamics number:
    // a compressed master sits still, a live take doesn't.
    out.push(mean_std(&energy).1);
    debug_assert_eq!(out.len(), DIM);
    Some(out)
}

/// Mean and population standard deviation, or zeros for nothing.
fn mean_std(values: &[f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Spectral jumps per second: the rhythm half of the sketch, and the one
/// number that separates a beat from a drone at the same brightness.
fn onset_rate(flux: &[f32], secs: f32) -> f32 {
    if flux.is_empty() || secs <= 0.0 {
        return 0.0;
    }
    let mut reference = flux[0];
    let mut armed = true;
    let mut count = 0usize;
    for &f in flux {
        if armed && f > ONSET_FLOOR && f > reference * ONSET_RATIO {
            count += 1;
            armed = false;
        } else if !armed && f < reference * ONSET_REARM {
            armed = true;
        }
        let ease = if f > reference {
            ONSET_ATTACK
        } else {
            ONSET_RELEASE
        };
        reference += (f - reference) * ease;
    }
    count as f32 / secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin() * 0.5)
            .collect()
    }

    /// Deterministic white-ish noise, so the test means the same thing on
    /// every machine and every run.
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

    fn distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    }

    /// The same audio in gives the same vector out, every time. Without this
    /// a re-analysis would quietly reshuffle a library's neighbours.
    #[test]
    fn the_same_window_always_describes_the_same() {
        let audio = tone(440.0, 2.0);
        let first = features(&audio).expect("two seconds is enough to describe");
        let second = features(&audio).unwrap();
        assert_eq!(first.len(), DIM);
        assert_eq!(first, second);
    }

    /// A window too short to hold a few hops describes nothing, and says so
    /// rather than returning a vector built from one frame.
    #[test]
    fn too_short_a_window_is_refused() {
        assert!(features(&tone(440.0, 0.01)).is_none());
    }

    /// Two near neighbours land close together and something categorically
    /// different lands far away. The absolute numbers don't matter, the gap
    /// between them does.
    #[test]
    fn a_tone_and_noise_are_measurably_apart() {
        let a = features(&tone(440.0, 2.0)).unwrap();
        let b = features(&tone(466.0, 2.0)).unwrap();
        let n = features(&noise(2.0)).unwrap();
        let near = distance(&a, &b);
        let far = distance(&a, &n);
        assert!(
            far > near * 5.0,
            "a semitone apart is {near:.2}, noise is {far:.2}"
        );
    }

    /// A burst every `per_sec` over silence: transients and nothing else.
    /// Each one decays over a few milliseconds so it survives the window as
    /// a spectral jump rather than a single sample the FFT smears away.
    fn pulses(per_sec: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        let period = (RATE as f32 / per_sec) as usize;
        const BURST: usize = 220;
        (0..n)
            .map(|i| {
                let phase = i % period;
                if phase < BURST {
                    (1.0 - phase as f32 / BURST as f32) * (i as f32 * 0.7).sin() * 0.8
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// The onset rate is what tells a beat from a drone at the same
    /// brightness, so a pulse train has to read as busy and a held tone as
    /// still. Steady noise counts as still too, which is the point: it's
    /// loud and broadband, but nothing in it ever starts.
    #[test]
    fn onsets_separate_a_drone_from_a_beat() {
        let rate = DIM - 2;
        let beat = features(&pulses(8.0, 2.0)).unwrap()[rate];
        let held = features(&tone(440.0, 2.0)).unwrap()[rate];
        let hiss = features(&noise(2.0)).unwrap()[rate];
        assert!(
            beat > 4.0,
            "eight pulses a second should register, got {beat}"
        );
        assert_eq!(
            held, 0.0,
            "a held tone starts nothing after its first frame"
        );
        assert_eq!(hiss, 0.0, "steady noise is loud, not busy");
    }
}
