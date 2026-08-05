//! Acoustic embeddings: one feature vector per track per model, and the
//! nearest-neighbour query over them. What produces the vectors is the app's
//! business (`rox/src/embeddings.rs` runs the DSP extractor); this module
//! only stores them and answers "what sounds like this".
//!
//! Rows are keyed by model name rather than a version number, so a library
//! can hold two models' worth at once and switching between them costs
//! nothing. An extractor whose output changes meaning takes a new name, and
//! the old vectors sit there until something clears them.
//!
//! Vectors go in raw, unnormalized. Standardizing belongs to the query
//! ([`nearest`] z-scores each dimension against the corpus before comparing),
//! because raw feature statistics live on wildly different scales: a band
//! energy in the tens and an onset rate near one, compared by cosine, means
//! the loud dimensions decide every neighbour and the rest are decoration.
//! Doing it at read time also means the weighting can be retuned without
//! re-extracting the library.

use rusqlite::Connection;

/// The table beside the tracks it describes. Composite key, so one track
/// carries a row per model it has been through.
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
/// carries nothing it could open.
pub fn missing(conn: &Connection, model: &str) -> rusqlite::Result<Vec<Pending>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path, t.duration_ms FROM tracks t
         LEFT JOIN embeddings e ON e.track_id = t.id AND e.model = ?1
         WHERE e.track_id IS NULL AND t.source = 'local' AND t.duration_ms > 0
         ORDER BY t.id",
    )?;
    let rows = stmt.query_map([model], |r| {
        Ok(Pending {
            id: r.get(0)?,
            path: r.get(1)?,
            duration_ms: r.get::<_, i64>(2)? as u32,
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
/// [`coverage`] answers the same question and more, at the cost of counting
/// two tables; this stops at the first row, so it's cheap enough for the
/// paths that only need to know whether ranking by sound can answer
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
/// A vector carrying a NaN or an infinity is refused rather than written.
/// One of them is enough to make every score in the library NaN, because
/// the standardization takes its mean from the corpus and hands the poison
/// to every vector it touches, and the row would go on doing that to every
/// query until somebody deleted it by hand. The caller isn't told: the read
/// side skips such a row anyway, this is the store declining to hold one at
/// all, and failing a whole batch over a single bad track would throw away
/// the good work beside it. Whoever produced the vector is the one that can
/// name the file, so the app-side guard is where a listener hears about it.
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

/// Drop every vector for a model, for when its extractor changed under the
/// same name and the library needs redoing.
pub fn clear(conn: &Connection, model: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM embeddings WHERE model = ?1", [model])
}

/// Drop vectors whose track is gone. A no-op while the cascade is doing its
/// job (see [`init_schema`]), and the cleanup for a database whose writer
/// had foreign keys off.
pub fn prune(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM embeddings WHERE track_id NOT IN (SELECT id FROM tracks)",
        [],
    )
}

/// How much of the corpus a single query will read before it starts
/// sampling. Above this the scan takes an evenly spaced slice instead of
/// every row, which keeps a huge library's "more like this" bounded rather
/// than linear. 200k vectors is ~50 MB read and a fifth of a second, which
/// is the most a background query should cost.
pub const CANDIDATE_CAP: usize = 200_000;

/// Rows the standardization statistics are estimated from. A mean and a
/// standard deviation converge long before the whole corpus is counted, so
/// there's nothing to gain from reading a million vectors to find them.
const STATS_SAMPLE: usize = 20_000;

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

    /// Standardize a raw vector into `out` and scale it to unit length, so a
    /// dot product between two of them is the cosine.
    fn apply(&self, raw: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend(
            raw.iter()
                .zip(&self.mean)
                .zip(&self.inv_std)
                .map(|((v, m), i)| (v - m) * i),
        );
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for v in out.iter_mut() {
                *v /= norm;
            }
        }
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

/// Take every `stride`-th row, picked so no more than `cap` come back. One
/// means read every row, which is the answer for any library that fits under
/// the cap.
///
/// A plain stride over a sequential scan, which reads more than it scores.
/// That's deliberate, and it's measured: making the read itself selective
/// (an index on a hashed bucket, or a union of contiguous id windows) forces
/// SQLite to fetch each 256-byte vector by rowid, and scattered fetches cost
/// several times what reading the table straight through does. The cap
/// bounds the decoding and the arithmetic, which is what grows without one;
/// the scan underneath stays linear and cheap per row.
///
/// Deterministic rather than random: the same query twice gives the same
/// numbers, which a column showing a score needs. Variety for a radio that
/// wants a different answer each time belongs to whatever picks the seed
/// track, not here.
fn stride_for(conn: &Connection, model: &str, cap: usize) -> rusqlite::Result<i64> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM embeddings WHERE model = ?1",
        [model],
        |r| r.get(0),
    )?;
    // Ceiling division, so a corpus of exactly the cap is still read whole
    // and one row past it is the first to stride. Adding one to a plain
    // division instead strides by two at every exact multiple of the cap,
    // which scores half a corpus the cap says to read all of.
    let total = total.max(0) as usize;
    Ok(total.div_ceil(cap.max(1)).max(1) as i64)
}

/// Walk a model's vectors, handing each to `visit` in turn. The vector is
/// borrowed from a buffer reused across rows, so a million-track scan holds
/// one vector at a time rather than a million of them.
fn each_vector(
    conn: &Connection,
    model: &str,
    dim: usize,
    stride: i64,
    mut visit: impl FnMut(i64, &[f32]),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT e.track_id, e.vec FROM embeddings e
         JOIN tracks t ON t.id = e.track_id
         WHERE e.model = ?1 AND e.dim = ?2 AND (?3 = 1 OR e.track_id % ?3 = 0)",
    )?;
    let mut rows = stmt.query(rusqlite::params![model, dim as i64, stride])?;
    let mut buf: Vec<f32> = Vec::with_capacity(dim);
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let bytes = row.get_ref(1)?.as_blob()?;
        buf.clear();
        buf.extend(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        );
        // A blob of the wrong width can't be compared against the rest, and
        // neither can one carrying a NaN or an infinity: it wouldn't merely
        // score badly, it would take the whole corpus with it. One NaN in a
        // dimension makes that dimension's mean NaN, the variance clamp
        // reads NaN as zero (f64::max ignores it), and every vector
        // standardized against those statistics comes out NaN. Every score
        // in the library then ties, and "nearest" quietly means "lowest id".
        if buf.len() == dim && buf.iter().all(|v| v.is_finite()) {
            visit(id, &buf);
        }
    }
    Ok(())
}

/// The standardization for a model, estimated off a bounded sample of its
/// vectors. None when the model has none, or too few for a spread to mean
/// anything.
pub fn stats(conn: &Connection, model: &str) -> rusqlite::Result<Option<Stats>> {
    let Some(dim) = dominant_dim(conn, model)? else {
        return Ok(None);
    };
    let stride = stride_for(conn, model, STATS_SAMPLE)?;
    let mut n = 0usize;
    let mut sum = vec![0f64; dim];
    let mut sq = vec![0f64; dim];
    each_vector(conn, model, dim, stride, |_, vec| {
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

/// How much every other track resembles `track_id`, in -1..=1, unordered.
/// Empty when the seed has no vector, or when the corpus is too small to
/// standardize against.
///
/// Streams the corpus rather than loading it: one vector is held at a time,
/// so what the call costs in memory is the scores it returns (twelve bytes a
/// track) rather than the vectors it read (256 bytes a track). Past
/// [`CANDIDATE_CAP`] it scores an evenly spaced slice instead of everything,
/// which is what keeps a very large library from turning one question into a
/// full table scan.
///
/// The whole map rather than a top-k, because callers want different slices
/// of it: [`nearest`] takes the head, and a column showing what resembles the
/// playing track needs a score for rows anywhere in the list.
pub fn scores(conn: &Connection, track_id: i64, model: &str) -> rusqlite::Result<Vec<(i64, f32)>> {
    let Some(stats) = stats(conn, model)? else {
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
    let mut seed = Vec::with_capacity(stats.dim);
    stats.apply(&raw, &mut seed);
    let stride = stride_for(conn, model, CANDIDATE_CAP)?;
    let mut out = Vec::new();
    let mut buf = Vec::with_capacity(stats.dim);
    each_vector(conn, model, stats.dim, stride, |id, vec| {
        if id == track_id {
            return;
        }
        stats.apply(vec, &mut buf);
        out.push((id, dot(&seed, &buf)));
    })?;
    Ok(out)
}

/// The `k` tracks whose vectors sit closest to `track_id`'s, nearest first.
/// The seed itself is never in the result, and neither is a track whose file
/// the library has since dropped.
pub fn nearest(
    conn: &Connection,
    track_id: i64,
    model: &str,
    k: usize,
) -> rusqlite::Result<Vec<(i64, f32)>> {
    let mut scored = scores(conn, track_id, model)?;
    // Descending by similarity, ties broken by id so the order is stable
    // between calls rather than however the sort happened to land.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(k);
    Ok(scored)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
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
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
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
    /// corpus, so a single NaN reaches vectors that never had one.
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
    /// This is what keeps one "more like this" from turning into a full
    /// table scan on a library with a million tracks in it.
    #[test]
    fn a_corpus_past_the_cap_is_sampled_evenly() {
        let conn = conn();
        for i in 0..1000 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", &[i as f32, -(i as f32)]).unwrap();
        }
        assert_eq!(
            stride_for(&conn, "m", 5000).unwrap(),
            1,
            "under the cap every row is read"
        );
        let stride = stride_for(&conn, "m", 100).unwrap();
        assert!(stride > 1, "past the cap the scan strides");
        let mut seen = Vec::new();
        each_vector(&conn, "m", 2, stride, |id, _| seen.push(id)).unwrap();
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
        each_vector(&conn, "m", 2, stride, |id, _| again.push(id)).unwrap();
        assert_eq!(seen, again);
    }

    /// The cap is a ceiling, so a corpus sitting exactly on it is still read
    /// whole. Adding one to a plain division strides by two there instead,
    /// and every score a "more like this" comes back with is drawn from
    /// whichever half of the library the stride happened to land on.
    #[test]
    fn a_corpus_at_exactly_the_cap_is_still_read_whole() {
        let conn = conn();
        for i in 0..1000 {
            let id = add_track(&conn, &format!("/m/{i}.mp3"), 200_000);
            upsert(&conn, id, "m", &[i as f32, -(i as f32)]).unwrap();
        }
        assert_eq!(stride_for(&conn, "m", 1000).unwrap(), 1);
        assert_eq!(stride_for(&conn, "m", 999).unwrap(), 2);
        assert_eq!(stride_for(&conn, "m", 500).unwrap(), 2, "and so is half");
        assert_eq!(stride_for(&conn, "m", 499).unwrap(), 3);
        // An empty corpus and a nonsense cap still stride by something the
        // scan's modulo can divide by.
        assert_eq!(stride_for(&conn, "none", 1000).unwrap(), 1);
        assert_eq!(stride_for(&conn, "m", 0).unwrap(), 1000);
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
                    duration_ms: 200_000
                },
                Pending {
                    id: b,
                    path: "/m/b.mp3".into(),
                    duration_ms: 200_000
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

    /// A dimension carrying no information changes no neighbours, however
    /// large it is. This is what standardizing buys: raw cosine over these
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
