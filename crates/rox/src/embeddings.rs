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
//! A weights file the user points rox at is the same thing without a catalog
//! entry: [`Source`] carries either, and a local file takes its name from its
//! own hash so its vectors sit beside the catalog's rather than in them.
//!
//! The pass itself is the ReplayGain measurement's shape (`replaygain_job`):
//! app-global rather than owned by a window, blocking work on the background
//! executor, an `Arc<Progress>` the UI samples on a timer. It differs in
//! working through a bounded pool rather than one file at a time, because
//! every track here is independent and there are thirty seconds of decoding
//! in each.
//!
//! Gated behind the Library page's acoustic switch while the feature
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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
/// The default for [`crate::settings::Settings::acoustic_workers`]: enough
/// to make a dent, few enough that the machine stays usable while a library
/// analyzes in the background. The setting exists because that trade is the
/// user's to make: on a machine with cores to spare, more workers is the
/// difference between a pass measured in days and one measured in hours.
pub const DEFAULT_WORKERS: usize = 4;
/// Tracks a pass must get through before its rate is worth remembering as
/// this machine's pace. Half a batch: enough to wash out the cold start,
/// small enough that stopping a pass early still teaches the estimate
/// something.
const PACE_FLOOR: usize = 16;

/// Where a pass's weights come from: the catalog, or a file on disk the user
/// pointed rox at.
///
/// The two are one type rather than two code paths because everything
/// downstream only wants two things from a model, a name to store vectors
/// under and something to load, and a local file answers both. What it
/// doesn't have is a checksum in the catalog, which is why its name is
/// derived from the file's own hash: a different checkpoint produces vectors
/// in a different space, and letting one borrow another's name would mix two
/// sets of coordinates in one table and quietly wrong every similarity
/// answer after that.
#[derive(Clone)]
pub enum Source {
    /// A catalog entry ([`models::CATALOG`]): the built-in extractor, or a
    /// model rox ships a URL and a checksum for.
    Catalog(&'static Model),
    /// Weights the user picked off disk. Boxed in an `Arc` so the settings
    /// static, the settings page, and a running pass can all hold the same
    /// one without copying the path and the id around.
    Local(Arc<Local>),
}

/// A weights file outside the catalog: where it is, and the name its vectors
/// are stored under.
pub struct Local {
    pub path: PathBuf,
    /// Derived from the file's SHA-256 by [`local_id`], so two different
    /// checkpoints can never collide and the same one picked twice keeps the
    /// vectors it already wrote.
    pub id: String,
}

/// The name a local weights file's vectors are stored under: a fixed prefix
/// and the head of the file's SHA-256. Sixteen hex digits is 64 bits, which
/// is far past the point where two files a person owns collide, and short
/// enough to read in a log line.
pub fn local_id(sha256: &str) -> String {
    format!("local-{}", &sha256[..sha256.len().min(16)])
}

impl Source {
    /// The name this source's vectors are stored under.
    pub fn id(&self) -> &str {
        match self {
            Source::Catalog(model) => model.id,
            Source::Local(local) => &local.id,
        }
    }

    /// What to call it on screen.
    pub fn label(&self) -> String {
        match self {
            Source::Catalog(model) => model.label.to_string(),
            // The file's own name: the id is a hash, and a row that said
            // "local-9f2c..." would tell nobody which checkpoint they picked.
            Source::Local(local) => local
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Custom model".into()),
        }
    }

    /// Whether this is the built-in extractor rather than a network.
    pub fn is_builtin(&self) -> bool {
        matches!(self, Source::Catalog(model) if model.id == MODEL)
    }

    /// Whether the weights are there to load. Cheap enough for a settings
    /// render: a length check on the catalog side, an existence check on the
    /// local one. Whether the file is the right file is [`Extractor::load`]'s
    /// answer, not this one's.
    pub fn installed(&self) -> bool {
        match self {
            Source::Catalog(model) => model.installed(),
            Source::Local(local) => local.path.is_file(),
        }
    }
}

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
    /// Build the extractor for a source, or say why it can't run. This is
    /// where a missing, corrupt, or simply-not-this-network weights file is
    /// caught, so a pass either starts with a working model or doesn't start
    /// at all.
    pub fn load(source: &Source) -> Result<Self, String> {
        let net = match source {
            Source::Catalog(model) => match model.id {
                MODEL => return Ok(Extractor::Dsp),
                models::PANNS_CNN10 => panns::Cnn10::load(model)?,
                other => return Err(format!("no extractor is built for {other}")),
            },
            // No checksum to check it against, so the load itself is the
            // validation: `build` reads named tensors at fixed shapes, and
            // the mel filterbank the file carries is compared against the one
            // the front end computes.
            Source::Local(local) => panns::Cnn10::load_from(&local.path)?,
        };
        log::info!(
            "acoustic: {} loaded, running on {}",
            source.id(),
            net.device()
        );
        Ok(Extractor::Panns(Box::new(net)))
    }

    /// One track's vector, or why this file didn't produce one.
    fn describe(&self, path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
        let vector = match self {
            Extractor::Dsp => extract(path, duration_ms),
            Extractor::Panns(net) => net.extract(path, duration_ms),
        }?;
        if !usable(&vector) {
            return Err("the description came out with a NaN or an infinity in it".into());
        }
        Ok(vector)
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
    /// The pass's clock, for the "about 2 hours left" half of the readout.
    /// Started once the work list is built, so the model load and the
    /// database walk don't bill the first track.
    pace: crate::pace::Pace,
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

    /// Seconds each track has cost so far, measured over the whole pass.
    /// None until enough tracks have finished for the average to mean
    /// anything.
    pub fn secs_per_track(&self) -> Option<f64> {
        self.pace.secs_per_track(self.done())
    }

    /// Seconds the rest of the pass should take at the rate so far.
    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
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
/// Which model runs is [`crate::settings::acoustic_source`], resolved here
/// rather than passed in: it's the same pick the similarity queries read, and
/// a caller that could hand in a different one would be able to fill the
/// table under a name nothing reads.
pub fn start(library: Entity<Library>, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.acoustic_analysis {
        return;
    }
    // Read once here rather than inside the pass: a pass keeps the worker
    // count it started with, and the next one picks up a changed setting.
    let workers = settings.acoustic_workers.max(1);
    let source = crate::settings::acoustic_source();
    let db_path = library.read(cx).db_path();
    let progress = Arc::new(Progress::default());
    *progress.model.lock().unwrap() = source.id().to_string();
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Keeps the menubar chip and the tasks window ticking; nothing observes
    // an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
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
        let name = source.id().to_string();
        let result = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&source, &db_path, workers, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // What this machine can do, remembered for the next estimate.
            // Worker-seconds per track, so the Library page can price any
            // worker setting against it. Only off a decent stretch: a pass
            // over a handful of files measures its own startup, not the rate.
            if progress.done() >= PACE_FLOOR {
                if let Some(per) = progress.secs_per_track() {
                    let pace = (per * workers as f64) as f32;
                    let id = name.clone();
                    Settings::update(move |s| {
                        s.session.acoustic_pace.insert(id, pace);
                    });
                }
            }
            match result {
                Ok(written) => {
                    // The surfaces that offer ordering by sound are gated on
                    // there being vectors, and this is the moment there are.
                    if written > 0 {
                        crate::settings::set_acoustic_described(true, cx);
                    }
                    log::info!("acoustic: {written} tracks analyzed with {name}");
                }
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

/// Time a few tracks to learn what this machine costs per track, so a first
/// pass can be priced before anyone commits an afternoon to it. Returns
/// worker-seconds per track, the unit [`crate::pace::estimate`] divides.
///
/// Sequential on one thread on purpose: one track at a time is exactly one
/// worker-second per second, so the number needs no correcting for the pool
/// it was measured under.
///
/// The vectors it produces are kept. They're the same vectors the pass would
/// have written for those tracks, so throwing them away to keep the probe
/// tidy would mean decoding them twice for nothing.
///
/// Blocking, and slow enough to want a background thread: it decodes and
/// describes real files, which is the whole point.
pub fn measure_pace(source: &Source, db_path: &Path) -> Result<f32, String> {
    let extractor = Extractor::load(source)?;
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let pending = embeddings::missing(&conn, source.id()).map_err(|e| e.to_string())?;
    let picked = crate::pace::sample_indices(pending.len(), crate::pace::PROBE_TRACKS);
    if picked.is_empty() {
        return Err("there's nothing left to analyze".into());
    }

    let started = Instant::now();
    let mut vectors = Vec::with_capacity(picked.len());
    let mut last_err = String::new();
    for index in picked {
        let item = &pending[index];
        match extractor.describe(Path::new(&item.path), item.duration_ms) {
            Ok(vector) => vectors.push((item.id, vector)),
            Err(e) => {
                log::warn!("acoustic: probing {}: {e}", item.path);
                last_err = e;
            }
        }
    }
    // Timed over what actually described: a file that wouldn't decode cost
    // its share of the clock but produced no track, and counting it would
    // read as the machine being slow rather than the file being broken.
    if vectors.is_empty() {
        return Err(if last_err.is_empty() {
            "nothing decodable".into()
        } else {
            last_err
        });
    }
    let per = started.elapsed().as_secs_f64() / vectors.len() as f64;
    embeddings::upsert_many(&mut conn, source.id(), &vectors).map_err(|e| e.to_string())?;
    Ok(per as f32)
}

/// The blocking half: walk what's missing, analyze in batches, write each
/// batch in one transaction. Resumes by construction, since the work list is
/// whatever has no vector yet.
fn run(
    source: &Source,
    db_path: &Path,
    workers: usize,
    progress: &Progress,
) -> Result<usize, String> {
    // Before the work list, so a model whose weights went missing between
    // the settings page reading them and the pass starting says so instead
    // of counting a library's worth of work it can't do.
    let extractor = Extractor::load(source)?;
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let pending = embeddings::missing(&conn, source.id()).map_err(|e| e.to_string())?;
    progress.total.store(pending.len(), Ordering::Relaxed);
    progress.pace.begin();

    let mut written = 0;
    for batch in pending.chunks(BATCH) {
        if !progress.keep_going() {
            break;
        }
        let vectors = analyze_batch(&extractor, batch, workers, progress);
        written += vectors.len();
        embeddings::upsert_many(&mut conn, source.id(), &vectors).map_err(|e| e.to_string())?;
    }
    Ok(written)
}

/// One batch through a bounded pool. Every track is independent, so the
/// workers just race a cursor down the slice; the order results come back in
/// doesn't matter, they're keyed by id.
///
/// `workers` is the user's pick, clamped to the machine's cores and the
/// batch. It bounds the network extractor too: its forward pass fans out
/// through rayon's one shared pool, so concurrent tracks interleave there
/// rather than fight, and the serial work around the network (decoding,
/// resampling, the mel transform) is most of a track's wall time and only
/// scales by running more tracks at once.
fn analyze_batch(
    extractor: &Extractor,
    batch: &[Pending],
    workers: usize,
    progress: &Progress,
) -> Vec<(i64, Vec<f32>)> {
    let cursor = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(batch.len()));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(workers.max(1))
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

/// Whether a vector is worth storing at all.
///
/// One NaN is not one bad number, it's the whole library's similarity: the
/// query standardizes every dimension against the corpus, a NaN makes that
/// dimension's mean NaN, and every vector measured against it comes back
/// NaN too, so every score ties and the nearest tracks become whichever
/// ones have the lowest ids. A float-format file carrying NaN samples is
/// all it takes: the band logs and the energy spread here propagate it
/// straight through, and so does the network. The storage layer skips a row
/// like this on read as well, but the pass is where there's still a filename
/// to name in the log.
fn usable(vector: &[f32]) -> bool {
    vector.iter().all(|v| v.is_finite())
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

    /// A local model's name comes off its own bytes, which is the whole
    /// reason a user-supplied file is safe to store beside the catalog's:
    /// two checkpoints can't land in one set of coordinates, and the same
    /// file picked twice keeps the vectors it already wrote.
    #[test]
    fn a_local_model_is_named_after_its_own_bytes() {
        let one = local_id("0f1ccbde4f8c3cdf29d2fa4006cd3bcd5583c9afe4ebeb76eea334e75f0a08e3");
        let two = local_id("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(one, "local-0f1ccbde4f8c3cdf");
        assert_ne!(one, two);
        assert_eq!(local_id(&one), local_id(&one), "and it's a function");
        // Never the catalog's names, whatever the hash.
        assert_ne!(one, MODEL);
        assert_ne!(one, models::PANNS_CNN10);
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

    /// A file whose samples are NaN, which a float-format WAV is free to
    /// hold, describes as NaN. The pass has to catch that before it writes:
    /// one such row standardizes the whole corpus into NaN and every
    /// similarity answer in the library with it.
    #[test]
    fn a_description_carrying_a_nan_is_not_usable() {
        assert!(usable(&features(&tone(440.0, 2.0)).unwrap()));
        let mut audio = tone(440.0, 2.0);
        audio[RATE as usize] = f32::NAN;
        let poisoned = features(&audio).expect("it still frames and describes");
        assert!(
            !usable(&poisoned),
            "a NaN sample went through the statistics unnoticed"
        );
        assert!(!usable(&[1.0, f32::INFINITY]));
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
