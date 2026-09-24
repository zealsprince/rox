//! The acoustic analysis pass: a vector per track describing how it sounds,
//! so the library can answer "more like this" offline.
//!
//! Two extractors. [`MODEL`] is a built-in timbre and rhythm sketch (log-band
//! statistics, centroid, rolloff, flux, onset rate) on DSP already in the
//! tree: no download, no native dependency. [`panns`] runs PANNs CNN10 through
//! candle on downloaded weights ([`models`]). Vectors are stored per model
//! name, so both can coexist and switching costs nothing already analyzed. A
//! third extractor is a [`models::CATALOG`] entry and an [`Extractor`] arm;
//! a user's own weights file is a [`Source`] named by its hash.
//!
//! Blocking compute only: extractors, catalog, download, and [`run`]. The
//! app-global progress and spawning live in rox's `embeddings` module.

// Don't trim mel's config enums to the arms in use: they're how the module
// states which convention each recipe picked.
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

/// Defined in rox-core, where the settings default names it.
pub use rox_core::acoustic::MODEL;

/// Each band gives a mean and a spread, most of the vector.
const BANDS: usize = 28;

/// The bands twice over, then centroid, rolloff, flux and energy statistics,
/// and the onset rate.
pub const DIM: usize = BANDS * 2 + 8;

/// So files at different rates produce comparable numbers.
const RATE: u32 = 44_100;
const WINDOW_SECS: f64 = 10.0;
/// Across the range a window can start in. Three, since one window of a
/// quiet intro only describes the intro.
const PROBES: [f64; 3] = [0.25, 0.5, 0.75];

const FFT: usize = 2048;
const HOP: usize = FFT / 2;
const LO_HZ: f32 = 40.0;
const HI_HZ: f32 = 16_000.0;
/// The frequency under which this share of the frame's energy falls.
const ROLLOFF: f32 = 0.85;

// The onset trigger, shaped like rox-viz's onset signal.
const ONSET_RATIO: f32 = 1.6;
const ONSET_REARM: f32 = 1.15;
const ONSET_FLOOR: f32 = 1e-5;
const ONSET_ATTACK: f32 = 0.4;
const ONSET_RELEASE: f32 = 0.1;

/// Big enough that commits aren't the cost, small enough that a cancel
/// loses little.
const BATCH: usize = 32;
/// Tracks needed before a pass's rate counts as this machine's pace.
pub const PACE_FLOOR: usize = 16;

/// Where a pass's weights come from: the catalog, or a file the user picked.
/// A local file has no catalog checksum, so its name comes from its hash:
/// another checkpoint is another vector space, and sharing a name would mix
/// them in one table.
#[derive(Clone)]
pub enum Source {
    Catalog(&'static Model),
    /// An `Arc` so settings, the settings page, and a running pass share it.
    Local(Arc<Local>),
}

pub struct Local {
    pub path: PathBuf,
    /// From [`local_id`]: checkpoints never collide, and the same file keeps its vectors.
    pub id: String,
}

/// A prefix and 16 hex digits (64 bits) of the SHA-256: collision-free in
/// practice and short enough for a log line.
pub fn local_id(sha256: &str) -> String {
    format!("local-{}", &sha256[..sha256.len().min(16)])
}

impl Source {
    pub fn id(&self) -> &str {
        match self {
            Source::Catalog(model) => model.id,
            Source::Local(local) => &local.id,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Source::Catalog(model) => model.label.to_string(),
            // The id is a hash; show the file name.
            Source::Local(local) => local
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Custom model".into()),
        }
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, Source::Catalog(model) if model.id == MODEL)
    }

    /// Cheap enough for a settings render. Whether it's the right file is checked
    /// in [`Extractor::load`].
    pub fn installed(&self) -> bool {
        match self {
            Source::Catalog(model) => model.installed(),
            Source::Local(local) => local.path.is_file(),
        }
    }
}

/// The model arm holds the loaded network (24 MB of weights), built once per pass.
pub enum Extractor {
    Dsp,
    // Boxed so every `Extractor` doesn't carry the larger arm.
    Panns(Box<panns::Cnn10>),
}

impl Extractor {
    /// Missing, corrupt, or wrong-network weights fail here, before the pass starts.
    pub fn load(source: &Source) -> Result<Self, String> {
        let net = match source {
            Source::Catalog(model) => match model.id {
                MODEL => return Ok(Extractor::Dsp),
                models::PANNS_CNN10 => panns::Cnn10::load(model)?,
                other => return Err(format!("no extractor is built for {other}")),
            },
            // No checksum, so the load validates: named tensors at fixed shapes, and the
            // stored mel filterbank against the computed one.
            Source::Local(local) => panns::Cnn10::load_from(&local.path)?,
        };
        log::info!(
            "acoustic: {} loaded, running on {}",
            source.id(),
            net.device()
        );
        Ok(Extractor::Panns(Box::new(net)))
    }

    /// What the tag read-back checks: another width means the model changed
    /// under the same name.
    fn dim(&self) -> usize {
        match self {
            Extractor::Dsp => DIM,
            Extractor::Panns(_) => panns::DIM,
        }
    }

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

/// Written per file by the workers, polled by the UI. Zero total means the
/// work list is still being built.
#[derive(Default)]
pub struct Progress {
    /// Named so a readout survives the selection changing.
    model: Mutex<String>,
    done: AtomicUsize,
    total: AtomicUsize,
    failed: AtomicUsize,
    /// Whichever worker wrote last: a sample, not a position.
    current: Mutex<String>,
    /// Raised by [`Progress::cancel`] and app quit.
    cancel: AtomicBool,
    /// Started after the work list is built, so setup doesn't bill the first track.
    pace: Pace,
}

impl Progress {
    pub fn new(model: &str) -> Self {
        let progress = Progress::default();
        *progress.model.lock().unwrap() = model.to_string();
        progress
    }

    /// What it already wrote stays.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Zero while the work list is still being built.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// None until enough tracks finish.
    pub fn secs_per_track(&self) -> Option<f64> {
        self.pace.secs_per_track(self.done())
    }

    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// Time a few tracks to price a first pass, in worker-seconds per track (the
/// unit [`rox_core::pace::estimate`] divides). Sequential, so it needs no pool
/// correction. The vectors are kept, in the database only: never rewrite
/// someone's files from a button called Estimate. Blocking.
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
    // Timed over what described, so a broken file doesn't read as a slow machine.
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

/// Tracks described, and files rewritten. rox watches its own folders, so the
/// app claims these writes before the watcher reports them.
#[derive(Debug, Default)]
pub struct Analyzed {
    pub described: usize,
    pub tagged: Vec<PathBuf>,
}

/// The pass: analyze what's missing in batches, one transaction each, so it
/// resumes by construction. Every vector goes in the database; `save` adds a
/// copy in the file's tags (see [`AcousticSave`]). Blocking and long.
pub fn run(
    source: &Source,
    db_path: &Path,
    workers: usize,
    save: AcousticSave,
    progress: &Progress,
) -> Result<Analyzed, String> {
    // Before the work list, so missing weights fail fast.
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

/// Tags only when asked, for MP3 or FLAC ([`embed_tag::writable`]), and for a
/// whole file. Never a cue subsong: its image is shared, and the last track
/// to finish would stamp the whole disc.
fn tags_this_track(save: AcousticSave, item: &Pending) -> bool {
    save == AcousticSave::Tags
        && writer::writes_to_file(item.sub)
        && embed_tag::writable(Path::new(&item.path))
}

/// A vector already in the file's tags, tried before any decode whatever
/// `save` says: a wiped database or a copied folder recovers without
/// decoding. Rejected unless model and width match.
fn recover(extractor: &Extractor, model: &str, item: &Pending) -> Option<Vec<f32>> {
    if !writer::writes_to_file(item.sub) || !embed_tag::writable(Path::new(&item.path)) {
        return None;
    }
    embed_tag::read(Path::new(&item.path), model, extractor.dim())
}

/// One batch through a bounded pool racing a cursor; results are keyed by id.
/// `workers` bounds the network extractor too: its forward pass shares
/// rayon's pool, and decode, resample, and mel dominate a track's time.
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
            scope.spawn(|| {
                loop {
                    if !progress.keep_going() {
                        break;
                    }
                    let Some(item) = batch.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    *progress.current.lock().unwrap() = item.path.clone();
                    // The file's own tag first: a hit skips the decode, and needs no write-back.
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
                                    // A failed tag write costs only the tag; the row still lands, and it isn't
                                    // counted as a failure.
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
                }
            });
        }
    });
    (out.into_inner().unwrap(), tagged.into_inner().unwrap())
}

/// Three windows, described and averaged: one vector per window would make
/// the same song look different depending on where the probes fell.
pub fn extract(path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
    let duration = duration_ms as f64 / 1000.0;
    let frames = (WINDOW_SECS * RATE as f64) as usize;
    let locator = rox_library::locator::Locator::Local(path.to_path_buf());
    // A short track gets one window from the top; longer ones spread the probes
    // where a window fits.
    let single = duration <= WINDOW_SECS;
    let span = (duration - WINDOW_SECS).max(0.0);

    let mut sum = vec![0f64; DIM];
    let mut taken = 0usize;
    let mut last_err = String::new();
    for probe in PROBES {
        let stereo = match rox_playback::engine::decode_window(&locator, span * probe, RATE, frames)
        {
            Ok(stereo) => stereo,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let mono: Vec<f32> = stereo
            .as_chunks::<2>()
            .0
            .iter()
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

/// One mono window as [`DIM`] raw statistics. The query standardizes them
/// (see [`rox_library::embeddings::nearest`]), so weights can change without
/// re-analysis.
fn features(mono: &[f32]) -> Option<Vec<f32>> {
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
            // Log energy, so a quiet band isn't a rounding error beside a loud one.
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
    // Means, then spreads, so a slice of the vector is one statistic.
    let band_stats: Vec<(f32, f32)> = per_band.iter().map(|v| mean_std(v)).collect();
    out.extend(band_stats.iter().map(|(mean, _)| *mean));
    out.extend(band_stats.iter().map(|(_, std)| *std));
    for values in [&centroid, &rolloff, &flux] {
        let (mean, std) = mean_std(values);
        out.push(mean);
        out.push(std);
    }
    out.push(onset_rate(&flux, secs));
    // The one dynamics number: a compressed master barely moves.
    out.push(mean_std(&energy).1);
    debug_assert_eq!(out.len(), DIM);
    Some(out)
}

/// The novelty curve, fed a frame's magnitudes at a time so [`features`]
/// doesn't pay for a second transform. [`novelty_split`] is the standalone pass.
#[derive(Default)]
struct Flux {
    /// One value per hop after the first: magnitude that appeared, averaged over bins.
    curve: Vec<f32>,
    /// None until the second frame, or the window's opening edge would score as
    /// the loudest onset.
    previous: Option<Vec<f32>>,
}

impl Flux {
    /// Half-wave rectified: a note starting is an onset, a note ending isn't.
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

/// Kick and snare body sit under here; hats and strums above. Loose on purpose.
const DRUMS_HZ: f32 = 350.0;

/// One window's novelty twice, in one pass: full-band [`Flux`] (one value per
/// [`HOP`]) and the same over bins under [`DRUMS_HZ`]. The full curve is the
/// rhythm signal [`features`] and [`tempo`] read. The low curve is for the
/// tempo octave: a hat between kicks makes as much full-band flux as a third
/// kick, but only a kick lands down here.
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

/// A single NaN poisons the whole library's similarity: standardization turns
/// it into NaN everywhere, and every score ties. NaN samples in a float file
/// are enough. Checked here, where there's still a filename to log.
fn usable(vector: &[f32]) -> bool {
    vector.iter().all(|v| v.is_finite())
}

fn mean_std(values: &[f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Spectral jumps per second: what tells a beat from a drone.
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

    /// Only whole MP3/FLAC files in tags mode get a tag.
    #[test]
    fn only_whole_files_in_a_writable_format_are_offered_a_tag() {
        use AcousticSave::{Database, Tags};

        assert!(tags_this_track(Tags, &pending("/m/a.mp3", 0)));
        assert!(tags_this_track(Tags, &pending("/m/a.flac", 0)));
        assert!(!tags_this_track(Database, &pending("/m/a.mp3", 0)));
        assert!(!tags_this_track(Tags, &pending("/m/a.ogg", 0)));
        assert!(!tags_this_track(Tags, &pending("/m/a.wav", 0)));
        // A cue subsong shares its image, so nowhere on disk is only its own.
        assert!(!tags_this_track(Tags, &pending("/m/disc.flac", 4)));
    }

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin() * 0.5)
            .collect()
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

    fn distance(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    }

    /// A local model's name comes from its bytes, never a catalog name.
    #[test]
    fn a_local_model_is_named_after_its_own_bytes() {
        let one = local_id("0f1ccbde4f8c3cdf29d2fa4006cd3bcd5583c9afe4ebeb76eea334e75f0a08e3");
        let two = local_id("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(one, "local-0f1ccbde4f8c3cdf");
        assert_ne!(one, two);
        assert_eq!(local_id(&one), local_id(&one), "and it's a function");
        assert_ne!(one, MODEL);
        assert_ne!(one, models::PANNS_CNN10);
    }

    /// Deterministic, or re-analysis would reshuffle a library's neighbours.
    #[test]
    fn the_same_window_always_describes_the_same() {
        let audio = tone(440.0, 2.0);
        let first = features(&audio).expect("two seconds is enough to describe");
        let second = features(&audio).unwrap();
        assert_eq!(first.len(), DIM);
        assert_eq!(first, second);
    }

    /// A NaN sample describes as NaN, which the pass must refuse to store.
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

    #[test]
    fn too_short_a_window_is_refused() {
        assert!(features(&tone(440.0, 0.01)).is_none());
    }

    /// Neighbours close, something different far: the gap is the point.
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

    /// Bursts decaying over a few milliseconds, so the FFT sees a jump.
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

    /// [`features`] over [`pulses`] at eight a second, pinned: vectors are only
    /// comparable to each other, so a tiny drift would quietly skew everything
    /// analyzed after it.
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

    #[test]
    fn pulling_the_flux_curve_out_didnt_move_a_number() {
        assert_eq!(features(&pulses(8.0, 2.0)).unwrap(), BEFORE_THE_FLUX_MOVED);
    }

    /// [`novelty_split`]'s curve is the one [`features`] reduces: its three
    /// statistics match exactly.
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

    /// Pulses read busy; a held tone and steady noise read still.
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
