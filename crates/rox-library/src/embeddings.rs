//! Acoustic embeddings: one feature vector per track per model, and the
//! nearest-neighbour query over them. What produces the vectors is the app's
//! business (`rox/src/embeddings.rs` runs the DSP extractor); this module
//! only stores them and answers "what sounds like this".
//!
//! Rows are keyed by model name rather than a version number, so a library
//! can hold two models' worth at once and switching between them costs
//! nothing. An extractor whose output changes meaning takes a new name, and
//! the old vectors stay until something clears them.
//!
//! Vectors go in raw, unnormalized. Standardizing belongs to the query
//! ([`nearest`] z-scores each dimension against the corpus before comparing),
//! because raw feature statistics span wildly different scales: a band
//! energy in the tens and an onset rate near one, compared by cosine, means
//! the loud dimensions decide every neighbour and the rest are decoration.
//! Doing it at read time also means the weighting can be retuned without
//! re-extracting the library.
//!
//! Reading is expensive and the table barely moves, so the corpus is read
//! once and kept: the standardization for a model, the standardized vectors
//! themselves quantized down to a byte a dimension, and the score maps for the
//! last few seeds, all against evidence that the table is still the one they
//! were computed from (see [`Fingerprint`]). A query that arrives on an
//! unchanged table never touches the vector blobs at all.
//!
//! Sound isn't the whole of what makes two tracks play well together, so the
//! ranking playback draws from is the cosine with a tempo penalty on it (see
//! [`ranked`]). The tempo is loaded into the corpus beside the vectors, off
//! the join that was already there. [`scores`] stays the raw cosine for the
//! column that prints it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::Connection;

/// The table beside the tracks it describes. Composite key, so one track
/// has a row per model it has been through.
///
/// The cascade does fire: the bundled SQLite is compiled with foreign keys
/// defaulted on, so dropping a track drops its vectors. That's a build flag
/// rather than something the store asks for, though, and it's per-connection,
/// so the reads here join against `tracks` anyway and [`prune`] exists for a
/// database that was written without it. Belt and braces on a table nothing
/// else would notice going stale.
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

/// A track waiting on a vector: what the extractor needs to open it and
/// pick its sample windows.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: i64,
    pub path: String,
    pub duration_ms: u32,
    /// Which slice of the file this track is: 0 for a file that is its own
    /// track, higher for a cue subsong. The vector is per track either way,
    /// but the file underneath is shared, so a pass saving to tags has to
    /// know: writing one subsong's vector into the image would claim the
    /// whole disc sounds like track four. Only sub 0 is ever offered a tag
    /// (see [`crate::writer::writes_to_file`]).
    pub sub: u16,
}

/// How much of the library this model has covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub embedded: usize,
    pub total: usize,
}

impl Coverage {
    /// Tracks this model still owes a vector.
    pub fn missing(self) -> usize {
        self.total.saturating_sub(self.embedded)
    }
}

/// Every track with no vector for `model`, in id order. Zero-duration rows
/// are skipped: the extractor picks its sample windows as fractions of the
/// running time, and a track that claims none has nothing to aim at. Local
/// rows only, the same bound the rest of the store's work lists draw: the
/// extractor opens a path with a decoder, and a streaming source's row
/// has nothing it could open.
///
/// Cue subsongs are in the list: a span of an image decodes fine and gets a
/// vector like anything else. What their `sub` is for is the tag write,
/// which a shared file can't take (see [`Pending::sub`]).
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

/// How many tracks this model has vectors for, against how many there are
/// to cover. Both sides count only the rows [`missing`] would offer, so the
/// two numbers converge on a finished pass.
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

/// Whether this model has described anything at all.
///
/// [`coverage`] gives the same answer and more, at the cost of counting
/// two tables; this stops at the first row, so it's cheap enough for the
/// paths that only need to know whether ranking by sound can return
/// anything: a mode that would quietly do nothing is worth refusing, and
/// refusing it is a decision drawn on every menu that offers it.
pub fn any(conn: &Connection, model: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM embeddings WHERE model = ?1)",
        [model],
        |r| r.get(0),
    )
}

/// Store one track's vector, replacing whatever this model had for it.
///
/// A vector with a NaN or an infinity is refused rather than written.
/// One of them is enough to make every score in the library NaN, because
/// the standardization takes its mean from the corpus and hands the poison
/// to every vector it touches, and the row would go on doing that to every
/// query until somebody deleted it by hand. The caller isn't told: the read
/// side skips such a row anyway, this is the store declining to hold one at
/// all, and failing a whole batch over a single bad track would throw away
/// the good work beside it. Whoever produced the vector can name the file,
/// so the app-side guard is where a listener hears about it.
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

/// A batch of vectors in one transaction, which is how the extraction pass
/// writes: a few dozen tracks of decoding per commit rather than a commit
/// per track.
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

/// What one model holds in the table, for a page that lists what the
/// library is spending its disk on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRows {
    pub model: String,
    /// Tracks this model has described.
    pub rows: u64,
    /// How wide its vectors are, measured off a stored blob rather than
    /// read out of the `dim` column, so a row whose width and column
    /// disagree is reported as what it actually costs. The widest of the
    /// model's rows, which is the only one there is unless an extractor
    /// changed shape without changing name.
    pub dim: usize,
}

/// Every model with vectors in the table, alphabetically. Includes models
/// nothing in this build knows about: a renamed extractor leaves its old
/// rows behind, and being able to see them is the point of listing.
pub fn models(conn: &Connection) -> rusqlite::Result<Vec<ModelRows>> {
    let mut stmt = conn.prepare(
        "SELECT model, COUNT(*), COALESCE(MAX(length(vec)), 0) FROM embeddings
         GROUP BY model ORDER BY model",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ModelRows {
            model: r.get(0)?,
            rows: r.get::<_, i64>(1)? as u64,
            // Four bytes a value, the shape [`encode`] writes.
            dim: r.get::<_, i64>(2)? as usize / 4,
        })
    })?;
    rows.collect()
}

/// Drop every vector for a model, for when its extractor changed under the
/// same name and the library needs redoing, or when a library wants its
/// disk back more than it wants "sounds like this".
///
/// The pages go back to the filesystem rather than staying free inside the
/// file, the same move [`crate::thumbs::clear`] makes: the vectors are most
/// of what a described library weighs, so a clear that left the file the
/// same size would read as having done nothing. That rewrite is why this
/// belongs off a UI thread, and why it can't run inside a transaction.
/// A database another connection is busy reading can refuse it; the rows
/// are gone either way, which is what the caller asked for, so a refusal is
/// a warning rather than an error.
pub fn clear(conn: &Connection, model: &str) -> rusqlite::Result<usize> {
    let dropped = conn.execute("DELETE FROM embeddings WHERE model = ?1", [model])?;
    note_write(conn);
    if let Err(e) = conn.execute_batch("VACUUM;") {
        log::warn!("embeddings: cleared {model} but the file kept its pages: {e}");
    }
    Ok(dropped)
}

/// Drop vectors whose track is gone. A no-op while the cascade is doing its
/// job (see [`init_schema`]), and the cleanup for a database whose writer
/// had foreign keys off.
pub fn prune(conn: &Connection) -> rusqlite::Result<usize> {
    let dropped = conn.execute(
        "DELETE FROM embeddings WHERE track_id NOT IN (SELECT id FROM tracks)",
        [],
    )?;
    note_write(conn);
    Ok(dropped)
}

/// How much of the corpus is read into memory before it starts sampling.
/// Above this the scan takes an evenly spaced slice instead of every row,
/// which keeps a huge library's "more like this" bounded rather than linear.
/// It bounds two things now: the one scan that fills [`Corpus`], and what
/// that corpus then costs to hold, a byte per dimension per candidate. Under
/// the model rox ships that ceiling is a hundred megabytes for a library four
/// times the size of any real one, and a fifty-thousand-track library comes to
/// twenty-five.
pub const CANDIDATE_CAP: usize = 200_000;

/// Rows the standardization statistics are estimated from. A mean and a
/// standard deviation converge long before the whole corpus is counted, so
/// there's nothing to gain from reading a million vectors to find them.
const STATS_SAMPLE: usize = 20_000;

/// How many sigmas out a standardized cell is allowed to sit. Anything past
/// this is pulled back to it, either side of zero.
///
/// The z-scores under the model rox ships are heavy tailed in a particular
/// way: a handful of dimensions barely move across the whole library and
/// then, on one track in a few thousand, jump to fifty or a hundred and fifty
/// sigma. Left alone, one such cell is the whole row. Measured over the live
/// library, a track carrying one has a standardized length of sixty to a
/// hundred and fifty against a median of seventeen, with nineteen parts in
/// twenty of that length in two cells, and its cosine against anything is
/// then a question of whether the other track spikes in the same place. The
/// symptom is a neighbourhood of thirty tracks that share nothing but the
/// spike, intros and interludes beside power metal beside a French house
/// track, and the library's real neighbours nowhere in it. One track in eight
/// carries a cell past ten sigma, so it's not a corner case.
///
/// Four, because that's where a real reading ends and a spike begins. Half a
/// track's rows have at least one cell past it, so the clip touches a lot of
/// rows and almost nothing in any of them: the ordinary cells, which are what
/// the cosine should be reading, sit well inside it and come through
/// untouched. Anything past four sigma in a dimension that never moves is a
/// yes-or-no fact about the track, and a yes counts for four sigma rather
/// than a hundred. Prototyped against the same live library at three, four,
/// a signed log and a tanh: all four hand the seed above a neighbourhood of
/// the synth and electronic tracks it belongs with, and they agree on it to
/// within a place or two, so the choice among them is the simplest one.
const Z_CLIP: f32 = 4.0;

/// Per-dimension standardization for one model's corpus: what to subtract
/// and what to divide by before two vectors can be compared. See the module
/// header for why this isn't baked into the stored vectors.
#[derive(Clone, Debug)]
pub struct Stats {
    dim: usize,
    mean: Vec<f32>,
    /// Reciprocal standard deviation, so applying it is a multiply. Zero for
    /// a dimension that never varies: it says nothing about neighbours, and
    /// dividing by its spread would be dividing by nothing.
    inv_std: Vec<f32>,
}

impl Stats {
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Standardize a raw vector into `out`: every dimension in sigmas, none of
    /// them louder than another for having been measured in a bigger unit,
    /// and none of them past [`Z_CLIP`] of them, so a tail can't be louder
    /// than the rest of the row put together.
    ///
    /// Length is left alone here rather than scaled to one. Comparing two of
    /// these is still a cosine, and the division that makes it one is folded
    /// into [`Corpus`], which does it once per row against the bytes it
    /// actually scores.
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

/// The width most of a model's vectors share. An extractor that changed
/// shape without changing name leaves two widths in one table; the majority
/// is the live one and the leftovers take no part.
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

/// How many vectors a model holds, the number the stride is drawn from and
/// half the evidence a cached answer is checked against.
fn model_rows(conn: &Connection, model: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM embeddings WHERE model = ?1",
        [model],
        |r| r.get(0),
    )
}

/// Take every `stride`-th row of a `total`-row corpus, picked so no more than
/// `cap` come back. One means read every row, which is the answer for any
/// library that fits under the cap.
///
/// A plain stride over a sequential scan, which reads more than it scores.
/// That's deliberate, and it's measured: making the read itself selective
/// (an index on a hashed bucket, or a union of contiguous id windows) forces
/// SQLite to fetch each vector by rowid, and scattered fetches cost several
/// times what reading the table straight through does. The cap bounds the
/// decoding and the arithmetic, the parts that grow without one; the scan
/// underneath stays linear and cheap per row.
///
/// Deterministic rather than random: the same query twice gives the same
/// numbers, which a column showing a score needs. Variety for a radio that
/// wants a different answer each time belongs to whatever picks the seed
/// track, not here.
fn stride_from(total: i64, cap: usize) -> i64 {
    // Ceiling division, so a corpus of exactly the cap is still read whole
    // and one row past it is the first to stride. Adding one to a plain
    // division instead strides by two at every exact multiple of the cap,
    // which scores half a corpus the cap says to read all of.
    let total = total.max(0) as usize;
    total.div_ceil(cap.max(1)).max(1) as i64
}

/// Iterate over a model's vectors, handing each to `visit` in turn with the
/// tempo the row it belongs to claims. The vector is borrowed from a buffer
/// reused across rows, so a million-track scan holds one vector at a time
/// rather than a million of them.
///
/// The tempo comes off the join that was already there for the track's
/// existence, so it costs one more column rather than a second pass. None for
/// a track nothing has measured or tagged, and for a number outside what
/// [`crate::tempo`] will believe: a row claiming 0 or 9999 is a tagger's
/// filler, and folding it into octaves would put it a long way from music.
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
        // A blob of the wrong width can't be compared against the rest, and
        // neither can one holding a NaN or an infinity: it wouldn't merely
        // score badly, it would take the whole corpus with it. One NaN in a
        // dimension makes that dimension's mean NaN, the variance clamp
        // reads NaN as zero (f64::max ignores it), and every vector
        // standardized against those statistics comes out NaN. Every score
        // in the library then ties, and "nearest" quietly means "lowest id".
        if buf.len() == dim && buf.iter().all(|v| v.is_finite()) {
            visit(id, &buf, bpm);
        }
    }
    Ok(())
}

/// A stored tempo the ranking will act on, None for everything else. The
/// column is nullable and holds whatever a tagger wrote, so this is the one
/// place a number turns into a tempo: unread rows, unset zeros and the odd
/// beat period all read as "no tempo" and earn no penalty rather than a
/// wrong one.
fn believable_bpm(bpm: Option<f64>) -> Option<f32> {
    bpm.map(|b| b as f32)
        .filter(|b| (crate::tempo::SLOWEST..=crate::tempo::FASTEST).contains(b))
}

/// One track's stored tempo, for the seed of a ranking. A point query against
/// the primary key, read beside the seed's vector for the same reason that one
/// is: a seed the corpus stride left out still deserves an answer, and the
/// corpus only holds the rows it kept.
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

/// Evidence that the corpus a held answer came from is still the corpus in
/// the table. Cheap enough to take on every query: a pragma and a counted
/// index scan, against a scan of every vector behind it.
///
/// Three signals, because no one of them sees everything. `data_version` is
/// SQLite's own change counter, which moves when another connection commits,
/// but only for a connection that stays open long enough to compare two
/// readings; every caller here opens a fresh one per query, so on its own it
/// says nothing. `rows` catches the analysis pass filling the table, a
/// [`clear`], and a scan dropping tracks the cascade takes vectors with,
/// whoever wrote them. `writes` catches this process replacing a vector in
/// place, which leaves the count exactly where it was, and the tempo pass
/// writing a bpm onto `tracks` for the same reason: the corpus holds every
/// row's tempo, so a measurement that never touched the embeddings table
/// still changes what a ranking returns. [`crate::store::set_measured_bpm`]
/// counts each row it takes, which puts the held corpus and every held score
/// map back on the table.
///
/// What's left over is another process rewriting vectors without changing how
/// many there are: a second rox re-analyzing the same library under the same
/// model. That goes unseen until the count moves, which is the staleness this
/// design accepts.
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

/// Which database an answer belongs to, and whether it can be held at all.
/// None for a connection with no file behind it: an in-memory database is a
/// fixture or a scratch, two of them are never the same corpus, and they
/// have no path to tell apart, so they get recomputed fresh every time rather
/// than out of each other's rows.
fn db_key(conn: &Connection) -> Option<String> {
    conn.path().filter(|p| !p.is_empty()).map(str::to_owned)
}

/// Writes this process has made to each database's acoustic side. Module
/// state the way [`crate::genre`] holds its alias map, and per database
/// rather than one counter, so a scratch library being written doesn't throw
/// away what the one on screen already knows.
static WRITES: RwLock<Option<HashMap<String, u64>>> = RwLock::new(None);

/// Count one row written, whatever it did to the row count. Crate-visible
/// because the tempo pass writes onto `tracks` rather than here (see
/// [`crate::store::set_measured_bpm`]) and still moves what the acoustic
/// side describes a track by.
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

/// The last standardization computed, with the corpus it describes. One
/// entry: a library can hold vectors under two models, but only one of them
/// is the pick every caller scores against, and changing the pick pays for
/// one recompute.
static STATS_CACHE: RwLock<Option<CachedStats>> = RwLock::new(None);

struct CachedStats {
    db: String,
    model: String,
    at: Fingerprint,
    /// The None a model with nothing to standardize returns is worth
    /// holding too: it costs the same scan to find out.
    stats: Option<Arc<Stats>>,
}

/// Score maps for the last few seeds. Small enough to keep several: a map is
/// twelve bytes a track, so the whole cache is a couple of megabytes even on
/// a library at [`CANDIDATE_CAP`]. Several rather than one because the
/// questions arrive in clusters: the transport draws a similar track, the
/// library's Similar column then asks about that same seed, and a queue being
/// ordered by sound comes between the two.
///
/// The [`Corpus`] left it standing on a smaller argument than it was built on.
/// A fresh seed scored off the held corpus is six milliseconds rather than the
/// couple of hundred it used to be, so this no longer stands between the app
/// and a stall. What it saves now is three callers asking about one track and
/// doing the same arithmetic three times, which is still worth two megabytes.
static SCORES_CACHE: Mutex<Vec<CachedScores>> = Mutex::new(Vec::new());

/// How many seeds' scores are held, most recently asked first.
const SCORE_SEEDS: usize = 4;

struct CachedScores {
    db: String,
    model: String,
    seed: i64,
    at: Fingerprint,
    scores: Arc<Vec<(i64, f32)>>,
}

/// The standardized corpus one model's queries are answered from, read from
/// the table once and held for as long as the table stands still.
static CORPUS_CACHE: RwLock<Option<CachedCorpus>> = RwLock::new(None);

struct CachedCorpus {
    db: String,
    model: String,
    at: Fingerprint,
    corpus: Arc<Corpus>,
}

/// The largest magnitude a cell can hold. The signed byte's spare value at
/// -128 goes unused, so the scale is symmetric and a vector and its opposite
/// quantize to opposite cells.
const QUANT_PEAK: f32 = 127.0;

/// Under this a standardized row is the corpus mean to within nothing, and
/// whatever direction it appears to point is rounding rather than a
/// description. Small enough that no real vector meets it, large enough that
/// dividing by it stays inside f32.
const QUANT_FLOOR: f32 = 1e-20;

/// How far apart two tempos are, in octaves, folded so a track and its own
/// double count as the same tempo. Zero for a match and for 70 against 140,
/// a third for 140 against 175, a half at the furthest two tempos can be
/// from each other, which is a tempo and one and a half times it.
///
/// Folding is the whole point. Half time and double time are the same music
/// counted differently, and something has to count: taggers disagree with each
/// other about which one a track is, and an estimator picks whichever the
/// track's strongest periodicity is (see `rox_acoustic`), so an unfolded
/// distance would file a drum and bass track at 174 and the same one written
/// down as 87 in different worlds. Log space rather than a difference in beats
/// per minute, because tempo is heard as a ratio: 60 against 70 is a different
/// piece of music, 160 against 170 is the same one played tight.
///
/// Zero for a tempo either side is missing; what a missing tempo costs is
/// [`tempo_penalty`]'s call, not this one's. The NaN the corpus stands a
/// missing tempo up as goes down that branch, and so does anything else a
/// ratio and a logarithm have no answer for.
///
/// This is half of what a candidate pays now. Folding this hard means a pair
/// that's nearly a double reads as nearly a match, whatever the two numbers
/// are, so [`tempo_penalty`] charges the octaves the fold dropped as well.
fn tempo_distance(a: f32, b: f32) -> f32 {
    let Some(octaves) = tempo_octaves(a, b) else {
        return 0.0;
    };
    (octaves - octaves.round()).abs()
}

/// How far apart two tempos are in octaves before anything is folded, None
/// for a pair a ratio and a logarithm have no answer for. The one place a
/// tempo pair turns into a number, so the two things charged for it,
/// [`tempo_distance`] and the drift term in [`tempo_penalty`], agree on what
/// counts as measurable.
fn tempo_octaves(a: f32, b: f32) -> Option<f32> {
    let measured = |bpm: f32| bpm.is_finite() && bpm > 0.0;
    (measured(a) && measured(b)).then(|| (a / b).log2().abs())
}

/// The whole cosine a candidate at `b` is charged against a seed at `a`: the
/// folded distance at [`TEMPO_WEIGHT`], plus the octaves the fold threw away
/// at [`TEMPO_DRIFT_WEIGHT`]. Two missing tempos charge nothing, so an
/// unmeasured library still ranks on cosine alone; one missing against one
/// measured charges [`NO_TEMPO_PENALTY`], since those two tracks disagree
/// about something real.
///
/// Two terms because folding on its own has a hole in it, and the hole is
/// wide. Fold 70 against 133 and you get 0.074 of an octave, less than a
/// twentieth of a cosine at the folded weight, so 70 BPM ambient and 133 BPM
/// EBM read as very nearly the same tempo: a ratio of 1.9 is close enough to
/// a double that the fold treats the pair as the same music counted
/// differently. It isn't. Meanwhile 70 against 100, a gap you could actually
/// beatmatch through, pays 0.145. The drift term prices what the fold is
/// deliberately blind to, which is how many octaves apart the two numbers
/// literally are, and it does it gently enough that the fold still wins where
/// the fold is right.
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

/// The cosine a candidate is charged per octave of [`tempo_distance`] from
/// the seed. A gap tops out at half an octave, so the most any pair can be
/// charged by this term is half this; the drift term in [`tempo_penalty`]
/// adds its own share on top.
///
/// Calibrated against the live library, fifty thousand tracks under the model
/// rox ships, over eight seeds spaced evenly through it. The scale worth
/// matching is how tightly a neighbourhood is packed: across those seeds a
/// raw top thirty runs from its leader to its thirtieth in 0.095 of cosine at
/// the median (0.042 in the tightest neighbourhood measured, 0.296 in the
/// loosest), with a couple of thousandths between one place and the next. At
/// this weight the 0.3 octaves that separate two genuinely different tempos,
/// 140 against 175, costs 0.09: about the whole band, so a track at the wrong
/// tempo gives up the neighbourhood it was in, and a track a real timbral
/// distance ahead keeps its place anyway.
///
/// Charging a fixed cosine rather than a number of places makes that
/// behave. In a tight neighbourhood, where the vectors barely tell the
/// candidates apart, 0.09 moves a track hundreds of places and tempo decides
/// between tracks that all sound alike; in a loose one, where the seed has few
/// real neighbours, the same 0.09 is a place or two and timbre still picks.
/// Measured on a re-ranking of those bands against synthetic tempos, half the
/// raw top thirty stays in it, and the share of the band sharing the seed's
/// tempo goes from a third, the share chance gives, to four in five.
const TEMPO_WEIGHT: f32 = 0.3;

/// The cosine a candidate is charged per octave it sits from the seed before
/// the fold, on top of what [`TEMPO_WEIGHT`] charges for the folded gap.
///
/// A fifth of the folded weight, which is the ratio that makes the three
/// cases behave. Off a 70 BPM seed: 72 pays 0.015 and is still a match, 140
/// pays the ceiling at 0.060, 133 pays 0.078, and 100 pays 0.176. That's the
/// ordering the folded weight alone got wrong. 133 against a 70 is a ratio of
/// 1.9, which the fold reads as 0.074 of an octave and charges 0.022 for,
/// close enough to free that a track at 0.52 raw cosine got drawn off an
/// ambient seed with three hundred closer tracks in the library. It now pays
/// more than a clean double does and lands where it belongs, well outside the
/// seed's tempo without being thrown out of the library.
///
/// Sized against the neighbourhood spread [`TEMPO_WEIGHT`] was calibrated on,
/// 0.095 of cosine from leader to thirtieth at the median. A clean half or
/// double costs 0.060, about two thirds of that band: enough to move a track
/// down it, not enough to eject it, which is what half time deserves. It's a
/// real gap and not a wrong reading, so it shouldn't be free either. Be clear
/// about what this is: reasoned from the spread that comment measured, not a
/// second run of the fifty-thousand-track calibration behind it.
const TEMPO_DRIFT_WEIGHT: f32 = 0.06;

/// What a track with no tempo pays against a seed that has one, and the
/// other way round.
///
/// No tempo is a description too. The pass listens to every track and
/// refuses the ones it can't hear a beat in, which on a real library is the
/// classical, the ambient and the film score: about one track in six. A
/// seed running at 128 and a candidate the pass couldn't count are not
/// agreeing about tempo, they're disagreeing about whether there is one,
/// and charging nothing for that handed every beatless track a free pass
/// over the measured ones. Two beatless tracks still pay nothing, since
/// they agree, and that's the case that keeps a Debussy seed ranking its
/// own shelf first.
///
/// Twice a clean double and a little over half the most a measured pair
/// can pay: a real mismatch, short of the worst one. Enough that a 128 BPM
/// seed's band is measured tracks near 128 rather than whatever the pass
/// couldn't count, not so much that a beatless track a real timbral
/// distance ahead loses its place to a metronome.
const NO_TEMPO_PENALTY: f32 = 0.12;

/// Where the drift term stops counting octaves. Past a full octave the pair
/// is as far apart as tempo can say, and the fold residue is already pricing
/// which side of the octave the candidate landed on, so a quadruple isn't
/// charged twice a double. It also keeps the worst any pair can pay to 0.21,
/// twice the median neighbourhood and not much more: 40 against 300 is nearly
/// three octaves, and charging it 0.32 would mean tempo alone deciding
/// rankings that timbre should still have a say in.
const DRIFT_CEILING: f32 = 1.0;

/// One model's candidates, standardized once and kept as bytes.
///
/// A byte a dimension rather than the four a float takes. Each row is scaled
/// by its own largest magnitude, so 127 always means "this row's loudest
/// dimension" and every other cell is a fraction of it. A fixed step for the
/// whole corpus was tried first, on the grounds that [`Stats::standardize`]
/// already put every dimension in sigmas, and lost to this on the live
/// library: a row whose loudest cell is a fraction of a sigma would spend
/// most of the byte's range on nothing. Now that standardizing also clips to
/// [`Z_CLIP`], the loudest cell is never past four sigma either, so per-row
/// scaling never gets coarser than thirty steps a sigma and the score holds
/// within a thousandth of the float answer.
///
/// Nothing needs storing for that scale. The length each row is divided by is
/// measured from the bytes rather than from the floats they came from, so the
/// per-row factor cancels out of the score exactly, and every answer stays
/// inside -1..=1 the way a cosine does.
struct Corpus {
    dim: usize,
    ids: Vec<i64>,
    /// Row major, `dim` cells a track, in step with `ids`.
    cells: Vec<i8>,
    /// Reciprocal length of each row as quantized, in step with `ids` too.
    inv_norm: Vec<f32>,
    /// What each row runs at in beats a minute, in step with `ids`, NaN for a
    /// track with no tempo anything believes. Four bytes a track against
    /// the model's five hundred and twelve, so holding it costs under a
    /// percent of what the vectors already do, and reading the tempo out of
    /// the same scan is the difference between ranking by tempo and running a
    /// second query per candidate.
    bpm: Vec<f32>,
}

impl Corpus {
    /// Read a model's candidates, over the same slice of the table a query
    /// used to scan: every row under [`CANDIDATE_CAP`], an evenly spaced
    /// stride of them past it.
    ///
    /// `rows` is the fingerprint's count, which is both where the stride
    /// comes from and how much room to take up front. Growing a
    /// twenty-five-megabyte buffer by doubling copies most of it several
    /// times over, and the count is already in hand.
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

    /// Every candidate's cosine against an already quantized seed, `skip`
    /// left out. Integer arithmetic all the way to the end: the widest a
    /// dot can get is 127 squared times the width of the model, which is
    /// twenty-three bits under the vectors rox ships, so the accumulator has
    /// room to spare.
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

    /// Charge every score for how far its track's tempo is from the seed's,
    /// in place. See [`tempo_penalty`] for what the charge is worth and what
    /// counts as far.
    ///
    /// Iterated in step with the corpus rather than looked up by id. `scores`
    /// is what [`Corpus::scores`] returned for this same corpus, which is these
    /// rows in these positions with at most the seed missing, so one cursor
    /// covers it and a fifty-thousand-track ranking builds no map to throw
    /// away. The id check lets the seed drop out; a row that doesn't
    /// match is skipped rather than charged, so the worst a mismatch could do
    /// is leave scores raw.
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

/// Append one standardized row to `out` as bytes, and return the reciprocal
/// length of what was written. A row of nothing gets a reciprocal of zero,
/// which scores it zero against everything: a track exactly on the corpus
/// mean points nowhere, and that's the honest answer rather than a
/// direction invented by rounding.
///
/// The clamp is belt and braces. The row's own peak comes out at 127 by
/// construction and the cast saturates anyway; what it's guarding is the
/// float rounding either side of that.
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

/// A seed quantized on its own, the way a corpus row is, with the reciprocal
/// length that turns its dot products into cosines.
fn quantize_seed(z: &[f32]) -> (Vec<i8>, f32) {
    let mut cells = Vec::with_capacity(z.len());
    let inv = quantize(z, &mut cells);
    (cells, inv)
}

/// The corpus for a model, read from the table when nothing held describes it.
///
/// Held like the standardization is, under the same evidence, and rebuilt
/// whole when that evidence moves: a rebuild costs one table read, which is
/// what every single query cost before this existed. That's the shape the
/// analysis pass needs. It bumps the write counter with every batch it
/// commits, so while a pass is running this is back to a read per query, a
/// tenth slower than the old scan for the quantizing it does on top. The
/// moment the pass stops the corpus stands and the queries go quiet.
fn held_corpus(
    conn: &Connection,
    model: &str,
    stats: &Stats,
    db: Option<&str>,
    at: Fingerprint,
) -> rusqlite::Result<Arc<Corpus>> {
    if let Some(db) = db {
        let held = CORPUS_CACHE.read().expect("corpus cache never poisons");
        if let Some(entry) = held.as_ref() {
            if entry.db == db && entry.model == model && entry.at == at {
                return Ok(entry.corpus.clone());
            }
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

/// The standardization for a model, estimated off a bounded sample of its
/// vectors. None when the model has none, or too few for a spread to mean
/// anything.
///
/// Held between calls, so a second question about the same unchanged corpus
/// costs the fingerprint rather than the sample scan.
pub fn stats(conn: &Connection, model: &str) -> rusqlite::Result<Option<Stats>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    Ok(held_stats(conn, model, db.as_deref(), at)?.map(|stats| (*stats).clone()))
}

/// [`stats`] against a fingerprint the caller has already taken, which is how
/// [`scores`] gets its standardization without counting the table twice.
fn held_stats(
    conn: &Connection,
    model: &str,
    db: Option<&str>,
    at: Fingerprint,
) -> rusqlite::Result<Option<Arc<Stats>>> {
    if let Some(db) = db {
        let held = STATS_CACHE.read().expect("stats cache never poisons");
        if let Some(entry) = held.as_ref() {
            if entry.db == db && entry.model == model && entry.at == at {
                return Ok(entry.stats.clone());
            }
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

/// The sample scan itself, the part being held.
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
            // Variance as E[x^2] - E[x]^2, clamped: the two terms are close
            // for a near-constant dimension and floating point can put the
            // difference just under zero.
            let var = (q / n - (s / n) * (s / n)).max(0.0);
            let std = var.sqrt();
            if std > 1e-9 {
                (1.0 / std) as f32
            } else {
                0.0
            }
        })
        .collect();
    Ok(Some(Stats { dim, mean, inv_std }))
}

/// One track's raw stored vector.
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

/// Every vector this model holds, with the file it describes and which slice
/// of that file it is. For [`crate::bake`], which puts them back into the
/// files they came out of.
///
/// Local rows only, the bound the rest of the work lists draw: a streaming
/// source's row names nothing on disk to write a tag into. The `sub` is
/// selected too, for the refusal a shared cue image earns.
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
/// Empty when the seed has no vector, or when the corpus is too small to
/// standardize against.
///
/// Scored against a [`Corpus`] held in memory, so the table is read once and
/// not once per question. The first question after a change pays for that
/// read: the model rox ships stores two kilobytes a track, so a
/// fifty-thousand-track library is a hundred megabytes off disk, a third of a
/// second (the workspace builds this crate optimized in dev too, or it would
/// be seconds). Every question after it is arithmetic over twenty-five
/// megabytes of quantized vectors already in memory, six milliseconds in
/// release and fourteen in a debug build, which makes a score per track
/// change affordable. Past [`CANDIDATE_CAP`] both the corpus and the
/// scoring cover an evenly spaced slice instead of everything, which keeps
/// a very large library bounded.
///
/// The answer is held on top of that: asking the same seed again while the
/// table hasn't moved costs the fingerprint and a copy of the map, so a Similar
/// column asking about the track the transport just drew doesn't redo the
/// transport's arithmetic.
///
/// The whole map rather than a top-k, because callers want different slices
/// of it: [`nearest`] takes the head, and a column showing what resembles the
/// playing track needs a score for rows anywhere in the list.
///
/// Raw, and staying that way. This is the cosine and nothing else, which is
/// what a diagnostic wants: the library's Similar column prints this number,
/// and a column that quietly showed a track's timbre marked down for running
/// at the wrong speed would be reporting something nobody asked for. What
/// playback picks from is [`ranked`], which is this map with the tempo penalty
/// on top.
pub fn scores(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    Ok((*held_scores(conn, track_id, model, db, at)?).clone())
}

/// How much every other track resembles `track_id` once tempo has its say:
/// [`scores`] with [`tempo_penalty`] taken off each candidate in proportion to
/// how far its tempo is from the seed's.
///
/// Every pick playback makes runs on this. Two tracks can share a
/// timbre and be unplayable one after the other for running at different
/// speeds, and the vectors say nothing about it: the model hears texture, and
/// an eight-track band of the nearest is easily eight tempos. Subtracting
/// rather than filtering, because a tempo is a preference and not a rule. A
/// track a whole band better on timbre still wins, and a library with no
/// tempos measured ranks exactly as it did before this existed.
///
/// Costs the penalty pass over what [`scores`] already held: no second scan,
/// no second cache. The raw map is the thing worth holding, and it's the same
/// map whichever seed's tempo is being charged against it.
pub fn ranked(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
    let db = db_key(conn);
    let at = fingerprint(conn, model, db.as_deref())?;
    let mut scored = (*held_scores(conn, track_id, model, db.clone(), at)?).clone();
    // A seed with no tempo still charges: the candidates that have one
    // disagree with it about whether there's a beat, and [`tempo_penalty`]
    // prices that off the NaN the corpus files a missing tempo under. What
    // has nothing to charge is an unscorable model, which leaves the raw map
    // alone.
    let seed_bpm = track_bpm(conn, track_id)?.unwrap_or(f32::NAN);
    let Some(stats) = held_stats(conn, model, db.as_deref(), at)? else {
        return Ok(scored);
    };
    // The same fingerprint the scores were taken under, so the corpus this
    // iterates is the one they came out of, row for row.
    let corpus = held_corpus(conn, model, &stats, db.as_deref(), at)?;
    corpus.penalize(&mut scored, seed_bpm);
    Ok(scored)
}

/// [`scores`] against a fingerprint the caller has already taken, and the
/// holding that goes with it.
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

/// The scoring pass itself, the part being held.
///
/// The seed's own vector is read from its row every time rather than looked up
/// in the corpus: it's one point query against a primary key, and a seed the
/// stride left out still deserves an answer about the library.
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
    // The seed is read straight from its row rather than through
    // [`each_vector`], so it wants the same two checks: a width nothing can
    // be compared against, and a non-finite value that would make every
    // score in the answer NaN.
    if raw.len() != stats.dim || raw.iter().any(|v| !v.is_finite()) {
        return Ok(Vec::new());
    }
    let corpus = held_corpus(conn, model, &stats, db, at)?;
    let mut z = Vec::with_capacity(stats.dim);
    stats.standardize(&raw, &mut z);
    let (seed, seed_inv) = quantize_seed(&z);
    Ok(corpus.scores(&seed, seed_inv, track_id))
}

/// The `k` tracks whose vectors are closest to `track_id`'s, nearest first.
/// The seed itself is never in the result, and neither is a track whose file
/// the library has since dropped.
///
/// Off the raw cosine, the head of [`scores`]. Playback draws from
/// [`nearest_ranked`].
pub fn nearest(
    conn: &Connection,
    track_id: i64,
    model: &str,
    k: usize,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    Ok(head(scores(conn, track_id, model)?, k))
}

/// [`nearest`] over [`ranked`]: the `k` tracks that best resemble `track_id`
/// once a tempo the seed doesn't share has been charged for.
pub fn nearest_ranked(
    conn: &Connection,
    track_id: i64,
    model: &str,
    k: usize,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    Ok(head(ranked(conn, track_id, model)?, k))
}

/// The best `k` of a score map, nearest first. Ties are broken by id so the
/// order is stable between calls rather than however the sort happened to
/// come out.
fn head(mut scored: Vec<(i64, f32)>, k: usize) -> Vec<(i64, f32)> {
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(k);
    scored
}

/// f32 little-endian, the shape every platform rox ships on reads natively.
fn encode(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// The inverse, tolerant of a trailing partial float rather than panicking
/// on one: a truncated blob reads as the floats it does hold, and the width
/// check in [`scores`] drops it.
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

    /// A library on disk. The held answers are keyed by database file, so
    /// the in-memory fixture every other test here uses is recomputed fresh
    /// every time and would prove nothing about the cache.
    fn file_conn(name: &str) -> Connection {
        let dir = std::env::temp_dir().join(format!("rox-embeddings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::store::open(&dir.join("library.db")).unwrap();
        crate::store::init_schema(&conn).unwrap();
        conn
    }

    /// A track row with just enough columns to be joinable.
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

    /// Give a track a tempo the way a tagger would, without going through the
    /// measurement pass: the tests that care about the write counter call
    /// [`crate::store::set_measured_bpm`] instead.
    /// A tempo straight onto the row, with the write noted the way the
    /// store's own writers note it, so a corpus held from before the tag
    /// is reread rather than answering with the tempo it no longer has.
    fn tag_bpm(conn: &Connection, id: i64, bpm: f32) {
        conn.execute(
            "UPDATE tracks SET bpm = ?2 WHERE id = ?1",
            rusqlite::params![id, bpm],
        )
        .unwrap();
        note_write(conn);
    }

    /// Half time and double time are the same music counted differently, so
    /// the distance folds: 70 against 140 is no gap at all, and the furthest
    /// two tempos can be from each other is half an octave.
    #[test]
    fn tempo_distance_folds_octaves_and_ignores_what_it_cannot_measure() {
        let close = |a: f32, b: f32, want: f32| {
            let got = tempo_distance(a, b);
            assert!(
                (got - want).abs() < 0.005,
                "{a} against {b} came out {got}, wanted {want}"
            );
            // Which one is the seed changes nothing.
            assert!((got - tempo_distance(b, a)).abs() < 1e-6);
        };
        close(128.0, 128.0, 0.0);
        close(70.0, 140.0, 0.0);
        close(70.0, 280.0, 0.0);
        close(87.0, 174.0, 0.0);
        // The example the weight is calibrated against: two tempos nobody
        // would mix, a third of an octave apart.
        close(140.0, 175.0, 0.322);
        close(120.0, 128.0, 0.093);
        // The furthest apart two tempos get, a tempo and half again.
        close(100.0, 150.0, 0.415);
        close(100.0, 141.4, 0.5);
        // Nothing measurable earns no penalty rather than a wrong one, which
        // leaves an untempoed library ranking as it always did.
        assert_eq!(tempo_distance(f32::NAN, 128.0), 0.0);
        assert_eq!(tempo_distance(128.0, f32::NAN), 0.0);
        assert_eq!(tempo_distance(0.0, 128.0), 0.0);
        assert_eq!(tempo_distance(-128.0, 128.0), 0.0);
    }

    /// The regression the drift term exists for. A 70 BPM ambient seed drew a
    /// 133 BPM EBM track because the fold saw a ratio of 1.9 as very nearly a
    /// double and charged almost nothing for it. It has to cost more than a
    /// clean double now, and a clean double has to cost more than a match.
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
        // Which one is the seed changes nothing here either.
        assert!((paid(133.0, 70.0) - ebm).abs() < 1e-6);
    }

    /// What the fold is for still works. A drum and bass track written down
    /// at 87 and the same one at 174 is one tagger disagreeing with another,
    /// so the pair pays the ceiling and nothing more: 0.06 against the 0.095
    /// a median neighbourhood spans, which moves it down the band and leaves
    /// it in the band.
    #[test]
    fn a_half_time_reading_stays_inside_a_neighbourhood() {
        let dnb = tempo_penalty(87.0, 174.0);
        assert!(
            (dnb - 0.06).abs() < 0.002,
            "87 against 174 pays the ceiling, paid {dnb}"
        );
        assert!(dnb < 0.095, "and less than a median band, paid {dnb}");
        // Two octaves out is the same disagreement made twice, not twice the
        // disagreement, so the ceiling holds.
        assert!((tempo_penalty(70.0, 280.0) - dnb).abs() < 0.002);
        // Nothing pays more than the folded half octave plus the ceiling.
        assert!(tempo_penalty(40.0, 300.0) <= 0.21 + 0.002);
    }

    /// Two tracks with no tempo agree, so they pay nothing, which is what
    /// keeps a library nothing has measured ranking on cosine alone. One
    /// with and one without disagree about whether there's a beat at all,
    /// and pay the flat charge for it, whichever side the gap is on and
    /// whatever shape the missing number takes.
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
        // The worst a measured pair can do: half an octave folded and the
        // drift ceiling, 100 against a tempo one and a half octaves up.
        assert!(
            NO_TEMPO_PENALTY < tempo_penalty(100.0, 282.8),
            "better than the worst mismatch"
        );
    }

    /// What [`ranked`] is: [`scores`] with the tempo charge taken off, track
    /// for track, and the reordering that comes out of it.
    ///
    /// The candidates here are close enough together that the charge decides
    /// between them, which is the case worth pinning: the raw leader running
    /// at a tempo the seed doesn't share gives up the lead, and the track it
    /// gives it up to is the nearest one that does share it. The raw map is
    /// left exactly as it was, since the column that prints it wants the
    /// cosine and nothing else.
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

        // The seed at 140, the raw leader at half of 175 so the fold has
        // something to undo, the runner-up at double the seed's tempo, which
        // is the same music counted twice as fast. Everything else is
        // untempoed, and pays the flat charge for disagreeing with a seed
        // that has one.
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
        // Every candidate is still in the answer: this marks a track down, it
        // doesn't drop it.
        assert_eq!(ranked_order.len(), leader.len());
        // The raw map is untouched, and it's what the Similar column reads.
        assert_eq!(
            scores(&conn, seed, "m")
                .unwrap()
                .into_iter()
                .collect::<HashMap<_, _>>(),
            raw
        );
        assert_eq!(nearest(&conn, seed, "m", 12).unwrap(), leader);
    }

    /// A library with no tempos anywhere ranks exactly as it did before the
    /// penalty existed. One where only the candidates have them keeps its
    /// order too: a seed with no tempo charges every measured candidate the
    /// same flat amount, and a charge everyone pays moves nobody.
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

        // Tempos on the candidates and none on the seed: every candidate
        // pays the flat charge for having one where the seed has none, which
        // is the same charge for each and leaves the order where it was.
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
        // A tempo nothing believes is no tempo: the seed's 9999 reads as
        // missing, the same flat charge, rather than filing every candidate
        // half an octave off.
        tag_bpm(&conn, ids[0], 9999.0);
        flat(nearest_ranked(&conn, ids[0], "m", 6).unwrap());
    }

    /// The tempo pass writes onto `tracks`, not onto the embeddings table, and
    /// the held corpus holds every row's tempo. So a measurement has to put
    /// that corpus back on the table, or the first ranking after a tempo pass
    /// goes on ranking as though the library had no tempos at all.
    ///
    /// The seed's own tempo is read per query and would move on its own, so
    /// the candidate is the one being measured here: nothing but a reread of
    /// the corpus can tell the ranking about it.
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
        // The seed measured first, which builds and holds a corpus in which
        // nothing else has a tempo: every candidate pays the flat charge for
        // having none, and the order is the raw one.
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

        // The leader measured at a tempo the seed doesn't share. Its vector
        // never moved, so a corpus that outlived the write would hand back
        // the same ranking.
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
        // The raw cosine is what it always was: the write moved the ranking,
        // not the vectors.
        let raw: HashMap<i64, f32> = scores(&conn, ids[0], "m").unwrap().into_iter().collect();
        assert!((raw[&leader] - raw_order[0].1).abs() < 1e-6);
    }

    /// The cheap gate the modes that rank by sound are offered on: per
    /// model, and false until that model has actually described something.
    /// Its whole job is telling "the switch is on" apart from "the pass has
    /// run", which are the two states a menu has to draw differently.
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
        // Per model: another model's vectors are not this one's.
        assert!(!any(&conn, "other").unwrap());
    }

    #[test]
    fn a_stored_vector_comes_back_the_way_it_went_in() {
        let conn = conn();
        let id = add_track(&conn, "/m/1.mp3", 200_000);
        let vec = vec![0.5, -1.25, 0.0, 3.75];
        upsert(&conn, id, "m", &vec).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec.clone()));
        // A second write for the same pair replaces rather than duplicates.
        upsert(&conn, id, "m", &[1.0, 1.0, 1.0, 1.0]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec![1.0; 4]));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "the composite key holds one row per model");
        assert_eq!(vector(&conn, id, "other").unwrap(), None);
    }

    /// The store declines to hold a vector it knows ruins every query that
    /// touches it. Refusing costs the one track, where writing costs every
    /// score in the library: the standardization draws its mean from the
    /// corpus, so a single NaN spreads to vectors that never had one.
    #[test]
    fn a_vector_with_a_nan_never_reaches_the_table() {
        let conn = conn();
        let id = add_track(&conn, "/m/1.mp3", 200_000);
        upsert(&conn, id, "m", &[1.0, f32::NAN]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), None);
        assert!(!any(&conn, "m").unwrap(), "nothing was described");
        upsert(&conn, id, "m", &[1.0, f32::INFINITY]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), None);
        // A refusal leaves the row that was already there alone, rather
        // than replacing a good description with nothing.
        upsert(&conn, id, "m", &[1.0, 2.0]).unwrap();
        upsert(&conn, id, "m", &[f32::NAN, 2.0]).unwrap();
        assert_eq!(vector(&conn, id, "m").unwrap(), Some(vec![1.0, 2.0]));
    }

    /// A library under the cap is read whole; past it the scan takes an
    /// even slice and stays near the ceiling however big the corpus gets.
    /// That keeps one "more like this" from turning into a full table scan
    /// on a library with a million tracks in it.
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
        // Spread across the library rather than clustered at one end: a
        // contiguous slice of scan order would be one folder's worth.
        let lo = *seen.iter().min().unwrap();
        let hi = *seen.iter().max().unwrap();
        assert!(
            lo < 150 && hi > 850,
            "the sample spans the library, got {lo}..{hi} over {} rows",
            seen.len()
        );
        // Deterministic, which is what a column showing a score needs.
        let mut again = Vec::new();
        each_vector(&conn, "m", 2, stride, |id, _, _| again.push(id)).unwrap();
        assert_eq!(seen, again);
    }

    /// The cap is a ceiling, so a corpus exactly at it is still read whole.
    /// Adding one to a plain division strides by two there instead, and
    /// every score a "more like this" comes back with is drawn from
    /// whichever half of the library the stride happened to pick.
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
        // An empty corpus and a nonsense cap still stride by something the
        // scan's modulo can divide by.
        assert_eq!(stride_from(model_rows(&conn, "none").unwrap(), 1000), 1);
        assert_eq!(stride_from(rows, 0), 1000);
    }

    /// One track whose vector came out NaN must not cost the library every
    /// similarity answer it has. It would: a NaN makes that dimension's
    /// mean NaN, the variance clamp reads NaN as zero, and every vector
    /// standardized against those statistics comes out NaN too, so every
    /// score ties and "nearest" degenerates into "lowest id". The row is
    /// skipped on the way in and on the way out, and the rest of the corpus
    /// ranks as though it were never there.
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

        // A row written by something that didn't check, or a blob that rotted
        // in place: the right width, and not a number.
        let bad = add_track(&conn, "/m/bad.mp3", 200_000);
        upsert(&conn, bad, "m", &[f32::NAN, f32::INFINITY]).unwrap();
        let after = nearest(&conn, ids[0], "m", 5).unwrap();
        assert_eq!(after, clean, "the poisoned row changed nothing");
        assert!(
            after.iter().all(|(id, _)| *id != bad),
            "and it is not a neighbour of anything"
        );
        // Asking what the poisoned track sounds like answers nothing rather
        // than answering with the whole library at a score of NaN.
        assert!(nearest(&conn, bad, "m", 5).unwrap().is_empty());
    }

    /// Asking twice about one seed reads the table once, and a write puts
    /// both the held score map and the held [`Corpus`] back on it.
    ///
    /// The two writes below are the same bytes into the same row, so the
    /// only thing separating them is the evidence they leave: the raw
    /// statement moves no row count and isn't a write this module made, which
    /// is precisely the change the fingerprint can't see, and the answer that
    /// comes back is the held one. The [`upsert`] right after it changes
    /// nothing in the table and everything about the evidence, and the reread
    /// finds the vector that was already sitting there.
    ///
    /// The seed asked at the end never had a score map of its own, so the only
    /// thing that could hand it a stale answer is a corpus that outlived the
    /// write. It's the assertion that separates "the map was dropped" from
    /// "the vectors were reread".
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
        // Held per seed, so the neighbour's own question is its own answer
        // rather than the seed's map handed back under a different name.
        let other = nearest(&conn, near, "m", 6).unwrap();
        assert!(other.iter().all(|(id, _)| *id != near));
        assert_eq!(nearest(&conn, seed, "m", 6).unwrap(), after);

        // A seed nothing has asked about, so nothing but the corpus can
        // answer it. The rewritten track now holds the same vector the
        // original seed does, and a corpus kept across the write would
        // still be holding the old one and scoring the two apart.
        let fresh: HashMap<i64, f32> = scores(&conn, ids[3], "m").unwrap().into_iter().collect();
        assert_eq!(
            fresh[&far], fresh[&seed],
            "two identical vectors score identically, so the corpus was reread"
        );
    }

    /// Deterministic values with the shape a real embedding has: a bell with
    /// a heavy tail on it. The tail is the part that matters, because it's
    /// what decides between quantizing against a fixed range and quantizing
    /// against each row's own peak.
    struct Rolls(u64);

    impl Rolls {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / 8388608.0 - 1.0
        }

        /// Twelve uniforms make a passable bell, and one roll in forty comes
        /// back ten times as far out.
        fn value(&mut self) -> f32 {
            let bell: f32 = (0..12).map(|_| self.next()).sum::<f32>() / 2.0;
            if self.next() > 0.95 {
                bell * 10.0
            } else {
                bell
            }
        }
    }

    /// The bytes the corpus scores against the floats they stand in for.
    ///
    /// The live library's failure, in miniature. One dimension sits at zero
    /// for every track but two, where it reads one: a fact about those two,
    /// and after standardizing a cell of fifteen sigma against a row of
    /// ordinary ones. Without the clip that cell is the row, and the two
    /// spiking tracks come out as each other's nearest neighbour whatever the
    /// rest of them holds. With it, the track that actually shares a seed's
    /// other sixty-three dimensions is the one that comes back first.
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
        // The seed and a stranger both spike; a twin shares the seed's every
        // other cell and stays quiet.
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

    /// Quantizing is only worth doing if the answer holds up under it, and "the
    /// answer" is two things: the number the Similar column prints, which
    /// wants the score right to about a hundredth, and the order the
    /// neighbours come back in, which wants the top of the list to be the
    /// same tracks. Both hold here with room to spare, against a corpus built
    /// the way a real one is: dimensions on wildly different scales, so the
    /// standardization has real work, and a heavy tail on each so the
    /// quantizer meets the outliers it was chosen for.
    #[test]
    fn quantizing_the_corpus_keeps_the_score_and_the_order() {
        let conn = conn();
        let (tracks, dim) = (300usize, 512usize);
        let mut rolls = Rolls(0x2545_F491_4F6C_DD1D);
        // Per dimension: a scale over four orders of magnitude and an offset
        // to match, exactly what standardizing is for.
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

        // The same corpus in floats, standardized and scaled to unit length,
        // which is the exact answer the bytes are approximating.
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

            // Ranked the way [`nearest`] ranks, and compared as sets: what a
            // caller asks for is which tracks come back, not what order two
            // near-identical scores landed in.
            let order = |mut v: Vec<(i64, f32)>| -> Vec<(i64, f32)> {
                v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                v.truncate(10);
                v
            };
            let want = order(exact);
            let got = order(quantized);
            // The same track leads, unless the float answer had two in a
            // dead heat at the top, in which case whichever of the pair the
            // bytes put first is as right as the other. The same tolerance
            // as the tenth place gets below.
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
            // A random corpus puts tracks within a ten-thousandth of each
            // other either side of the line, and which of those two the cut
            // falls between is not something a score is being trusted for.
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

    /// What the storage page lists: every model in the table, however this
    /// build's own model is named, with what it costs.
    #[test]
    fn models_report_their_rows_and_their_width() {
        let conn = conn();
        assert!(models(&conn).unwrap().is_empty(), "nothing described yet");
        let a = add_track(&conn, "/m/a.mp3", 200_000);
        let b = add_track(&conn, "/m/b.mp3", 200_000);
        upsert(&conn, a, "panns", &vec![0.25; 512]).unwrap();
        upsert(&conn, b, "panns", &vec![0.25; 512]).unwrap();
        // A model this build has never heard of, left behind by a rename.
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
        // The width is measured off the blob, so a row whose dim column
        // disagrees with what it stores is reported as what it costs.
        conn.execute("UPDATE embeddings SET dim = 9 WHERE model = 'old'", [])
            .unwrap();
        assert_eq!(models(&conn).unwrap()[0].dim, 3);
    }

    /// Clearing a model hands the file's pages back rather than leaving
    /// them free inside it. On a described library the vectors are most of
    /// what the database weighs, so a clear that left the file the same
    /// size would read as having done nothing at all.
    #[test]
    fn clearing_a_model_shrinks_the_file() {
        let dir = std::env::temp_dir().join("rox-embeddings-vacuum");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A rollback-journal database rather than [`crate::store::open`]'s
        // WAL, so the shrink shows up in the file's own size the moment the
        // VACUUM commits instead of waiting on a checkpoint.
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
        // The tracks the vectors described are untouched: this drops a
        // description, not a library.
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
        // A track with no running time has no windows to sample and never
        // appears, embedded or not.
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
        // Another model has covered nothing, whatever the first one did.
        assert_eq!(missing(&conn, "other").unwrap().len(), 2);
        assert_eq!(
            coverage(&conn, "m").unwrap(),
            Coverage {
                embedded: 1,
                total: 2
            }
        );
        assert_eq!(coverage(&conn, "m").unwrap().missing(), 1);

        // A row from another source has no file for the extractor to open,
        // so it's neither work to do nor a track the coverage owes a vector.
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

        // A cue subsong is work like any other: it decodes and it earns a
        // vector. Its sub comes back with it so the pass can tell the file
        // underneath is shared and can't be tagged for it.
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
        // Two dimensions that vary together, one track sitting between the
        // other two and closer to the first.
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
        // k caps the result.
        assert_eq!(nearest(&conn, seed, "m", 1).unwrap().len(), 1);
        // A track with no vector for this model has no neighbours.
        let bare = add_track(&conn, "/m/bare.mp3", 200_000);
        assert!(nearest(&conn, bare, "m", 5).unwrap().is_empty());
    }

    /// A dimension with no information in it changes no neighbours, however
    /// large it is. That's what standardizing buys: raw cosine over these
    /// two models would rank them differently, because the constant 500
    /// would dominate the sum and pull every pair toward identical.
    #[test]
    fn a_constant_dimension_does_not_change_the_ordering() {
        let conn = conn();
        // Enough points spread over two axes that the per-dimension
        // statistics mean something.
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
        // One row under "two" is a corpus of itself; nothing to compare to.
        assert!(nearest(&conn, a, "two", 5).unwrap().is_empty());
        assert_eq!(clear(&conn, "two").unwrap(), 1);
        assert_eq!(nearest(&conn, a, "one", 5).unwrap().len(), 1);

        // A dropped track takes its vector with it: foreign keys default on
        // in this build, so the schema's cascade fires and there is nothing
        // left for the prune to find.
        conn.execute("DELETE FROM tracks WHERE id = ?1", [b])
            .unwrap();
        assert!(nearest(&conn, a, "one", 5).unwrap().is_empty());
        assert_eq!(prune(&conn).unwrap(), 0, "the cascade already took it");

        // With enforcement off, which is SQLite's own default and what a
        // database written by another build could have had, the row outlives
        // its track. The joins hide it and the prune clears it.
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
