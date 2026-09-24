//! Acoustic embeddings: one feature vector per track per model, and the
//! nearest-neighbour query over them. The extractors live in `rox_acoustic`;
//! this module stores the vectors and answers "what sounds like this".
//!
//! Rows are keyed by model name, so a library can hold two models at once. An
//! extractor whose output changes meaning takes a new name.
//!
//! Vectors go in raw. [`nearest`] z-scores each dimension against the corpus
//! at read time, because raw features span wildly different scales and the
//! loud dimensions would otherwise decide every neighbour.
//!
//! The standardization, the quantized corpus and the last few seeds' score
//! maps are held in memory and checked against a [`Fingerprint`] of the
//! table, so a query on an unchanged table never touches the vector blobs.
//!
//! Playback ranks by [`ranked`], the cosine with a tempo penalty on it.
//! [`scores`] stays the raw cosine for the column that prints it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::Connection;

/// Foreign keys are on in the bundled SQLite, so the cascade fires, but that's
/// a build flag and per-connection. Reads join against `tracks` anyway and
/// [`prune`] covers a database written without it.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embeddings (
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            model    TEXT NOT NULL,
            dim      INTEGER NOT NULL,
            vec      BLOB NOT NULL,
            PRIMARY KEY (track_id, model)
        );
        CREATE INDEX IF NOT EXISTS embeddings_model ON embeddings (model);",
    )
}

/// A track waiting on a vector.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: i64,
    pub path: String,
    pub duration_ms: u32,
    /// 0 for a whole file, higher for a cue subsong. Only sub 0 is ever offered a
    /// tag: one subsong's vector written into the image would describe the whole disc.
    pub sub: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub embedded: usize,
    pub total: usize,
}

impl Coverage {
    pub fn missing(self) -> usize {
        self.total.saturating_sub(self.embedded)
    }
}

/// Every local track with no vector for `model`, in id order. Zero-duration
/// rows are skipped: the extractor picks its sample windows as fractions of
/// the running time.
pub fn missing(conn: &Connection, model: &str) -> rusqlite::Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path, t.duration_ms, t.sub FROM tracks t
         LEFT JOIN embeddings e ON e.track_id = t.id AND e.model = ?1
         WHERE e.track_id IS NULL AND t.source = 'local' AND t.duration_ms > 0
         ORDER BY t.id",
    )?;
    let rows = stmt.query_map([model], |r| {
        Ok(Pending {
            id: r.get(0)?,
            path: r.get(1)?,
            duration_ms: r.get::<_, i64>(2)? as u32,
            sub: r.get::<_, i64>(3)? as u16,
        })
    })?;
    rows.collect()
}

/// Both sides count only the rows [`missing`] would offer, so the two numbers
/// converge on a finished pass.
pub fn coverage(conn: &Connection, model: &str) -> rusqlite::Result<Coverage> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'local' AND duration_ms > 0",
        [],
        |r| r.get(0),
    )?;
    let embedded: i64 = conn.query_row(
        "SELECT COUNT(*) FROM embeddings e JOIN tracks t ON t.id = e.track_id
         WHERE e.model = ?1 AND t.source = 'local' AND t.duration_ms > 0",
        [model],
        |r| r.get(0),
    )?;
    Ok(Coverage {
        embedded: embedded as usize,
        total: total as usize,
    })
}

/// Stops at the first row, unlike [`coverage`], so it's cheap enough for menus
/// deciding whether to offer a rank-by-sound mode.
pub fn any(conn: &Connection, model: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM embeddings WHERE model = ?1)",
        [model],
        |r| r.get(0),
    )
}

/// A vector with a NaN or an infinity is silently refused: one of them turns
/// every score in the library into NaN through the corpus mean. Failing the
/// batch would throw away the good tracks beside it.
pub fn upsert(conn: &Connection, track_id: i64, model: &str, vec: &[f32]) -> rusqlite::Result<()> {
    if !vec.iter().all(|v| v.is_finite()) {
        log::warn!("embeddings: refusing a vector with a NaN or an infinity for track {track_id}");
        return Ok(());
    }
    conn.execute(
        "INSERT INTO embeddings (track_id, model, dim, vec) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id, model) DO UPDATE
            SET dim = excluded.dim, vec = excluded.vec",
        rusqlite::params![track_id, model, vec.len() as i64, encode(vec)],
    )?;
    note_write(conn);
    Ok(())
}

pub fn upsert_many(
    conn: &mut Connection,
    model: &str,
    rows: &[(i64, Vec<f32>)],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for (id, vec) in rows {
        upsert(&tx, *id, model, vec)?;
    }
    tx.commit()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRows {
    pub model: String,
    pub rows: u64,
    /// Measured off a stored blob rather than the `dim` column, so a row whose
    /// width and column disagree is reported as what it costs.
    pub dim: usize,
}

/// Includes models this build doesn't know, so a renamed extractor's leftover
/// rows show up.
pub fn models(conn: &Connection) -> rusqlite::Result<Vec<ModelRows>> {
    let mut stmt = conn.prepare(
        "SELECT model, COUNT(*), COALESCE(MAX(length(vec)), 0) FROM embeddings
         GROUP BY model ORDER BY model",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ModelRows {
            model: r.get(0)?,
            rows: r.get::<_, i64>(1)? as u64,
            dim: r.get::<_, i64>(2)? as usize / 4,
        })
    })?;
    rows.collect()
}

/// Drop every vector for a model, then VACUUM so the file actually shrinks.
/// Keep it off the UI thread and outside a transaction. A VACUUM refused by a
/// busy reader is a warning, since the rows are gone either way.
pub fn clear(conn: &Connection, model: &str) -> rusqlite::Result<usize> {
    let dropped = conn.execute("DELETE FROM embeddings WHERE model = ?1", [model])?;
    note_write(conn);
    if let Err(e) = conn.execute_batch("VACUUM;") {
        log::warn!("embeddings: cleared {model} but the file kept its pages: {e}");
    }
    Ok(dropped)
}

/// Drop vectors whose track is gone. A no-op while the cascade works (see
/// [`init_schema`]).
pub fn prune(conn: &Connection) -> rusqlite::Result<usize> {
    let dropped = conn.execute(
        "DELETE FROM embeddings WHERE track_id NOT IN (SELECT id FROM tracks)",
        [],
    )?;
    note_write(conn);
    Ok(dropped)
}

/// Past this many rows the scan takes an evenly spaced slice. It bounds the
/// held [`Corpus`] at a byte per dimension per candidate: a hundred megabytes
/// at the cap under the shipped model, twenty-five for a fifty-thousand-track
/// library.
pub const CANDIDATE_CAP: usize = 200_000;

/// Rows the standardization statistics are estimated from.
const STATS_SAMPLE: usize = 20_000;

/// How many sigmas out a standardized cell is allowed to sit.
///
/// Under the shipped model a handful of near-constant dimensions jump to 50 to
/// 150 sigma on one track in a few thousand, and one such cell is then the
/// whole row: measured on the live library, a spiking track has a length of 60
/// to 150 against a median of 17, with 95% of it in two cells. Its neighbours
/// become whatever else spikes there. One track in eight has a cell past ten
/// sigma.
///
/// Prototyped at three, four, a signed log and a tanh: all agree to within a
/// place or two, so this is the simplest.
const Z_CLIP: f32 = 4.0;

/// Per-dimension standardization for one model's corpus.
#[derive(Clone, Debug)]
pub struct Stats {
    dim: usize,
    mean: Vec<f32>,
    /// Reciprocal standard deviation. Zero for a dimension that never varies.
    inv_std: Vec<f32>,
}

impl Stats {
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Standardize into `out`, clipped to [`Z_CLIP`]. Length is left alone:
    /// [`Corpus`] normalizes against the bytes it actually scores.
    fn standardize(&self, raw: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend(
            raw.iter()
                .zip(&self.mean)
                .zip(&self.inv_std)
                .map(|((v, m), i)| ((v - m) * i).clamp(-Z_CLIP, Z_CLIP)),
        );
    }
}

/// The width most of a model's vectors share. Rows of any other width take no
/// part.
fn dominant_dim(conn: &Connection, model: &str) -> rusqlite::Result<Option<usize>> {
    conn.query_row(
        "SELECT dim FROM embeddings WHERE model = ?1
         GROUP BY dim ORDER BY COUNT(*) DESC LIMIT 1",
        [model],
        |r| r.get::<_, i64>(0),
    )
    .map(|d| Some(d as usize))
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

fn model_rows(conn: &Connection, model: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM embeddings WHERE model = ?1",
        [model],
        |r| r.get(0),
    )
}

/// Stride over a `total`-row corpus so no more than `cap` rows come back.
///
/// A plain stride over a sequential scan, measured: making the read selective
/// forces SQLite into scattered rowid fetches, which cost several times a
/// straight table read. Deterministic, so a score column is stable between
/// calls.
fn stride_from(total: i64, cap: usize) -> i64 {
    // Ceiling division: a corpus of exactly the cap is still read whole.
    let total = total.max(0) as usize;
    total.div_ceil(cap.max(1)).max(1) as i64
}

/// Visit each of a model's vectors with its row's believable tempo. The vector
/// is borrowed from one reused buffer.
fn each_vector(
    conn: &Connection,
    model: &str,
    dim: usize,
    stride: i64,
    mut visit: impl FnMut(i64, &[f32], Option<f32>),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT e.track_id, e.vec, t.bpm FROM embeddings e
         JOIN tracks t ON t.id = e.track_id
         WHERE e.model = ?1 AND e.dim = ?2 AND (?3 = 1 OR e.track_id % ?3 = 0)",
    )?;
    let mut rows = stmt.query(rusqlite::params![model, dim as i64, stride])?;
    let mut buf: Vec<f32> = Vec::with_capacity(dim);
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let bpm = believable_bpm(row.get(2)?);
        let bytes = row.get_ref(1)?.as_blob()?;
        buf.clear();
        buf.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c)),
        );
        // Skip wrong-width and non-finite blobs. One NaN makes a dimension's mean
        // NaN, the variance clamp reads it as zero (f64::max ignores NaN), and every
        // score ties, so "nearest" means "lowest id".
        if buf.len() == dim && buf.iter().all(|v| v.is_finite()) {
            visit(id, &buf, bpm);
        }
    }
    Ok(())
}

/// The one place a stored bpm becomes a tempo. Unset, zero and out-of-range
/// values read as no tempo, which earns no penalty rather than a wrong one.
fn believable_bpm(bpm: Option<f64>) -> Option<f32> {
    bpm.map(|b| b as f32)
        .filter(|b| (crate::tempo::SLOWEST..=crate::tempo::FASTEST).contains(b))
}

/// Read per query rather than from the corpus, since the stride may have left
/// the seed out.
fn track_bpm(conn: &Connection, track_id: i64) -> rusqlite::Result<Option<f32>> {
    conn.query_row("SELECT bpm FROM tracks WHERE id = ?1", [track_id], |r| {
        r.get::<_, Option<f64>>(0)
    })
    .map(believable_bpm)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Evidence that a held answer's corpus is still the table's.
///
/// Three signals, because no one of them sees everything. `data_version` only
/// moves for a connection kept open across two readings, and callers open a
/// fresh one per query. `rows` catches passes filling the table, a [`clear`],
/// and cascade deletes. `writes` catches this process replacing a vector in
/// place, and the tempo pass writing a bpm onto `tracks` through
/// [`crate::store::set_measured_bpm`], since the corpus holds every row's tempo.
///
/// Accepted staleness: another process rewriting vectors without changing the
/// count goes unseen until the count moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    data_version: i64,
    rows: i64,
    writes: u64,
}

fn fingerprint(conn: &Connection, model: &str, db: Option<&str>) -> rusqlite::Result<Fingerprint> {
    Ok(Fingerprint {
        data_version: conn.pragma_query_value(None, "data_version", |r| r.get(0))?,
        rows: model_rows(conn, model)?,
        writes: db.map(writes_for).unwrap_or(0),
    })
}

/// None for an in-memory database: there's no path to tell two apart, so
/// they're never held.
fn db_key(conn: &Connection) -> Option<String> {
    conn.path().filter(|p| !p.is_empty()).map(str::to_owned)
}

/// Writes this process has made, per database, so writing a scratch library
/// doesn't drop what the one on screen holds.
static WRITES: RwLock<Option<HashMap<String, u64>>> = RwLock::new(None);

/// Crate-visible because the tempo pass writes onto `tracks` (see
/// [`crate::store::set_measured_bpm`]) and still changes what a ranking returns.
pub(crate) fn note_write(conn: &Connection) {
    let Some(db) = db_key(conn) else {
        return;
    };
    let mut counts = WRITES.write().expect("write counter never poisons");
    *counts
        .get_or_insert_with(HashMap::new)
        .entry(db)
        .or_insert(0) += 1;
}

fn writes_for(db: &str) -> u64 {
    WRITES
        .read()
        .expect("write counter never poisons")
        .as_ref()
        .and_then(|counts| counts.get(db).copied())
        .unwrap_or(0)
}

/// One entry: only one model is ever the pick callers score against.
static STATS_CACHE: RwLock<Option<CachedStats>> = RwLock::new(None);

struct CachedStats {
    db: String,
    model: String,
    at: Fingerprint,
    stats: Option<Arc<Stats>>,
}

/// Score maps for the last few seeds, twelve bytes a track. A fresh seed off
/// the held [`Corpus`] is about six milliseconds; this saves the transport,
/// the Similar column and queue ordering redoing it for the same track.
static SCORES_CACHE: Mutex<Vec<CachedScores>> = Mutex::new(Vec::new());

const SCORE_SEEDS: usize = 4;

struct CachedScores {
    db: String,
    model: String,
    seed: i64,
    at: Fingerprint,
    scores: Arc<Vec<(i64, f32)>>,
}

static CORPUS_CACHE: RwLock<Option<CachedCorpus>> = RwLock::new(None);

struct CachedCorpus {
    db: String,
    model: String,
    at: Fingerprint,
    corpus: Arc<Corpus>,
}

/// -128 goes unused, so a vector and its opposite quantize to opposite cells.
const QUANT_PEAK: f32 = 127.0;

/// Below this a standardized row sits on the corpus mean and points nowhere.
/// Large enough that dividing by it stays inside f32.
const QUANT_FLOOR: f32 = 1e-20;

/// Distance between two tempos in octaves, folded so half and double time
/// count as the same tempo. Zero for 70 against 140, a third for 140 against
/// 175, at most a half. Log space because tempo is heard as a ratio.
///
/// Zero when either side is missing or unmeasurable; [`tempo_penalty`] decides
/// what that costs, and also charges the octaves this fold drops.
fn tempo_distance(a: f32, b: f32) -> f32 {
    let Some(octaves) = tempo_octaves(a, b) else {
        return 0.0;
    };
    (octaves - octaves.round()).abs()
}

/// Unfolded octaves between two tempos, None where the ratio or log has no
/// answer. Shared so both penalty terms agree on what's measurable.
fn tempo_octaves(a: f32, b: f32) -> Option<f32> {
    let measured = |bpm: f32| bpm.is_finite() && bpm > 0.0;
    (measured(a) && measured(b)).then(|| (a / b).log2().abs())
}

/// The cosine a candidate at `b` is charged against a seed at `a`: the folded
/// distance at [`TEMPO_WEIGHT`] plus the unfolded drift at
/// [`TEMPO_DRIFT_WEIGHT`]. Two missing tempos charge nothing; one missing
/// charges [`NO_TEMPO_PENALTY`].
///
/// The fold alone has a hole: 70 against 133 folds to 0.074 octaves, so 70 BPM
/// ambient and 133 BPM EBM read as the same tempo, while 70 against 100 pays
/// 0.145. The drift term prices that.
fn tempo_penalty(a: f32, b: f32) -> f32 {
    let Some(octaves) = tempo_octaves(a, b) else {
        let measured = |bpm: f32| bpm.is_finite() && bpm > 0.0;
        return if measured(a) == measured(b) {
            0.0
        } else {
            NO_TEMPO_PENALTY
        };
    };
    TEMPO_WEIGHT * tempo_distance(a, b) + TEMPO_DRIFT_WEIGHT * octaves.min(DRIFT_CEILING)
}

/// Cosine charged per octave of [`tempo_distance`], so at most half this.
///
/// Calibrated on the live library, fifty thousand tracks, eight evenly spaced
/// seeds. A raw top thirty spans 0.095 of cosine at the median (0.042 tightest,
/// 0.296 loosest). At this weight 140 against 175 costs 0.09, about the whole
/// band. Re-ranking those bands against synthetic tempos kept half the raw top
/// thirty and raised the share at the seed's tempo from a third to four in five.
const TEMPO_WEIGHT: f32 = 0.3;

/// Cosine charged per unfolded octave, on top of [`TEMPO_WEIGHT`].
///
/// Off a 70 BPM seed: 72 pays 0.015, 140 pays 0.060, 133 pays 0.078, 100 pays
/// 0.176. Without it, 133 paid 0.022 and got drawn off an ambient seed past
/// three hundred closer tracks. A clean double at 0.060 is two thirds of the
/// median band. Reasoned from that spread, not a second calibration run.
const TEMPO_DRIFT_WEIGHT: f32 = 0.06;

/// What a track with no tempo pays against a seed with one, either way round.
///
/// The pass refuses tracks it can't hear a beat in, about one in six on a real
/// library, so no tempo is a description. Charging nothing would hand every
/// beatless track a free pass. Twice a clean double, a little over half the
/// worst a measured pair pays.
const NO_TEMPO_PENALTY: f32 = 0.12;

/// Where the drift term stops counting octaves, so a quadruple isn't charged
/// twice a double. Caps the worst any pair pays at 0.21.
const DRIFT_CEILING: f32 = 1.0;

/// One model's candidates, standardized once and held as a byte a dimension.
///
/// Each row is scaled by its own peak. A fixed corpus-wide step lost to this
/// on the live library. With [`Z_CLIP`] the peak is at most four sigma, so the
/// step is never coarser than thirty to a sigma and scores hold within a
/// thousandth of the float answer. Row lengths are measured off the bytes, so
/// the per-row scale cancels and scores stay inside -1..=1.
struct Corpus {
    dim: usize,
    ids: Vec<i64>,
    /// Row major, `dim` cells a track, in step with `ids`.
    cells: Vec<i8>,
    inv_norm: Vec<f32>,
    /// NaN for no believable tempo. Read in the same scan so ranking by tempo
    /// costs no per-candidate query.
    bpm: Vec<f32>,
}

impl Corpus {
    /// `rows` is the fingerprint's count, used for the stride and to size the
    /// buffer up front.
    fn build(conn: &Connection, model: &str, stats: &Stats, rows: i64) -> rusqlite::Result<Corpus> {
        let stride = stride_from(rows, CANDIDATE_CAP);
        let expect = (rows.max(0) as usize).div_ceil(stride.max(1) as usize);
        let mut corpus = Corpus {
            dim: stats.dim,
            ids: Vec::with_capacity(expect),
            cells: Vec::with_capacity(expect * stats.dim),
            inv_norm: Vec::with_capacity(expect),
            bpm: Vec::with_capacity(expect),
        };
        let mut z = Vec::with_capacity(stats.dim);
        each_vector(conn, model, stats.dim, stride, |id, vec, bpm| {
            stats.standardize(vec, &mut z);
            corpus.push(id, &z, bpm);
        })?;
        log::debug!(
            "embeddings: corpus for {model} holds {} vectors, {} MB",
            corpus.ids.len(),
            corpus.cells.len() / (1024 * 1024)
        );
        Ok(corpus)
    }

    fn push(&mut self, id: i64, z: &[f32], bpm: Option<f32>) {
        let inv = quantize(z, &mut self.cells);
        self.ids.push(id);
        self.inv_norm.push(inv);
        self.bpm.push(bpm.unwrap_or(f32::NAN));
    }

    /// Integer arithmetic to the end: the widest dot is 127 squared times the
    /// width, twenty-three bits under the shipped model.
    fn scores(&self, seed: &[i8], seed_inv: f32, skip: i64) -> Vec<(i64, f32)> {
        let mut out = Vec::with_capacity(self.ids.len());
        let rows = self.cells.chunks_exact(self.dim.max(1));
        for ((row, &id), &inv) in rows.zip(&self.ids).zip(&self.inv_norm) {
            if id == skip {
                continue;
            }
            let mut acc = 0i32;
            for (a, b) in row.iter().zip(seed) {
                acc += i32::from(*a) * i32::from(*b);
            }
            out.push((id, acc as f32 * seed_inv * inv));
        }
        out
    }

    /// Charge every score for its tempo gap to the seed, in place.
    ///
    /// `scores` must be what [`Corpus::scores`] returned for this same corpus, so
    /// one cursor walks both. A row whose id doesn't match is left raw.
    fn penalize(&self, scores: &mut [(i64, f32)], seed_bpm: f32) {
        let mut at = 0usize;
        for (&id, &bpm) in self.ids.iter().zip(&self.bpm) {
            let Some(row) = scores.get_mut(at) else {
                break;
            };
            if row.0 != id {
                continue;
            }
            row.1 -= tempo_penalty(seed_bpm, bpm);
            at += 1;
        }
    }
}

/// Append a row as bytes and return the reciprocal length written. A row of
/// nothing gets zero, which scores zero against everything.
fn quantize(z: &[f32], out: &mut Vec<i8>) -> f32 {
    let peak = z.iter().fold(0f32, |m, v| m.max(v.abs()));
    let scale = if peak > QUANT_FLOOR {
        QUANT_PEAK / peak
    } else {
        0.0
    };
    let start = out.len();
    out.extend(
        z.iter()
            .map(|v| (v * scale).round().clamp(-QUANT_PEAK, QUANT_PEAK) as i8),
    );
    inv_len(square_sum(&out[start..]))
}

fn square_sum(cells: &[i8]) -> i64 {
    cells.iter().map(|&c| i64::from(c) * i64::from(c)).sum()
}

fn inv_len(square_sum: i64) -> f32 {
    if square_sum > 0 {
        (1.0 / (square_sum as f64).sqrt()) as f32
    } else {
        0.0
    }
}

fn quantize_seed(z: &[f32]) -> (Vec<i8>, f32) {
    let mut cells = Vec::with_capacity(z.len());
    let inv = quantize(z, &mut cells);
    (cells, inv)
}

/// Rebuilt whole when the fingerprint moves. During an analysis pass that's a
/// table read per query.
fn held_corpus(
    conn: &Connection,
    model: &str,
    stats: &Stats,
    db: Option<&str>,
    at: Fingerprint,
) -> rusqlite::Result<Arc<Corpus>> {
    if let Some(db) = db {
        let held = CORPUS_CACHE.read().expect("corpus cache never poisons");
        if let Some(entry) = held.as_ref()
            && entry.db == db
            && entry.model == model
            && entry.at == at
        {
            return Ok(entry.corpus.clone());
        }
    }
    let built = Arc::new(Corpus::build(conn, model, stats, at.rows)?);
    if let Some(db) = db {
        *CORPUS_CACHE.write().expect("corpus cache never poisons") = Some(CachedCorpus {
            db: db.to_owned(),
            model: model.to_owned(),
            at,
            corpus: built.clone(),
        });
    }
    Ok(built)
}

/// None when the model has too few vectors for a spread to mean anything.
pub fn stats(conn: &Connection, model: &str) -> rusqlite::Result<Option<Stats>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    Ok(held_stats(conn, model, db.as_deref(), at)?.map(|stats| (*stats).clone()))
}

fn held_stats(
    conn: &Connection,
    model: &str,
    db: Option<&str>,
    at: Fingerprint,
) -> rusqlite::Result<Option<Arc<Stats>>> {
    if let Some(db) = db {
        let held = STATS_CACHE.read().expect("stats cache never poisons");
        if let Some(entry) = held.as_ref()
            && entry.db == db
            && entry.model == model
            && entry.at == at
        {
            return Ok(entry.stats.clone());
        }
    }
    let computed = compute_stats(conn, model, at)?.map(Arc::new);
    if let Some(db) = db {
        *STATS_CACHE.write().expect("stats cache never poisons") = Some(CachedStats {
            db: db.to_owned(),
            model: model.to_owned(),
            at,
            stats: computed.clone(),
        });
    }
    Ok(computed)
}

fn compute_stats(
    conn: &Connection,
    model: &str,
    at: Fingerprint,
) -> rusqlite::Result<Option<Stats>> {
    let Some(dim) = dominant_dim(conn, model)? else {
        return Ok(None);
    };
    let stride = stride_from(at.rows, STATS_SAMPLE);
    let mut n = 0usize;
    let mut sum = vec![0f64; dim];
    let mut sq = vec![0f64; dim];
    each_vector(conn, model, dim, stride, |_, vec, _| {
        n += 1;
        for ((s, q), &v) in sum.iter_mut().zip(sq.iter_mut()).zip(vec) {
            *s += v as f64;
            *q += (v as f64) * (v as f64);
        }
    })?;
    if n < 2 {
        return Ok(None);
    }
    let n = n as f64;
    let mean: Vec<f32> = sum.iter().map(|s| (s / n) as f32).collect();
    let inv_std: Vec<f32> = sq
        .iter()
        .zip(&sum)
        .map(|(q, s)| {
            // Clamped: for a near-constant dimension rounding can go just under zero.
            let var = (q / n - (s / n) * (s / n)).max(0.0);
            let std = var.sqrt();
            if std > 1e-9 { (1.0 / std) as f32 } else { 0.0 }
        })
        .collect();
    Ok(Some(Stats { dim, mean, inv_std }))
}

pub fn vector(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Option<Vec<f32>>> {
    conn.query_row(
        "SELECT vec FROM embeddings WHERE track_id = ?1 AND model = ?2",
        rusqlite::params![track_id, model],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .map(|bytes| Some(decode(&bytes)))
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// Every local vector with its path and sub, for [`crate::bake`].
pub fn embedded(conn: &Connection, model: &str) -> rusqlite::Result<Vec<(String, u16, Vec<f32>)>> {
    let mut stmt = conn.prepare(
        "SELECT t.path, t.sub, e.vec FROM embeddings e
         JOIN tracks t ON t.id = e.track_id
         WHERE e.model = ?1 AND t.source = 'local'
         ORDER BY t.path",
    )?;
    let rows = stmt.query_map([model], |r| {
        Ok((
            r.get(0)?,
            r.get::<_, i64>(1)? as u16,
            decode(&r.get::<_, Vec<u8>>(2)?),
        ))
    })?;
    rows.collect()
}

/// How much every other track resembles `track_id`, in -1..=1, unordered.
/// Empty when the seed has no vector or the corpus is too small.
///
/// The first query after a change reads the table: a hundred megabytes for a
/// fifty-thousand-track library, a third of a second (the workspace builds this
/// crate optimized in dev, or it'd be seconds). After that it's six
/// milliseconds in release, fourteen in debug, over the held [`Corpus`].
///
/// Raw cosine only. The Similar column prints it; playback picks from
/// [`ranked`].
pub fn scores(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    Ok((*held_scores(conn, track_id, model, db, at)?).clone())
}

/// [`scores`] with [`tempo_penalty`] subtracted per candidate. Every pick
/// playback makes runs on this. Subtracting rather than filtering, so a track
/// far better on timbre still wins.
pub fn ranked(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    let mut scored = (*held_scores(conn, track_id, model, db.clone(), at)?).clone();
    // A seed with no tempo still charges the candidates that have one.
    let seed_bpm = track_bpm(conn, track_id)?.unwrap_or(f32::NAN);
    let Some(stats) = held_stats(conn, model, db.as_deref(), at)? else {
        return Ok(scored);
    };
    let corpus = held_corpus(conn, model, &stats, db.as_deref(), at)?;
    corpus.penalize(&mut scored, seed_bpm);
    Ok(scored)
}

fn held_scores(
    conn: &Connection,
    track_id: i64,
    model: &str,
    db: Option<String>,
    at: Fingerprint,
) -> rusqlite::Result<Arc<Vec<(i64, f32)>>> {
    if let Some(db) = db.as_deref() {
        let held = SCORES_CACHE.lock().expect("score cache never poisons");
        if let Some(entry) = held
            .iter()
            .find(|e| e.seed == track_id && e.db == db && e.model == model && e.at == at)
        {
            return Ok(entry.scores.clone());
        }
    }
    let computed = Arc::new(compute_scores(conn, track_id, model, db.as_deref(), at)?);
    if let Some(db) = db {
        let mut held = SCORES_CACHE.lock().expect("score cache never poisons");
        held.retain(|e| !(e.seed == track_id && e.db == db && e.model == model));
        held.insert(
            0,
            CachedScores {
                db,
                model: model.to_owned(),
                seed: track_id,
                at,
                scores: computed.clone(),
            },
        );
        held.truncate(SCORE_SEEDS);
    }
    Ok(computed)
}

/// The seed is read from its row, since the stride may have left it out of the
/// corpus.
fn compute_scores(
    conn: &Connection,
    track_id: i64,
    model: &str,
    db: Option<&str>,
    at: Fingerprint,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    let Some(stats) = held_stats(conn, model, db, at)? else {
        return Ok(Vec::new());
    };
    let Some(raw) = vector(conn, track_id, model)? else {
        return Ok(Vec::new());
    };
    // Same width and finiteness checks as [`each_vector`].
    if raw.len() != stats.dim || raw.iter().any(|v| !v.is_finite()) {
        return Ok(Vec::new());
    }
    let corpus = held_corpus(conn, model, &stats, db, at)?;
    let mut z = Vec::with_capacity(stats.dim);
    stats.standardize(&raw, &mut z);
    let (seed, seed_inv) = quantize_seed(&z);
    Ok(corpus.scores(&seed, seed_inv, track_id))
}

/// The `k` nearest tracks by raw cosine, seed excluded. Playback draws from
/// [`nearest_ranked`].
pub fn nearest(
    conn: &Connection,
    track_id: i64,
    model: &str,
    k: usize,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    Ok(head(scores(conn, track_id, model)?, k))
}

pub fn nearest_ranked(
    conn: &Connection,
    track_id: i64,
    model: &str,
    k: usize,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    Ok(head(ranked(conn, track_id, model)?, k))
}

/// Ties break by id so the order is stable between calls.
fn head(mut scored: Vec<(i64, f32)>, k: usize) -> Vec<(i64, f32)> {
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(k);
    scored
}

/// f32 little-endian.
fn encode(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// A truncated blob reads as the floats it holds; the width check drops it.
fn decode(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::init_schema(&conn).unwrap();
        conn
    }

    /// Held answers are keyed by database file, so cache tests need one on disk.
    fn file_conn(name: &str) -> Connection {
        let dir = std::env::temp_dir().join(format!("rox-embeddings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::store::open(&dir.join("library.db")).unwrap();
        crate::store::init_schema(&conn).unwrap();
        conn
    }

    fn add_track(conn: &Connection, path: &str, duration_ms: u32) -> i64 {
        conn.execute(
            "INSERT INTO tracks (path, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
             VALUES (?1, 'T', 'A', 'Al', 'g', 0, 1, ?2, 0, 0)",
            rusqlite::params![path, duration_ms as i64],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Notes the write like the store's own writers do.
    fn tag_bpm(conn: &Connection, id: i64, bpm: f32) {
        conn.execute(
            "UPDATE tracks SET bpm = ?2 WHERE id = ?1",
            rusqlite::params![id, bpm],
        )
        .unwrap();
        note_write(conn);
    }

    #[test]
    fn tempo_distance_folds_octaves_and_ignores_what_it_cannot_measure() {
        let close = |a: f32, b: f32, want: f32| {
            let got = tempo_distance(a, b);
            assert!(
                (got - want).abs() < 0.005,
                "{a} against {b} came out {got}, wanted {want}"
            );
            assert!((got - tempo_distance(b, a)).abs() < 1e-6);
        };
        close(128.0, 128.0, 0.0);
        close(70.0, 140.0, 0.0);
        close(70.0, 280.0, 0.0);
        close(87.0, 174.0, 0.0);
        close(140.0, 175.0, 0.322);
        close(120.0, 128.0, 0.093);
        // The furthest two tempos get is half an octave, at 141.4.
        close(100.0, 150.0, 0.415);
        close(100.0, 141.4, 0.5);
        assert_eq!(tempo_distance(f32::NAN, 128.0), 0.0);
        assert_eq!(tempo_distance(128.0, f32::NAN), 0.0);
        assert_eq!(tempo_distance(0.0, 128.0), 0.0);
        assert_eq!(tempo_distance(-128.0, 128.0), 0.0);
    }

    /// Regression: a 70 BPM ambient seed drew a 133 BPM EBM track.
    #[test]
    fn a_near_double_costs_more_than_a_real_one() {
        let paid = |a: f32, b: f32| tempo_penalty(a, b);
        let same = paid(70.0, 70.0);
        let double = paid(70.0, 140.0);
        let ebm = paid(70.0, 133.0);
        let hundred = paid(70.0, 100.0);
        assert!(same < 0.001, "a shared tempo is free, paid {same}");
        assert!(
            paid(70.0, 72.0) < 0.02,
            "and so is a couple of BPM either side, paid {}",
            paid(70.0, 72.0)
        );
        assert!(
            (double - 0.06).abs() < 0.002,
            "a clean double pays the drift ceiling, paid {double}"
        );
        assert!(
            ebm > double + 0.01,
            "70 against 133 has to read as further off than 70 against 140, paid {ebm} against {double}"
        );
        assert!(
            ebm < hundred,
            "and still nearer than a tempo in the middle of the fold, paid {ebm} against {hundred}"
        );
        assert!((paid(133.0, 70.0) - ebm).abs() < 1e-6);
    }

    #[test]
    fn a_half_time_reading_stays_inside_a_neighbourhood() {
        let dnb = tempo_penalty(87.0, 174.0);
        assert!(
            (dnb - 0.06).abs() < 0.002,
            "87 against 174 pays the ceiling, paid {dnb}"
        );
        assert!(dnb < 0.095, "and less than a median band, paid {dnb}");
        // Two octaves out still pays only the ceiling.
        assert!((tempo_penalty(70.0, 280.0) - dnb).abs() < 0.002);
        assert!(tempo_penalty(40.0, 300.0) <= 0.21 + 0.002);
    }

    #[test]
    fn a_missing_tempo_pays_against_a_measured_one_and_not_another_missing() {
        for gap in [f32::NAN, 0.0, -128.0, f32::INFINITY] {
            assert_eq!(
                tempo_penalty(gap, 128.0),
                NO_TEMPO_PENALTY,
                "{gap} against 128"
            );
            assert_eq!(
                tempo_penalty(128.0, gap),
                NO_TEMPO_PENALTY,
                "128 against {gap}"
            );
            assert_eq!(tempo_penalty(gap, f32::NAN), 0.0, "{gap} against nothing");
        }
        assert!(
            NO_TEMPO_PENALTY > tempo_penalty(70.0, 140.0),
            "worse than a clean double"
        );
        assert!(
            NO_TEMPO_PENALTY < tempo_penalty(100.0, 282.8),
            "better than the worst mismatch"
        );
    }

    #[test]
    fn ranked_charges_each_candidate_for_the_tempo_it_runs_at() {
        let conn = conn();
        let mut rolls = Rolls(0x1234_5678_9ABC_DEF0);
        let mut ids = Vec::new();
        for i in 0..12 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            let raw: Vec<f32> = (0..32).map(|_| rolls.value()).collect();
            upsert(&conn, id, "m", &raw).unwrap();
            ids.push(id);
        }
        let seed = ids[0];
        let raw: HashMap<i64, f32> = scores(&conn, seed, "m").unwrap().into_iter().collect();
        let leader = nearest(&conn, seed, "m", 12).unwrap();
        let spread = leader[0].1 - leader[2].1;
        assert!(
            spread < TEMPO_WEIGHT * 0.4,
            "the fixture's candidates have to sit inside a tempo's charge, spread was {spread}"
        );

        // The seed at 140, the raw leader at 87.5 so the fold has something to undo,
        // the runner-up at double the seed. The rest are untempoed.
        tag_bpm(&conn, seed, 140.0);
        tag_bpm(&conn, leader[0].0, 87.5);
        tag_bpm(&conn, leader[1].0, 280.0);
        let with_tempo: HashMap<i64, f32> = ranked(&conn, seed, "m").unwrap().into_iter().collect();
        let charged = |id: i64| raw[&id] - with_tempo[&id];
        assert!(
            (charged(leader[0].0) - (TEMPO_WEIGHT * 0.322 + TEMPO_DRIFT_WEIGHT * 0.678)).abs()
                < 0.002,
            "the leader pays for a third of an octave folded and two thirds of one unfolded, paid {}",
            charged(leader[0].0)
        );
        assert!(
            (charged(leader[1].0) - TEMPO_DRIFT_WEIGHT).abs() < 0.002,
            "a double is the same tempo and still an octave away, paid {}",
            charged(leader[1].0)
        );
        for (id, _) in &leader[2..] {
            assert!(
                (charged(*id) - NO_TEMPO_PENALTY).abs() < 0.002,
                "an untempoed track pays the flat charge, paid {}",
                charged(*id)
            );
        }

        let ranked_order = nearest_ranked(&conn, seed, "m", 12).unwrap();
        assert_eq!(
            ranked_order[0].0, leader[1].0,
            "the lead goes to the nearest track that shares the tempo"
        );
        assert!(
            ranked_order.iter().position(|(id, _)| *id == leader[0].0) > Some(0),
            "and the track at the wrong tempo gave it up"
        );
        assert_eq!(ranked_order.len(), leader.len());
        assert_eq!(
            scores(&conn, seed, "m")
                .unwrap()
                .into_iter()
                .collect::<HashMap<_, _>>(),
            raw
        );
        assert_eq!(nearest(&conn, seed, "m", 12).unwrap(), leader);
    }

    #[test]
    fn an_untempoed_library_ranks_the_way_it_always_did() {
        let conn = conn();
        let points = [
            [1.0f32, 1.0],
            [0.9, 1.1],
            [-2.0, -1.5],
            [-1.0, 2.0],
            [2.0, -1.0],
            [-3.0, -3.0],
        ];
        let mut ids = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", p).unwrap();
            ids.push(id);
        }
        let raw = nearest(&conn, ids[0], "m", 6).unwrap();
        assert_eq!(nearest_ranked(&conn, ids[0], "m", 6).unwrap(), raw);

        // Every candidate pays the same flat charge, so the order holds.
        for (i, id) in ids.iter().enumerate().skip(1) {
            tag_bpm(&conn, *id, 80.0 + i as f32 * 20.0);
        }
        let flat = |got: Vec<(i64, f32)>| {
            assert_eq!(got.len(), raw.len());
            for ((id, score), (want_id, want_score)) in got.iter().zip(&raw) {
                assert_eq!(id, want_id, "the order held");
                assert!(
                    (want_score - score - NO_TEMPO_PENALTY).abs() < 1e-4,
                    "track {id} went from {want_score} to {score}"
                );
            }
        };
        flat(nearest_ranked(&conn, ids[0], "m", 6).unwrap());
        // A tempo nothing believes is no tempo.
        tag_bpm(&conn, ids[0], 9999.0);
        flat(nearest_ranked(&conn, ids[0], "m", 6).unwrap());
    }

    /// A tempo written onto `tracks` has to invalidate the held corpus.
    #[test]
    fn a_measured_tempo_puts_the_held_corpus_back_on_the_table() {
        let mut conn = file_conn("tempo");
        let points = [
            [1.0f32, 1.0],
            [0.9, 1.1],
            [-2.0, -1.5],
            [-1.0, 2.0],
            [2.0, -1.0],
            [-3.0, -3.0],
        ];
        let mut ids = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", p).unwrap();
            ids.push(id);
        }
        assert_eq!(
            crate::store::set_measured_bpm(&mut conn, &[("/m/0.mp3", 0, 140.0)]).unwrap(),
            1
        );
        let raw_order = nearest(&conn, ids[0], "m", 6).unwrap();
        let before = nearest_ranked(&conn, ids[0], "m", 6).unwrap();
        for ((id, score), (raw_id, raw_score)) in before.iter().zip(&raw_order) {
            assert_eq!(id, raw_id, "nothing to reorder yet");
            assert!((raw_score - score - NO_TEMPO_PENALTY).abs() < 1e-4);
        }

        let leader = before[0].0;
        let path = format!(
            "/m/{}.mp3",
            ids.iter().position(|id| *id == leader).unwrap()
        );
        assert_eq!(
            crate::store::set_measured_bpm(&mut conn, &[(&path, 0, 175.0)]).unwrap(),
            1
        );
        let after = nearest_ranked(&conn, ids[0], "m", 6).unwrap();
        let charged = raw_order[0].1 - after.iter().find(|(id, _)| *id == leader).unwrap().1;
        assert!(
            (charged - (TEMPO_WEIGHT + TEMPO_DRIFT_WEIGHT) * 0.322).abs() < 0.002,
            "the corpus was reread and the tempo charged, paid {charged}"
        );
        let raw: HashMap<i64, f32> = scores(&conn, ids[0], "m").unwrap().into_iter().collect();
        assert!((raw[&leader] - raw_order[0].1).abs() < 1e-6);
    }

    #[test]
    fn nothing_is_analyzed_until_a_model_has_written_something() {
        let conn = conn();
        assert!(
            !any(&conn, "m").unwrap(),
            "an empty table describes nothing"
        );
        let id = add_track(&conn, "/m/1.mp3", 200_000);
        upsert(&conn, id, "m", &[1.0, 2.0]).unwrap();
        assert!(any(&conn, "m").unwrap());
        assert!(!any(&conn, "other").unwrap());
    }

    #[test]
    fn a_stored_vector_comes_back_the_way_it_went_in() {
        let conn = conn();
        let id = add_track(&conn, "/m/1.mp3", 200_000);
        let vec = vec![0.5, -1.25, 0.0, 3.75];
        upsert(&conn, id, "m", &vec).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec.clone()));
        upsert(&conn, id, "m", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec![1.0; 4]));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "the composite key holds one row per model");
        assert_eq!(vector(&conn, id, "other").unwrap(), None);
    }

    #[test]
    fn a_vector_with_a_nan_never_reaches_the_table() {
        let conn = conn();
        let id = add_track(&conn, "/m/1.mp3", 200_000);
        upsert(&conn, id, "m", &[1.0, f32::NAN]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), None);
        assert!(!any(&conn, "m").unwrap(), "nothing was described");
        upsert(&conn, id, "m", &[1.0, f32::INFINITY]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), None);
        // A refusal leaves the existing row alone.
        upsert(&conn, id, "m", &[1.0, 2.0]).unwrap();
        upsert(&conn, id, "m", &[f32::NAN, 2.0]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec![1.0, 2.0]));
    }

    #[test]
    fn a_corpus_past_the_cap_is_sampled_evenly() {
        let conn = conn();
        for i in 0..1000 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", &[i as f32, -(i as f32)]).unwrap();
        }
        let rows = model_rows(&conn, "m").unwrap();
        assert_eq!(
            stride_from(rows, 5000),
            1,
            "under the cap every row is read"
        );
        let stride = stride_from(rows, 100);
        assert!(stride > 1, "past the cap the scan strides");
        let mut seen = Vec::new();
        each_vector(&conn, "m", 2, stride, |id, _, _| seen.push(id)).unwrap();
        assert!(
            !seen.is_empty() && seen.len() < 200,
            "the slice stays near the cap, got {}",
            seen.len()
        );
        // Spread across the library, not clustered at one end.
        let lo = *seen.iter().min().unwrap();
        let hi = *seen.iter().max().unwrap();
        assert!(
            lo < 150 && hi > 850,
            "the sample spans the library, got {lo}..{hi} over {} rows",
            seen.len()
        );
        let mut again = Vec::new();
        each_vector(&conn, "m", 2, stride, |id, _, _| again.push(id)).unwrap();
        assert_eq!(seen, again);
    }

    #[test]
    fn a_corpus_at_exactly_the_cap_is_still_read_whole() {
        let conn = conn();
        for i in 0..1000 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", &[i as f32, -(i as f32)]).unwrap();
        }
        let rows = model_rows(&conn, "m").unwrap();
        assert_eq!(stride_from(rows, 1000), 1);
        assert_eq!(stride_from(rows, 999), 2);
        assert_eq!(stride_from(rows, 500), 2, "and so is half");
        assert_eq!(stride_from(rows, 499), 3);
        assert_eq!(stride_from(model_rows(&conn, "none").unwrap(), 1000), 1);
        assert_eq!(stride_from(rows, 0), 1000);
    }

    #[test]
    fn one_poisoned_vector_does_not_take_the_corpus_with_it() {
        let conn = conn();
        let points = [
            [0.0f32, 0.0],
            [0.2, 0.1],
            [0.9, 0.8],
            [-0.7, 0.4],
            [0.4, -0.6],
            [-0.3, -0.9],
        ];
        let mut ids = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", p).unwrap();
            ids.push(id);
        }
        let clean = nearest(&conn, ids[0], "m", 5).unwrap();
        assert_eq!(clean.len(), 5);
        assert!(clean.iter().all(|(_, score)| score.is_finite()));

        let bad = add_track(&conn, "/m/bad.mp3", 200_000);
        upsert(&conn, bad, "m", &[f32::NAN, f32::INFINITY]).unwrap();
        let after = nearest(&conn, ids[0], "m", 5).unwrap();
        assert_eq!(after, clean, "the poisoned row changed nothing");
        assert!(
            after.iter().all(|(id, _)| *id != bad),
            "and it is not a neighbour of anything"
        );
        assert!(nearest(&conn, bad, "m", 5).unwrap().is_empty());
    }

    /// The raw statement below moves no row count and isn't a noted write, so the
    /// held answer comes back. The [`upsert`] after it changes nothing in the table
    /// and everything about the evidence. The final seed never had a map, so only
    /// a stale corpus could answer it wrong.
    #[test]
    fn a_held_answer_stands_until_a_write_says_otherwise() {
        let conn = file_conn("cache");
        let points = [
            [1.0f32, 1.0],
            [0.9, 1.1],
            [-2.0, -1.5],
            [-1.0, 2.0],
            [2.0, -1.0],
            [-3.0, -3.0],
        ];
        let mut ids = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", p).unwrap();
            ids.push(id);
        }
        let (seed, near, far) = (ids[0], ids[1], ids[5]);
        let first = nearest(&conn, seed, "m", 6).unwrap();
        assert_eq!(first[0].0, near, "the closest track leads");

        conn.execute(
            "UPDATE embeddings SET vec = ?1 WHERE track_id = ?2 AND model = 'm'",
            rusqlite::params![encode(&[1.0, 1.0]), far],
        )
        .unwrap();
        assert_eq!(
            nearest(&conn, seed, "m", 6).unwrap(),
            first,
            "nothing the fingerprint can see changed, so the held map answers"
        );

        upsert(&conn, far, "m", &[1.0, 1.0]).unwrap();
        let after = nearest(&conn, seed, "m", 6).unwrap();
        assert_eq!(
            after[0].0, far,
            "the write is evidence, and the reread finds the seed's own vector"
        );
        let other = nearest(&conn, near, "m", 6).unwrap();
        assert!(other.iter().all(|(id, _)| *id != near));
        assert_eq!(nearest(&conn, seed, "m", 6).unwrap(), after);

        // A seed nothing has asked about, so only the corpus can answer it.
        let fresh: HashMap<i64, f32> = scores(&conn, ids[3], "m").unwrap().into_iter().collect();
        assert_eq!(
            fresh[&far], fresh[&seed],
            "two identical vectors score identically, so the corpus was reread"
        );
    }

    /// A bell with a heavy tail, the shape a real embedding has.
    struct Rolls(u64);

    impl Rolls {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / 8388608.0 - 1.0
        }

        fn value(&mut self) -> f32 {
            let bell: f32 = (0..12).map(|_| self.next()).sum::<f32>() / 2.0;
            if self.next() > 0.95 {
                bell * 10.0
            } else {
                bell
            }
        }
    }

    /// One dimension spikes to fifteen sigma on two tracks. Without the clip those
    /// two are each other's nearest neighbour.
    #[test]
    fn a_spike_in_a_quiet_dimension_does_not_decide_the_neighbourhood() {
        let conn = conn();
        let (tracks, dim, quiet) = (500usize, 64usize, 5usize);
        let mut rolls = Rolls(0x9E37_79B9_7F4A_7C15);
        let mut ids = Vec::new();
        let mut rows: Vec<Vec<f32>> = Vec::new();
        for i in 0..tracks {
            let mut raw: Vec<f32> = (0..dim).map(|_| rolls.next()).collect();
            raw[quiet] = 0.0;
            rows.push(raw);
            ids.push(add_track(&conn, &format!("/m/{i}.mp3"), 200_000));
        }
        let (seed, stranger, twin) = (0usize, 1usize, 2usize);
        rows[seed][quiet] = 1.0;
        rows[stranger][quiet] = 1.0;
        let twin_row = {
            let mut r = rows[seed].clone();
            r[quiet] = 0.0;
            r
        };
        rows[twin] = twin_row;
        for (id, raw) in ids.iter().zip(&rows) {
            upsert(&conn, *id, "m", raw).unwrap();
        }
        let at = fingerprint(&conn, "m", None).unwrap();
        let stats = compute_stats(&conn, "m", at).unwrap().unwrap();
        let corpus = Corpus::build(&conn, "m", &stats, at.rows).unwrap();
        let mut z = Vec::new();
        stats.standardize(&rows[seed], &mut z);
        assert!(
            z[quiet].abs() <= Z_CLIP + 1e-3,
            "the spike is held at the clip, got {}",
            z[quiet]
        );
        let (cells, inv) = quantize_seed(&z);
        let mut scores = corpus.scores(&cells, inv, ids[seed]);
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        assert_eq!(
            scores[0].0, ids[twin],
            "the twin is the nearest, not the stranger sharing the spike"
        );
        let stranger_at = scores
            .iter()
            .position(|(id, _)| *id == ids[stranger])
            .unwrap();
        assert!(
            stranger_at > 10,
            "the stranger shares one clipped cell and nothing else, ranked {stranger_at}"
        );
    }

    /// Quantized scores hold to about a hundredth, and the top of the list keeps
    /// the same tracks.
    #[test]
    fn quantizing_the_corpus_keeps_the_score_and_the_order() {
        let conn = conn();
        let (tracks, dim) = (300usize, 512usize);
        let mut rolls = Rolls(0x2545_F491_4F6C_DD1D);
        let scale: Vec<f32> = (0..dim).map(|d| 10f32.powi(d as i32 % 5 - 2)).collect();
        let offset: Vec<f32> = (0..dim).map(|d| scale[d] * (d as f32 - 60.0)).collect();
        let mut ids = Vec::new();
        for i in 0..tracks {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            let raw: Vec<f32> = (0..dim)
                .map(|d| offset[d] + scale[d] * rolls.value())
                .collect();
            upsert(&conn, id, "m", &raw).unwrap();
            ids.push(id);
        }

        let at = fingerprint(&conn, "m", None).unwrap();
        let stats = compute_stats(&conn, "m", at).unwrap().unwrap();
        let corpus = Corpus::build(&conn, "m", &stats, at.rows).unwrap();
        assert_eq!(corpus.ids, ids, "every row is a candidate");

        let exact_rows: Vec<Vec<f32>> = ids
            .iter()
            .map(|id| {
                let raw = vector(&conn, *id, "m").unwrap().unwrap();
                let mut z = Vec::new();
                stats.standardize(&raw, &mut z);
                let len = z.iter().map(|v| v * v).sum::<f32>().sqrt();
                z.iter().map(|v| v / len).collect()
            })
            .collect();

        let mut worst = 0f32;
        let mut total = 0f64;
        let mut pairs = 0f64;
        for (si, seed_id) in ids.iter().enumerate().step_by(37) {
            let mut z = Vec::new();
            stats.standardize(&vector(&conn, *seed_id, "m").unwrap().unwrap(), &mut z);
            let (cells, inv) = quantize_seed(&z);
            let quantized = corpus.scores(&cells, inv, *seed_id);
            assert_eq!(quantized.len(), tracks - 1, "the seed is the only one out");

            let exact: Vec<(i64, f32)> = ids
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != si)
                .map(|(i, id)| {
                    let d = exact_rows[i]
                        .iter()
                        .zip(&exact_rows[si])
                        .map(|(a, b)| a * b)
                        .sum();
                    (*id, d)
                })
                .collect();
            let by_id: HashMap<i64, f32> = exact.iter().copied().collect();
            for (id, score) in &quantized {
                assert!(
                    (-1.0..=1.0).contains(score),
                    "a cosine that left its range: {score}"
                );
                let off = (score - by_id[id]).abs();
                worst = worst.max(off);
                total += off as f64;
                pairs += 1.0;
            }

            // Compared as sets: near-identical scores can swap order.
            let order = |mut v: Vec<(i64, f32)>| -> Vec<(i64, f32)> {
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                v.truncate(10);
                v
            };
            let want = order(exact);
            let got = order(quantized);
            // The same track leads unless the float answer had a dead heat at the top.
            if want[0].0 != got[0].0 {
                let gap = (want[0].1 - by_id[&got[0].0]).abs();
                assert!(
                    gap < 0.005,
                    "a different track leads on a real gap of {gap}"
                );
            }
            let ids = |v: &[(i64, f32)]| -> std::collections::HashSet<i64> {
                v.iter().map(|(id, _)| *id).collect()
            };
            let (want_ids, got_ids) = (ids(&want), ids(&got));
            // Anything that crossed the tenth place did it from a dead heat.
            let cut = want[9].1;
            for id in want_ids.symmetric_difference(&got_ids) {
                let gap = (by_id[id] - cut).abs();
                assert!(
                    gap < 0.005,
                    "track {id} changed places on a real gap of {gap}"
                );
            }
            assert!(
                want_ids.intersection(&got_ids).count() >= 9,
                "at most one track trades places in the ten"
            );
        }
        assert!(
            worst < 0.01,
            "no score drifts far enough to redraw the column, worst was {worst}"
        );
        assert!(
            total / pairs < 0.002,
            "and typically nowhere near it, mean was {}",
            total / pairs
        );
    }

    #[test]
    fn models_report_their_rows_and_their_width() {
        let conn = conn();
        assert!(models(&conn).unwrap().is_empty(), "nothing described yet");
        let a = add_track(&conn, "/m/a.mp3", 200_000);
        let b = add_track(&conn, "/m/b.mp3", 200_000);
        upsert(&conn, a, "panns", &vec![0.25; 512]).unwrap();
        upsert(&conn, b, "panns", &vec![0.25; 512]).unwrap();
        upsert(&conn, a, "old", &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(
            models(&conn).unwrap(),
            vec![
                ModelRows {
                    model: "old".into(),
                    rows: 1,
                    dim: 3
                },
                ModelRows {
                    model: "panns".into(),
                    rows: 2,
                    dim: 512
                },
            ]
        );
        conn.execute("UPDATE embeddings SET dim = 9 WHERE model = 'old'", [])
            .unwrap();
        assert_eq!(models(&conn).unwrap()[0].dim, 3);
    }

    #[test]
    fn clearing_a_model_shrinks_the_file() {
        let dir = std::env::temp_dir().join("rox-embeddings-vacuum");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A rollback journal rather than WAL, so the shrink shows in the file size
        // without waiting on a checkpoint.
        let path = dir.join("library.db");
        let conn = Connection::open(&path).unwrap();
        crate::store::init_schema(&conn).unwrap();
        for i in 0..500 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", &vec![i as f32; 512]).unwrap();
        }
        let size = || std::fs::metadata(&path).unwrap().len();
        let full = size();
        assert!(full > 1_000_000, "half a megabyte of vectors at least");

        assert_eq!(clear(&conn, "m").unwrap(), 500);
        assert!(!any(&conn, "m").unwrap());
        assert!(
            size() < full / 2,
            "the file gave its pages back, {} against {full}",
            size()
        );
        let free: i64 = conn
            .pragma_query_value(None, "freelist_count", |r| r.get(0))
            .unwrap();
        assert_eq!(free, 0, "and holds no free pages after the vacuum");
        let tracks: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tracks, 500);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_lists_what_a_model_has_not_covered() {
        let conn = conn();
        let a = add_track(&conn, "/m/a.mp3", 200_000);
        let b = add_track(&conn, "/m/b.mp3", 200_000);
        add_track(&conn, "/m/c.mp3", 0);
        assert_eq!(
            missing(&conn, "m").unwrap(),
            vec![
                Pending {
                    id: a,
                    path: "/m/a.mp3".into(),
                    duration_ms: 200_000,
                    sub: 0
                },
                Pending {
                    id: b,
                    path: "/m/b.mp3".into(),
                    duration_ms: 200_000,
                    sub: 0
                },
            ]
        );
        upsert(&conn, a, "m", &[1.0, 2.0]).unwrap();
        assert_eq!(
            missing(&conn, "m")
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![b]
        );
        assert_eq!(missing(&conn, "other").unwrap().len(), 2);
        assert_eq!(
            coverage(&conn, "m").unwrap(),
            Coverage {
                embedded: 1,
                total: 2
            }
        );
        assert_eq!(coverage(&conn, "m").unwrap().missing(), 1);

        conn.execute(
            "INSERT INTO tracks (source, path, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
             VALUES ('stream', 'rox://1', 'T', 'A', 'Al', 'g', 0, 1, 200000, 0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            missing(&conn, "m")
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![b]
        );
        assert_eq!(coverage(&conn, "m").unwrap().total, 2);

        conn.execute(
            "INSERT INTO tracks (path, sub, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
             VALUES ('/m/disc.flac', 4, 'T', 'A', 'Al', 'g', 0, 4, 200000, 0, 0)",
            [],
        )
        .unwrap();
        let pending = missing(&conn, "m").unwrap();
        assert_eq!(
            pending.iter().map(|p| p.sub).collect::<Vec<_>>(),
            vec![0, 4]
        );
    }

    #[test]
    fn nearest_orders_by_similarity_and_leaves_the_seed_out() {
        let conn = conn();
        let seed = add_track(&conn, "/m/seed.mp3", 200_000);
        let near = add_track(&conn, "/m/near.mp3", 200_000);
        let far = add_track(&conn, "/m/far.mp3", 200_000);
        upsert(&conn, seed, "m", &[1.0, 1.0, 0.0]).unwrap();
        upsert(&conn, near, "m", &[0.9, 1.1, 0.0]).unwrap();
        upsert(&conn, far, "m", &[-2.0, -1.5, 0.0]).unwrap();
        let hits = nearest(&conn, seed, "m", 10).unwrap();
        assert_eq!(
            hits.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![near, far],
            "the seed is excluded and the closer track leads"
        );
        assert!(hits[0].1 > hits[1].1);
        assert_eq!(nearest(&conn, seed, "m", 1).unwrap().len(), 1);
        let bare = add_track(&conn, "/m/bare.mp3", 200_000);
        assert!(nearest(&conn, bare, "m", 5).unwrap().is_empty());
    }

    /// A constant dimension changes no neighbours, however large it is.
    #[test]
    fn a_constant_dimension_does_not_change_the_ordering() {
        let conn = conn();
        let points = [
            [0.0f32, 0.0],
            [0.2, 0.1],
            [0.9, 0.8],
            [-0.7, 0.4],
            [0.4, -0.6],
            [-0.3, -0.9],
        ];
        let mut ids = Vec::new();
        for (i, p) in points.iter().enumerate() {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "plain", p).unwrap();
            upsert(&conn, id, "padded", &[p[0], p[1], 500.0]).unwrap();
            ids.push(id);
        }
        let order = |model| -> Vec<i64> {
            nearest(&conn, ids[0], model, 5)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        };
        let plain = order("plain");
        assert_eq!(plain.len(), 5, "every other track is a candidate");
        assert_eq!(plain, order("padded"));
    }

    #[test]
    fn models_and_orphans_stay_out_of_each_others_way() {
        let conn = conn();
        let a = add_track(&conn, "/m/a.mp3", 200_000);
        let b = add_track(&conn, "/m/b.mp3", 200_000);
        upsert(&conn, a, "one", &[1.0, 0.0]).unwrap();
        upsert(&conn, b, "one", &[0.0, 1.0]).unwrap();
        upsert(&conn, a, "two", &[1.0, 0.0]).unwrap();
        assert_eq!(nearest(&conn, a, "one", 5).unwrap().len(), 1);
        assert!(nearest(&conn, a, "two", 5).unwrap().is_empty());
        assert_eq!(clear(&conn, "two").unwrap(), 1);
        assert_eq!(nearest(&conn, a, "one", 5).unwrap().len(), 1);

        // Foreign keys default on in this build, so the cascade leaves nothing to prune.
        conn.execute("DELETE FROM tracks WHERE id = ?1", [b])
            .unwrap();
        assert!(nearest(&conn, a, "one", 5).unwrap().is_empty());
        assert_eq!(prune(&conn).unwrap(), 0, "the cascade already took it");

        // With enforcement off, the row outlives its track. The joins hide it and the
        // prune clears it.
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        let orphan = add_track(&conn, "/m/c.mp3", 200_000);
        upsert(&conn, orphan, "one", &[1.0, 1.0]).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = ?1", [orphan])
            .unwrap();
        assert!(nearest(&conn, a, "one", 5).unwrap().is_empty());
        assert_eq!(prune(&conn).unwrap(), 1);
        assert_eq!(prune(&conn).unwrap(), 0);
    }
}
