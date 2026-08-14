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
//! the similarity queries read, is the app's `settings::acoustic_model`.
//!
//! Adding a third means a [`models::CATALOG`] entry and an [`Extractor`]
//! arm. Nothing about the storage underneath ([`rox_library::embeddings`])
//! changes at all.
//!
//! A weights file the user points rox at is the same thing without a catalog
//! entry: [`Source`] carries either, and a local file takes its name from its
//! own hash so its vectors sit beside the catalog's rather than in them.
//!
//! ## What this crate is and isn't
//!
//! Everything here is blocking compute over paths and a database: the
//! extractors, the catalog, the download, and [`run`], which is the pass
//! itself. The app-global bookkeeping around it, the `Arc<Progress>` a UI
//! samples on a timer and the spawn that keeps it off the main thread, is
//! rox's `embeddings` module, which re-exports all of this so the paths on
//! that side read the way they always have.
//!
//! The pass is the ReplayGain measurement's shape (rox's `replaygain_job`),
//! differing in working through a bounded pool rather than one file at a
//! time, because every track here is independent and there are thirty
//! seconds of decoding in each.

// The mel module's config vocabulary is deliberately complete rather than
// trimmed to what today's one model asks for: the enums are how the module
// states which conventions exist and which this recipe picked, and the
// filterbank and the log branch on them. Trimming to the used arms would
// turn "Slaney rather than HTK" from a decision the code makes into a
// comment, which is exactly the class of mistake that module exists to stop.
pub mod mel;
pub mod models;
pub mod panns;
pub mod resample;
pub mod tempo;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rox_core::pace::Pace;
use rox_core::settings::AcousticSave;
use rox_library::embeddings::{self, Pending};
use rox_library::{embed_tag, store, writer};
use rox_viz::analysis::{self, Analyzer};

use crate::models::Model;

/// The built-in extractor's name. It sits in rox-core, which the settings
/// file's default model pick is written in terms of, and comes back out here
/// where the extractor that answers to it lives.
pub use rox_core::acoustic::MODEL;

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
/// Tracks a pass must get through before its rate is worth remembering as
/// this machine's pace. Half a batch: enough to wash out the cold start,
/// small enough that stopping a pass early still teaches the estimate
/// something.
pub const PACE_FLOOR: usize = 16;

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

    /// How wide this extractor's vectors are. What the tag read-back checks
    /// a stored value against: a vector of another width came from a model
    /// whose output changed under the same name, and taking it would put two
    /// coordinate systems in one table.
    fn dim(&self) -> usize {
        match self {
            Extractor::Dsp => DIM,
            Extractor::Panns(_) => panns::DIM,
        }
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
    /// Raised by [`Progress::cancel`] and by app quit; the workers drop out
    /// at the next file.
    cancel: AtomicBool,
    /// The pass's clock, for the "about 2 hours left" half of the readout.
    /// Started once the work list is built, so the model load and the
    /// database walk don't bill the first track.
    pace: Pace,
}

impl Progress {
    /// A fresh readout for a pass about to run `model`.
    pub fn new(model: &str) -> Self {
        let progress = Progress::default();
        *progress.model.lock().unwrap() = model.to_string();
        progress
    }

    /// Ask the pass to stop at the next file. What it already wrote stays.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

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

/// Time a few tracks to learn what this machine costs per track, so a first
/// pass can be priced before anyone commits an afternoon to it. Returns
/// worker-seconds per track, the unit [`rox_core::pace::estimate`] divides.
///
/// Sequential on one thread on purpose: one track at a time is exactly one
/// worker-second per second, so the number needs no correcting for the pool
/// it was measured under.
///
/// The vectors it produces are kept, in the database only. They're the same
/// vectors the pass would have written for those tracks, so throwing them
/// away to keep the probe tidy would mean decoding them twice for nothing.
/// No tag is written even with the setting on, for the reason the ReplayGain
/// probe writes nothing at all: rewriting three of someone's audio files is
/// not what a button called Estimate should do. The cost is that those few
/// tracks carry no tag until something clears their rows.
///
/// Blocking, and slow enough to want a background thread: it decodes and
/// describes real files, which is the whole point.
pub fn measure_pace(source: &Source, db_path: &Path) -> Result<f32, String> {
    let extractor = Extractor::load(source)?;
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let pending = embeddings::missing(&conn, source.id()).map_err(|e| e.to_string())?;
    let picked = rox_core::pace::sample_indices(pending.len(), rox_core::pace::PROBE_TRACKS);
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

/// What a finished pass leaves behind: how many tracks it described, and
/// which files it rewrote getting there.
///
/// The paths matter to the caller for one reason: rox watches its own library
/// folders, and a tag write it doesn't know about comes back through the
/// watcher as a change to reindex. The app notes them as its own before the
/// watch batch lands. Empty in database mode, which rewrites nothing.
#[derive(Debug, Default)]
pub struct Analyzed {
    pub described: usize,
    pub tagged: Vec<PathBuf>,
}

/// The blocking half: walk what's missing, analyze in batches, write each
/// batch in one transaction. Resumes by construction, since the work list is
/// whatever has no vector yet.
///
/// Every track's vector goes into the database. `save` only decides whether
/// a second copy goes into the file's own tags on the way past; see
/// [`AcousticSave`] for what that buys and what it costs.
///
/// Blocking, and long: this is the whole pass, and the caller is expected to
/// be a background thread with the `progress` handle shared out to whatever
/// is drawing it.
pub fn run(
    source: &Source,
    db_path: &Path,
    workers: usize,
    save: AcousticSave,
    progress: &Progress,
) -> Result<Analyzed, String> {
    // Before the work list, so a model whose weights went missing between
    // the settings page reading them and the pass starting says so instead
    // of counting a library's worth of work it can't do.
    let extractor = Extractor::load(source)?;
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let pending = embeddings::missing(&conn, source.id()).map_err(|e| e.to_string())?;
    progress.total.store(pending.len(), Ordering::Relaxed);
    progress.pace.begin();

    let mut out = Analyzed::default();
    for batch in pending.chunks(BATCH) {
        if !progress.keep_going() {
            break;
        }
        let (vectors, tagged) =
            analyze_batch(&extractor, source.id(), batch, workers, save, progress);
        out.described += vectors.len();
        out.tagged.extend(tagged);
        embeddings::upsert_many(&mut conn, source.id(), &vectors).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Whether this track's vector may go into its own file.
///
/// Three things have to hold, and all three fail quietly: the pass was asked
/// for tags, the file is a format the writer handles ([`embed_tag::writable`]
/// is MP3 and FLAC), and the track is a file rather than a slice of one. A
/// cue subsong is the interesting refusal: twelve tracks share one image, so
/// the last one to finish would leave the whole disc claiming to sound like
/// itself. Those tracks keep their database row and nothing more, which is
/// the same deal every unsupported format gets.
fn tags_this_track(save: AcousticSave, item: &Pending) -> bool {
    save == AcousticSave::Tags
        && writer::writes_to_file(item.sub)
        && embed_tag::writable(Path::new(&item.path))
}

/// A vector already in the file, or None to go and work one out.
///
/// Tried before every decode, whatever `save` says, because a tag rox wrote
/// last month is worth reading whether or not this pass would write one: a
/// wiped database, a folder copied off another machine, and a library
/// rebuilt from scratch all land here, and the alternative is thirty seconds
/// of decoding per track to recompute something the file is holding. The
/// value carries the model and the width and is refused unless both match,
/// so a hit is the same vector the extractor would have produced, to f16.
fn recover(extractor: &Extractor, model: &str, item: &Pending) -> Option<Vec<f32>> {
    if !writer::writes_to_file(item.sub) || !embed_tag::writable(Path::new(&item.path)) {
        return None;
    }
    embed_tag::read(Path::new(&item.path), model, extractor.dim())
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
    model: &str,
    batch: &[Pending],
    workers: usize,
    save: AcousticSave,
    progress: &Progress,
) -> (Vec<(i64, Vec<f32>)>, Vec<PathBuf>) {
    let cursor = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(batch.len()));
    let tagged = Mutex::new(Vec::new());
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
                // The file's own tag first: a hit is a description this
                // library already paid for, and taking it skips the decode
                // entirely. Nothing is written back on a hit, since what
                // would be written is what was just read.
                if let Some(vector) = recover(extractor, model, item) {
                    out.lock().unwrap().push((item.id, vector));
                    progress.done.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                match extractor.describe(Path::new(&item.path), item.duration_ms) {
                    Ok(vector) => {
                        if tags_this_track(save, item) {
                            let path = PathBuf::from(&item.path);
                            match writer::commit_embedding(&path, model, &vector) {
                                Ok(()) => tagged.lock().unwrap().push(path),
                                // The vector is good and the row below takes
                                // it either way, so a file that wouldn't take
                                // a tag costs its tag and nothing else. Not
                                // counted as a failure: the track is
                                // described, which is what the readout is
                                // counting.
                                Err(e) => log::warn!("acoustic: tagging {}: {e}", item.path),
                            }
                        }
                        out.lock().unwrap().push((item.id, vector));
                    }
                    Err(e) => {
                        log::warn!("acoustic: {}: {e}", item.path);
                        progress.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    (out.into_inner().unwrap(), tagged.into_inner().unwrap())
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
    let mut flux = Flux::default();
    let mut energy = Vec::new();

    let mut start = 0;
    while start + FFT <= mono.len() {
        let frame = &mono[start..start + FFT];
        start += HOP;
        energy.push((frame.iter().map(|s| s * s).sum::<f32>() / FFT as f32).sqrt());
        let mags = analyzer.magnitudes(frame);

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

        flux.push(mags);
    }
    let flux = flux.curve;
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

/// The novelty curve as it builds up, one frame's half-spectrum at a time.
///
/// A struct fed frame by frame rather than a function over the samples,
/// because [`features`] is already walking the frames for its own
/// statistics and there's no reason it should pay for a second transform of
/// the same audio: it hands the magnitudes it already has straight here.
/// [`novelty_split`] is that same walk for callers who want nothing else
/// out of the window.
#[derive(Default)]
struct Flux {
    /// One value per hop after the first: how much magnitude appeared since
    /// the frame before, averaged over the bins.
    curve: Vec<f32>,
    /// None until the second frame: the first has nothing behind it, and
    /// measuring it against silence would score the window's own opening
    /// edge as the loudest onset in the track.
    previous: Option<Vec<f32>>,
}

impl Flux {
    /// Take one frame's magnitudes. Half-wave rectified: what appeared since
    /// the last frame, not what faded out. A note starting is an onset, a
    /// note ending isn't.
    fn push(&mut self, mags: &[f32]) {
        if let Some(previous) = &self.previous {
            let rise: f32 = mags
                .iter()
                .zip(previous)
                .map(|(m, p)| (m - p).max(0.0))
                .sum();
            self.curve.push(rise / mags.len() as f32);
        }
        match &mut self.previous {
            Some(previous) => {
                previous.clear();
                previous.extend_from_slice(mags);
            }
            None => self.previous = Some(mags.to_vec()),
        }
    }
}

/// Where the drum band ends. Kick and snare body live under here; hats,
/// cymbals and strummed strings almost entirely above. The exact figure is
/// loose on purpose: it only has to catch the drums that carry a beat and
/// miss the brightness that carries its subdivisions.
const DRUMS_HZ: f32 = 350.0;

/// One mono window's novelty, twice over one walk: the full-band curve,
/// [`Flux`] over every frame with one value per [`HOP`] and so one every
/// 23 ms at [`RATE`], and the same flux summed over only the bins under
/// [`DRUMS_HZ`].
///
/// The full curve is the rhythm signal the whole crate reads. [`features`]
/// reduces it to a mean, a spread and an onset rate; [`tempo`] looks for
/// the lag it repeats at. Cost is one [`FFT`]-wide transform per hop, which
/// is the same order as describing the window, so a caller wanting both
/// should expect to pay twice.
///
/// The low curve exists for the tempo estimator's octave. A full-band
/// curve says something starts, never what: a hat between two kicks makes
/// as much flux as a third kick would, and the two read identically at
/// every lag. The low curve is the one place the difference survives, since
/// a kick lands in it and a hat doesn't, so it's what can say whether the
/// events between a candidate's beats are drums or decoration.
fn novelty_split(mono: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let bins = (DRUMS_HZ / (RATE as f32 / FFT as f32)) as usize;
    let mut analyzer = Analyzer::new(FFT);
    let mut full = Flux::default();
    let mut low = Flux::default();
    let mut start = 0;
    while start + FFT <= mono.len() {
        let mags = analyzer.magnitudes(&mono[start..start + FFT]);
        full.push(mags);
        low.push(&mags[..bins.min(mags.len())]);
        start += HOP;
    }
    (full.curve, low.curve)
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

    fn pending(path: &str, sub: u16) -> Pending {
        Pending {
            id: 1,
            path: path.into(),
            duration_ms: 200_000,
            sub,
        }
    }

    /// Which tracks a tags-mode pass actually writes into. The three
    /// refusals are the ones that would otherwise be discovered by a user:
    /// an OGG library where nothing seemed to save, a cue rip where every
    /// track stamped the same image, and a database-mode pass rewriting
    /// files nobody asked it to touch.
    #[test]
    fn only_whole_files_in_a_writable_format_are_offered_a_tag() {
        use AcousticSave::{Database, Tags};

        assert!(tags_this_track(Tags, &pending("/m/a.mp3", 0)));
        assert!(tags_this_track(Tags, &pending("/m/a.flac", 0)));
        // Database mode never touches a file, whatever it is.
        assert!(!tags_this_track(Database, &pending("/m/a.mp3", 0)));
        // A format the writer has no path for keeps its row and nothing more.
        assert!(!tags_this_track(Tags, &pending("/m/a.ogg", 0)));
        assert!(!tags_this_track(Tags, &pending("/m/a.wav", 0)));
        // A cue subsong is a span of an image twelve tracks share, so there
        // is nowhere on disk that means "track four sounds like this".
        assert!(!tags_this_track(Tags, &pending("/m/disc.flac", 4)));
    }

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

    /// [`features`] over [`pulses`] at eight a second, as it described that
    /// signal before the flux curve was pulled out into [`Flux`] and
    /// [`novelty_split`] for the tempo estimator to share.
    ///
    /// Written down because a library's vectors are only comparable to each
    /// other: a change here that shifts a number by a hundredth doesn't
    /// break anything visibly, it quietly makes every track analyzed after
    /// it a slightly different distance from every track analyzed before,
    /// and there is nothing in the app that would say so.
    const BEFORE_THE_FLUX_MOVED: [f32; DIM] = [
        -15.965477,
        -15.965543,
        -15.965543,
        -15.966264,
        -15.9669695,
        -15.966479,
        -15.963373,
        -15.959001,
        -15.956571,
        -15.956884,
        -15.948458,
        -15.942306,
        -15.931433,
        -15.921043,
        -15.903531,
        -15.881925,
        -15.851968,
        -15.806578,
        -15.749245,
        -15.657156,
        -15.514209,
        -15.214363,
        -14.532366,
        -15.310902,
        -15.710731,
        -15.948189,
        -16.123562,
        -16.263449,
        6.1063194,
        6.1047564,
        6.1047564,
        6.102757,
        6.1013722,
        6.1017203,
        6.1052575,
        6.1093645,
        6.1091866,
        6.106963,
        6.1148543,
        6.1175547,
        6.1279283,
        6.1366906,
        6.1538215,
        6.1755953,
        6.2045255,
        6.2501903,
        6.3124237,
        6.408907,
        6.566268,
        6.8987045,
        7.7230873,
        6.7881427,
        6.3496385,
        6.096466,
        5.912886,
        5.7709303,
        8.687916,
        0.12176962,
        8.972943,
        0.27300486,
        0.00045497756,
        0.0009282503,
        6.0,
        0.050930053,
    ];

    /// The description is what it was, to the bit.
    #[test]
    fn pulling_the_flux_curve_out_didnt_move_a_number() {
        assert_eq!(features(&pulses(8.0, 2.0)).unwrap(), BEFORE_THE_FLUX_MOVED);
    }

    /// And the curve [`novelty_split`] hands the tempo estimator is the
    /// same curve [`features`] reduced, rather than a second one computed
    /// alongside it. The flux mean, the flux spread and the onset rate are
    /// the three numbers in the vector that come off it, so all three
    /// landing exactly is the whole claim.
    #[test]
    fn the_curve_the_tempo_estimator_reads_is_the_one_the_vector_came_from() {
        let audio = pulses(8.0, 2.0);
        let vector = features(&audio).unwrap();
        let curve = novelty_split(&audio).0;
        let (mean, std) = mean_std(&curve);
        assert_eq!((mean, std), (vector[DIM - 4], vector[DIM - 3]));
        let secs = audio.len() as f32 / RATE as f32;
        assert_eq!(onset_rate(&curve, secs), vector[DIM - 2]);
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
