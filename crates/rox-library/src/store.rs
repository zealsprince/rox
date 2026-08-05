//! The durable side of ADR 5: bundled SQLite in WAL mode. The write path is
//! batched upsert transactions from the scanner; the read path is the
//! projection load, either one reader or one reader per core over disjoint
//! rowid ranges (WAL gives concurrent readers for free).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::TrackRow;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

/// The library database's migration ladder (ADR 5). Step 1 is the baseline
/// converge, folding in every historical column probe; new schema changes
/// append a clean forward step here rather than growing another probe. See
/// [`crate::migrate`] for the versioning and downgrade policy.
const MIGRATIONS: &[crate::migrate::Migration] = &[
    crate::migrate::Migration {
        name: "baseline",
        up: baseline,
    },
    // Playlist members and listens snapshot the track's path beside its tags,
    // the content key the post-scan reattach matches dangling rows back on
    // when a pruned file returns under a fresh id. Backfilled from the live
    // catalog so existing rows get the durability without a re-add.
    crate::migrate::Migration {
        name: "snapshot-paths",
        up: |conn| {
            crate::playlists::add_path_snapshot(conn)?;
            crate::listens::add_path_snapshot(conn)
        },
    },
    // The library's genre opinions (aliases, display, art) beside the
    // tracks they describe, never touching a file's tags.
    crate::migrate::Migration {
        name: "genre-meta",
        up: crate::genre_meta::init_schema,
    },
    // The stream's sample rate and bit depth beside the bitrate, so the
    // library can show what a file actually is rather than just its
    // container. Every mtime resets, the same move the codec column made:
    // without it the next scan skips unchanged files and the columns stay
    // empty forever.
    crate::migrate::Migration {
        name: "stream-format",
        up: |conn| {
            conn.execute_batch(
                "ALTER TABLE tracks ADD COLUMN sample_rate INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE tracks ADD COLUMN bit_depth INTEGER NOT NULL DEFAULT 0;
                 UPDATE tracks SET mtime = 0;",
            )
        },
    },
    // What a file's ReplayGain tags measured (ADR 19), so the player can
    // level a track without reopening it. Nullable rather than defaulted:
    // 0 dB is a real measurement, and a column that cannot tell it from an
    // untagged file would level every untagged track to the reference. The
    // mtime reset again, since these only arrive by re-reading tags.
    crate::migrate::Migration {
        name: "replaygain",
        up: |conn| {
            conn.execute_batch(
                "ALTER TABLE tracks ADD COLUMN rg_track_gain REAL;
                 ALTER TABLE tracks ADD COLUMN rg_track_peak REAL;
                 ALTER TABLE tracks ADD COLUMN rg_album_gain REAL;
                 ALTER TABLE tracks ADD COLUMN rg_album_peak REAL;
                 UPDATE tracks SET mtime = 0;",
            )
        },
    },
    // Which of the two sources filled the four columns above: the file's
    // tags, or rox's own measurement pass (ADR 19). Nullable and unbackfilled,
    // since NULL reads as tag-sourced and that is what every existing row is.
    // No mtime reset here, unlike the rung above: measuring happens app-side
    // off audio rox already has to decode, so no file is owed a tag re-read
    // for this column.
    crate::migrate::Migration {
        name: "replaygain-source",
        up: |conn| conn.execute_batch("ALTER TABLE tracks ADD COLUMN rg_source INTEGER;"),
    },
    // The acoustic feature vectors behind "sounds like this". Its own table
    // rather than columns on tracks: a vector is orders of magnitude wider
    // than any tag, a library may hold more than one model's worth at once,
    // and nothing in the projection reads them.
    crate::migrate::Migration {
        name: "acoustic-embeddings",
        up: crate::embeddings::init_schema,
    },
];

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    crate::migrate::run(conn, MIGRATIONS)
}

/// The baseline schema: the whole store as it stood before the version ladder,
/// probes and all, so any pre-versioning database converges to this shape on
/// its first run and stamps user_version 1. On a fresh database the CREATEs
/// build the current shape and the probe ALTERs are harmless no-ops. Do not add
/// new columns here; append a step to [`MIGRATIONS`] instead.
fn baseline(conn: &Connection) -> rusqlite::Result<()> {
    // Source-qualified identity per the components contract: local files are
    // the first source, streaming extensions add rows instead of migrations.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks (
            id           INTEGER PRIMARY KEY,
            source       TEXT NOT NULL DEFAULT 'local',
            path         TEXT NOT NULL,
            title        TEXT NOT NULL,
            artist       TEXT NOT NULL,
            album_artist TEXT NOT NULL DEFAULT '',
            album        TEXT NOT NULL,
            genre        TEXT NOT NULL,
            year         INTEGER NOT NULL,
            disc_no      INTEGER NOT NULL DEFAULT 0,
            track_no     INTEGER NOT NULL,
            duration_ms  INTEGER NOT NULL,
            codec        TEXT NOT NULL DEFAULT '',
            bitrate      INTEGER NOT NULL DEFAULT 0,
            rating       INTEGER NOT NULL DEFAULT 0,
            added        INTEGER NOT NULL DEFAULT 0,
            size         INTEGER NOT NULL,
            mtime        INTEGER NOT NULL,
            UNIQUE (source, path)
        );",
    )?;
    // A library from before the album artist column: add it, and reset
    // every mtime so the next scan re-reads tags instead of skipping the
    // files as unchanged, which would leave the column empty forever.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'album_artist'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN album_artist TEXT NOT NULL DEFAULT '';
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // Same move for a library from before codec and bitrate.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'codec'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN codec TEXT NOT NULL DEFAULT '';
             ALTER TABLE tracks ADD COLUMN bitrate INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // And for a library from before the disc number.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'disc_no'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN disc_no INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // And for a library from before ratings. No mtime reset here: the
    // rating is the app's own, never read from tags, so no rescan is owed.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'rating'")?;
    if !stmt.exists([])? {
        conn.execute_batch("ALTER TABLE tracks ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;")?;
    }
    // And for a library from before the added timestamp: add it and
    // backfill every row to now, so tracks scanned in after the upgrade
    // sort newer while the existing catalog clusters at the upgrade time.
    // No mtime reset: the timestamp is the app's own, never read from tags.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'added'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN added INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET added = CAST(strftime('%s', 'now') AS INTEGER);",
        )?;
    }
    // The listen events ride the same database and schema setup (ADR 11).
    crate::listens::init_schema(conn)?;
    // Playlists share the database too (ADR 16).
    crate::playlists::init_schema(conn)?;
    Ok(())
}

pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
}

/// The upsert's ReplayGain rule as one SQL condition, spelled into the
/// statement once per column it guards: the stored numbers are rox's own
/// measurement and the file being rescanned brought no gain to replace them
/// with, so the measurement stands. Tags always win where the file has any,
/// and a rescan that finds a tag-sourced gain gone still clears it: a stale
/// number would keep levelling a track by a measurement its file no longer
/// makes. The literal 1 is [`crate::replaygain::Source::Measured`]'s code,
/// which SQL cannot ask for; `measured_code_matches_the_sql` pins the two.
const KEEPS_MEASURED_GAIN: &str = "rg_source = 1
                    AND excluded.rg_track_gain IS NULL AND excluded.rg_album_gain IS NULL";

/// Insert or refresh one batch of local rows inside a single transaction. An
/// existing (source, path) row keeps its id, so projection db_ids stay valid
/// across a rescan. A re-read file's rating imports like any tag, except a
/// zero keeps the stored one: a rating the writer could not land in the
/// file (wav, read-only media) must not vanish because the file changed.
/// ReplayGain follows [`KEEPS_MEASURED_GAIN`]: tags overwrite anything,
/// including a measurement, and only a measured row survives a rescan that
/// found no tags.
pub fn insert_batch(conn: &mut Connection, rows: &[TrackRow]) -> rusqlite::Result<()> {
    // The scan time stamps first-seen rows only: the conflict update below
    // leaves `added` alone, so a rescan of an unchanged or edited file keeps
    // the moment it entered the library.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(&format!(
            "INSERT INTO tracks
             (path, title, artist, album_artist, album, genre, year, disc_no, track_no,
              duration_ms, codec, bitrate, sample_rate, bit_depth, rating, added, size, mtime,
              rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, rg_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20, ?21, ?22, 0)
             ON CONFLICT (source, path) DO UPDATE SET
                title = excluded.title, artist = excluded.artist,
                album_artist = excluded.album_artist,
                album = excluded.album, genre = excluded.genre,
                year = excluded.year, disc_no = excluded.disc_no,
                track_no = excluded.track_no,
                duration_ms = excluded.duration_ms, codec = excluded.codec,
                bitrate = excluded.bitrate,
                sample_rate = excluded.sample_rate, bit_depth = excluded.bit_depth,
                rating = CASE excluded.rating WHEN 0 THEN rating ELSE excluded.rating END,
                rg_track_gain = CASE WHEN {keep} THEN rg_track_gain
                    ELSE excluded.rg_track_gain END,
                rg_track_peak = CASE WHEN {keep} THEN rg_track_peak
                    ELSE excluded.rg_track_peak END,
                rg_album_gain = CASE WHEN {keep} THEN rg_album_gain
                    ELSE excluded.rg_album_gain END,
                rg_album_peak = CASE WHEN {keep} THEN rg_album_peak
                    ELSE excluded.rg_album_peak END,
                rg_source = CASE WHEN {keep} THEN rg_source ELSE 0 END,
                size = excluded.size,
                mtime = excluded.mtime",
            keep = KEEPS_MEASURED_GAIN,
        ))?;
        for r in rows {
            stmt.execute(rusqlite::params![
                r.path,
                r.title,
                r.artist,
                r.album_artist,
                r.album,
                r.genre,
                r.year,
                r.disc_no,
                r.track_no,
                r.duration_ms,
                r.codec,
                r.bitrate_kbps,
                r.sample_rate_hz,
                r.bit_depth,
                r.rating,
                now,
                r.size as i64,
                r.mtime,
                r.replay_gain.track_db,
                r.replay_gain.track_peak,
                r.replay_gain.album_db,
                r.replay_gain.album_peak,
            ])?;
        }
    }
    tx.commit()
}

/// The half-open path range holding exactly the files under `root`: from
/// the root plus a trailing separator up to the separator's successor
/// byte. SQLite compares TEXT bytewise, so the (source, path) index
/// serves range queries directly where a LIKE would not.
fn path_range(root: &Path) -> (String, String) {
    let mut lo = root.to_string_lossy().into_owned();
    if !lo.ends_with(std::path::MAIN_SEPARATOR) {
        lo.push(std::path::MAIN_SEPARATOR);
    }
    let mut hi = lo.clone();
    hi.pop();
    hi.push((std::path::MAIN_SEPARATOR as u8 + 1) as char);
    (lo, hi)
}

/// One scope's rollup: how many tracks and distinct albums it holds, what
/// its files weigh on disk, and how many folders hold them.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub tracks: u64,
    pub albums: u64,
    pub bytes: u64,
    /// Distinct parent folders of the local tracks: what a recursive
    /// filesystem watch would spend its per-directory watches on, so the
    /// watch ceiling reasons in this. Intermediate folders that hold only
    /// folders are not counted; callers wanting the true watch cost should
    /// leave headroom for them.
    pub dirs: u64,
}

/// The rollup columns behind [`Stats`]. Albums are distinct
/// (album_artist, album) pairs joined on the unit separator so the pair
/// never collides across the boundary; untagged tracks (empty album)
/// count no album, and the CASE's NULL keeps them out of the DISTINCT.
/// Dirs are distinct parents of the local rows: the nested replace empties
/// the path of both separators, and rtrim with that set eats the tail back
/// to the last separator, leaving the folder prefix. Non-local rows carry
/// no watchable folder, and the CASE's NULL keeps them out.
const STATS_COLUMNS: &str = "COUNT(*),
     COUNT(DISTINCT CASE WHEN album <> '' THEN album_artist || char(31) || album END),
     COALESCE(SUM(size), 0),
     COUNT(DISTINCT CASE WHEN source = 'local'
         THEN rtrim(path, replace(replace(path, '/', ''), '\\', '')) END)";

fn stats_row(r: &rusqlite::Row) -> rusqlite::Result<Stats> {
    Ok(Stats {
        tracks: r.get::<_, i64>(0)? as u64,
        albums: r.get::<_, i64>(1)? as u64,
        bytes: r.get::<_, i64>(2)? as u64,
        dirs: r.get::<_, i64>(3)? as u64,
    })
}

/// The whole library's rollup.
pub fn stats(conn: &Connection) -> rusqlite::Result<Stats> {
    conn.query_row(
        &format!("SELECT {STATS_COLUMNS} FROM tracks"),
        [],
        stats_row,
    )
}

/// The rollup for the local tracks under one folder.
pub fn stats_under(conn: &Connection, root: &Path) -> rusqlite::Result<Stats> {
    let (lo, hi) = path_range(root);
    conn.query_row(
        &format!(
            "SELECT {STATS_COLUMNS} FROM tracks
             WHERE source = 'local' AND path >= ?1 AND path < ?2"
        ),
        rusqlite::params![lo, hi],
        stats_row,
    )
}

/// The library's ReplayGain coverage split three ways. Every track lands in
/// exactly one bucket, so the three sum to the track count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GainCoverage {
    /// Tracks levelled by numbers their own file carried.
    pub tagged: u64,
    /// Tracks levelled by numbers rox measured.
    pub measured: u64,
    /// Tracks with no gain from either source. These play unlevelled, and
    /// they are what a measurement pass has left to do.
    pub missing: u64,
}

impl GainCoverage {
    /// Every track counted, whatever its source.
    pub fn total(self) -> u64 {
        self.tagged + self.measured + self.missing
    }

    /// Tracks with something to level by, whichever source wrote it. The
    /// honest answer to what turning leveling on will actually do.
    pub fn covered(self) -> u64 {
        self.tagged + self.measured
    }
}

/// The three-way coverage split, for a UI that distinguishes what a tagger
/// wrote from what rox measured. A row counts as covered on either gain -
/// the peaks bound a gain rather than being one - and a row marked measured
/// that somehow holds no gain counts as missing: the marker never invents a
/// number to level by.
pub fn replaygain_breakdown(conn: &Connection) -> rusqlite::Result<GainCoverage> {
    conn.query_row(
        "SELECT COUNT(CASE WHEN (rg_track_gain IS NOT NULL OR rg_album_gain IS NOT NULL)
                    AND COALESCE(rg_source, 0) <> 1 THEN 1 END),
                COUNT(CASE WHEN (rg_track_gain IS NOT NULL OR rg_album_gain IS NOT NULL)
                    AND COALESCE(rg_source, 0) = 1 THEN 1 END),
                COUNT(CASE WHEN rg_track_gain IS NULL AND rg_album_gain IS NULL THEN 1 END)
         FROM tracks",
        [],
        |row| {
            Ok(GainCoverage {
                tagged: row.get::<_, i64>(0)? as u64,
                measured: row.get::<_, i64>(1)? as u64,
                missing: row.get::<_, i64>(2)? as u64,
            })
        },
    )
}

/// One album's worth of work for the measurement pass: the files under it
/// that carry no track gain, grouped so an album gain can be measured over
/// the album as a unit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlbumToMeasure {
    /// The album's queue group id, the same (album artist, album) hash the
    /// player splices albums by. None for tracks with no album tag, which
    /// come back one entry per file: an untagged file is its own unit and
    /// has no album to average over.
    pub group: Option<u64>,
    pub album_artist: String,
    pub album: String,
    /// The untagged files, in disc and track order so a pass that reports
    /// progress walks the album the way it plays.
    pub paths: Vec<String>,
    /// How many local tracks the album holds in all, the tagged ones
    /// included. An album gain only means something measured over the whole
    /// album, so a caller with fewer paths than this measures track gains
    /// and leaves the album figures to the files that already have them.
    pub total: usize,
}

/// Every local track with no track gain from either source, grouped into the
/// albums a measurement pass would take one at a time. Ordered by album, then
/// by disc and track within it, so the work comes back in a stable order run
/// to run. A track carrying only an album gain still counts as missing: album
/// mode has something to level it by, track mode is only borrowing.
pub fn albums_missing_replaygain(conn: &Connection) -> rusqlite::Result<Vec<AlbumToMeasure>> {
    let mut stmt = conn.prepare(
        "SELECT t.album_artist, t.album, t.path, g.total
         FROM tracks t
         JOIN (SELECT album_artist, album, COUNT(*) AS total FROM tracks
               WHERE source = 'local' GROUP BY album_artist, album) g
           ON g.album_artist = t.album_artist AND g.album = t.album
         WHERE t.source = 'local' AND t.rg_track_gain IS NULL
         ORDER BY t.album_artist, t.album, t.disc_no, t.track_no, t.path",
    )?;
    let mut rows = stmt.query([])?;
    let mut out: Vec<AlbumToMeasure> = Vec::new();
    while let Some(row) = rows.next()? {
        let album_artist: String = row.get(0)?;
        let album: String = row.get(1)?;
        let path: String = row.get(2)?;
        let total = row.get::<_, i64>(3)? as usize;
        let group = crate::hash::album_group(&album_artist, &album);
        // An untagged file groups with nobody, so it lands as its own entry
        // of one rather than pooling with every other album-less track.
        let append = group.is_some()
            && out
                .last()
                .is_some_and(|last| last.album_artist == album_artist && last.album == album);
        if append {
            let last = out.last_mut().expect("append implies a last entry");
            last.paths.push(path);
        } else {
            out.push(AlbumToMeasure {
                group,
                album_artist,
                album,
                paths: vec![path],
                total: if group.is_some() { total } else { 1 },
            });
        }
    }
    Ok(out)
}

/// Write one measurement pass's numbers onto the rows it measured, marked as
/// rox's own so a later rescan knows not to clear them. One transaction for
/// the batch, which is how the pass lands an album: every file in it gets the
/// album figures alongside its own track ones.
///
/// A None field leaves its column alone, so a pass that measured only track
/// gains does not wipe an album gain the files already carried. Rows that
/// picked up a track gain from tags since [`albums_missing_replaygain`] listed
/// them are skipped: tags win over a measurement that lost the race. Returns
/// how many rows actually took the write.
pub fn set_measured_replaygain(
    conn: &mut Connection,
    measured: &[(&str, crate::replaygain::ReplayGain)],
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let mut written = 0;
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE tracks SET
                rg_track_gain = COALESCE(?2, rg_track_gain),
                rg_track_peak = COALESCE(?3, rg_track_peak),
                rg_album_gain = COALESCE(?4, rg_album_gain),
                rg_album_peak = COALESCE(?5, rg_album_peak),
                rg_source = ?6
             WHERE source = 'local' AND path = ?1
               AND (rg_source = ?6 OR rg_track_gain IS NULL)",
        )?;
        for (path, gain) in measured {
            written += stmt.execute(rusqlite::params![
                path,
                gain.track_db,
                gain.track_peak,
                gain.album_db,
                gain.album_peak,
                crate::replaygain::Source::Measured.code(),
            ])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// Drop every local track under one folder, for when it leaves the
/// library. The files themselves are untouched.
pub fn remove_under(conn: &Connection, root: &Path) -> rusqlite::Result<usize> {
    let (lo, hi) = path_range(root);
    conn.execute(
        "DELETE FROM tracks WHERE source = 'local' AND path >= ?1 AND path < ?2",
        rusqlite::params![lo, hi],
    )
}

/// Drop the row for one path and, if it was a directory, every row beneath
/// it, for a file or folder the watcher saw deleted. This is the delete that
/// never walks the disk: the vanished path is already the range, so a removed
/// artist folder is one bytewise sweep over the index, not a rescan of what
/// is left. Returns the number of rows removed.
pub fn remove_subtree(conn: &Connection, path: &Path) -> rusqlite::Result<usize> {
    let exact = path.to_string_lossy();
    let (lo, hi) = path_range(path);
    conn.execute(
        "DELETE FROM tracks
         WHERE source = 'local' AND (path = ?1 OR (path >= ?2 AND path < ?3))",
        rusqlite::params![exact, lo, hi],
    )
}

/// Move the row for one path and, if it was a directory, every row beneath
/// it, for a file or folder the watcher saw renamed. Ids stay put, so the
/// `added` timestamp, the db-only rating, and the playlist and listen joins
/// all ride along instead of dying with the old path and landing fresh on
/// the new one. Like the delete, this never walks the disk: the old path is
/// already the range, so a renamed artist folder is one bytewise prefix
/// rewrite over the index. Returns the number of rows moved.
pub fn rename_within(conn: &mut Connection, from: &Path, to: &Path) -> rusqlite::Result<usize> {
    let from_exact = from.to_string_lossy().into_owned();
    let to_exact = to.to_string_lossy().into_owned();
    let (lo, hi) = path_range(from);
    // The rows to move: the exact path and its subtree, collected first so the
    // rewrite runs off a plain list, not a live cursor over the table.
    let moving: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, path FROM tracks
             WHERE source = 'local' AND (path = ?1 OR (path >= ?2 AND path < ?3))",
        )?;
        let rows = stmt.query_map(rusqlite::params![from_exact, lo, hi], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.filter_map(Result::ok).collect()
    };
    if moving.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    {
        let mut upd = tx.prepare_cached("UPDATE tracks SET path = ?2 WHERE id = ?1")?;
        for (id, old) in &moving {
            // Swap the `from` prefix for `to`, bytewise; the exact-path row
            // has no suffix, a subtree row keeps its remainder under the new
            // root.
            let rest = &old[from_exact.len()..];
            let new = format!("{to_exact}{rest}");
            upd.execute(rusqlite::params![id, new])?;
        }
    }
    tx.commit()?;
    Ok(moving.len())
}

/// Drop the local rows under `root` whose path is not in `present`, the set
/// of files the walk actually found on disk. This is the rescan's delete
/// half: a file removed from disk loses its row so the rebuilt projection
/// drops it. Rows outside `root` are untouched, so scanning one folder never
/// prunes another's. The listen history and playlist entries keep their own
/// snapshot columns and only lose the join back to the track, by design.
/// Returns the number of rows removed.
pub fn prune_missing(
    conn: &mut Connection,
    root: &Path,
    present: &std::collections::HashSet<String>,
) -> rusqlite::Result<usize> {
    let (lo, hi) = path_range(root);
    // The stored paths under root the walk did not find. Collected first so
    // the delete runs off a plain list, not a live cursor over the table.
    let gone: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT path FROM tracks WHERE source = 'local' AND path >= ?1 AND path < ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![lo, hi], |r| r.get::<_, String>(0))?;
        rows.filter_map(Result::ok)
            .filter(|path| !present.contains(path))
            .collect()
    };
    if gone.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    {
        let mut del =
            tx.prepare_cached("DELETE FROM tracks WHERE source = 'local' AND path = ?1")?;
        for path in &gone {
            del.execute([path])?;
        }
    }
    tx.commit()?;
    Ok(gone.len())
}

/// Every local path with its (mtime, size), so a rescan can skip files that
/// have not changed without reading their tags.
pub fn local_files(conn: &Connection) -> rusqlite::Result<HashMap<String, (i64, u64)>> {
    let mut stmt = conn.prepare("SELECT path, mtime, size FROM tracks WHERE source = 'local'")?;
    let mut rows = stmt.query([])?;
    let mut out = HashMap::new();
    while let Some(row) = rows.next()? {
        out.insert(row.get(0)?, (row.get(1)?, row.get::<_, i64>(2)? as u64));
    }
    Ok(out)
}

/// The deepest directory holding every local track, for recovering the scan
/// root from a library indexed before anything recorded it. None on an
/// empty library.
pub fn common_root(conn: &Connection) -> rusqlite::Result<Option<PathBuf>> {
    let mut stmt = conn.prepare("SELECT path FROM tracks WHERE source = 'local'")?;
    let mut rows = stmt.query([])?;
    let mut root: Option<PathBuf> = None;
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let Some(dir) = Path::new(&path).parent() else {
            continue;
        };
        root = Some(match root {
            None => dir.to_path_buf(),
            Some(root) => root
                .components()
                .zip(dir.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    Ok(root.filter(|root| root.parent().is_some()))
}

/// Apply one file's committed tag changes to its row, so the projection
/// can reload the edit without a rescan. Only the columns the library
/// projects move; comment, composer, and custom fields have no column
/// and fall through. The stored mtime stays put on purpose: the write
/// bumped the file's, so the next rescan re-reads it and squares the
/// row with the tag wholesale.
pub fn apply_changes(
    conn: &Connection,
    id: i64,
    changes: &[crate::writer::Change],
) -> rusqlite::Result<()> {
    use crate::writer::Field;
    // The leading digits of a tag value: a "2020-05-01" date and a
    // "5/12" track fraction both reduce to the number the column holds,
    // the scanner's read of the same tags.
    fn leading(value: &str) -> i64 {
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
        // The projection reads these columns back as u16; saturate so an
        // oversized value cannot truncate to garbage.
        digits.parse().unwrap_or(0).min(i64::from(u16::MAX))
    }
    for change in changes {
        let value = change.value.as_deref().unwrap_or("");
        // The rating speaks the writer's 0-10 display number, not the
        // column's 0-100; a cleared or unparseable one lands as unrated.
        if change.field == Field::Rating {
            let rating = crate::rating::parse_display(value).unwrap_or(0);
            conn.execute(
                "UPDATE tracks SET rating = ?2 WHERE id = ?1",
                rusqlite::params![id, rating],
            )?;
            continue;
        }
        let (column, number) = match &change.field {
            Field::Title => ("title", false),
            Field::Artist => ("artist", false),
            Field::Album => ("album", false),
            Field::AlbumArtist => ("album_artist", false),
            Field::Genre => ("genre", false),
            Field::Year => ("year", true),
            Field::TrackNo => ("track_no", true),
            Field::DiscNo => ("disc_no", true),
            _ => continue,
        };
        if number {
            conn.execute(
                &format!("UPDATE tracks SET {column} = ?2 WHERE id = ?1"),
                rusqlite::params![id, leading(value)],
            )?;
        } else if column == "album_artist" && value.is_empty() {
            // A cleared album artist falls back to the track artist, the
            // scanner's grouping rule.
            conn.execute(
                "UPDATE tracks SET album_artist = artist WHERE id = ?1",
                rusqlite::params![id],
            )?;
        } else {
            conn.execute(
                &format!("UPDATE tracks SET {column} = ?2 WHERE id = ?1"),
                rusqlite::params![id, value],
            )?;
        }
    }
    Ok(())
}

/// One track's rating onto its row: the app's 0-100 scale, 0 unrated.
/// Ratings live in the library alone, never in the file's tags, so this
/// touches no mtime and owes no rescan.
pub fn set_rating(conn: &Connection, id: i64, rating: u8) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET rating = ?2 WHERE id = ?1",
        rusqlite::params![id, rating],
    )?;
    Ok(())
}

/// Resolve projection db_ids back to playable paths, in the order given.
pub fn paths_for(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT path FROM tracks WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Ok(path) = stmt.query_row([id], |r| r.get::<_, String>(0)) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Every local track's id, artist, and title, for a caller matching outside
/// names against the library. One walk of the table: the loved-tracks import
/// folds this into its own lookup and asks it thousands of questions, which
/// is a query each the other way around.
pub fn name_index(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, artist, title FROM tracks
          WHERE source = 'local' AND artist <> '' AND title <> ''",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

/// Resolve track ids to the artist and title an online service names them
/// by, skipping ids the library no longer holds. A track missing either tag
/// is skipped too: nothing downstream can name it, so an empty pair would
/// only travel to be thrown away.
pub fn names_for(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached("SELECT artist, title FROM tracks WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Ok((artist, title)) = stmt.query_row([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            if !artist.is_empty() && !title.is_empty() {
                out.push((artist, title));
            }
        }
    }
    Ok(out)
}

/// What the player reads off a row before handing the path to the engine,
/// which knows nothing but paths: the album group boundaries are decided by
/// (ADR 17) and the ReplayGain the source is levelled with (ADR 19).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QueueMeta {
    /// The library row this path resolved to, None for a file from outside
    /// the library. Continuation (ADR 17) keeps these so it can tell a
    /// provider what the session has already played, and it comes back from
    /// this lookup rather than its own so queueing a track costs one query
    /// instead of two.
    pub id: Option<i64>,
    pub group: Option<u64>,
    pub replay_gain: crate::replaygain::ReplayGain,
}

/// Resolve a playable path to its [`QueueMeta`]. Everything defaults when
/// the path is not in the library, which plays as ungrouped and untagged:
/// a file dropped on the player from outside still plays, it just has
/// nothing to level or splice by. The group is None for a track with no
/// album tag too, since ungrouped entries never claim album adjacency.
pub fn queue_meta_for_path(conn: &Connection, path: &str) -> rusqlite::Result<QueueMeta> {
    let mut stmt = conn.prepare_cached(
        "SELECT album_artist, album, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak, id
         FROM tracks WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => {
            let album_artist: String = row.get(0)?;
            let album: String = row.get(1)?;
            Ok(QueueMeta {
                id: row.get(6)?,
                group: crate::hash::album_group(&album_artist, &album),
                replay_gain: crate::replaygain::ReplayGain {
                    track_db: row.get(2)?,
                    track_peak: row.get(3)?,
                    album_db: row.get(4)?,
                    album_peak: row.get(5)?,
                },
            })
        }
        None => Ok(QueueMeta::default()),
    }
}

/// Every local track id in the canonical browse order, the same one
/// [`crate::listens::never_played`] reads in: album artist, album, disc,
/// track. What a continuation provider (ADR 17) draws its pool from.
///
/// The whole column rather than a page, because the providers that want it
/// want all of it: resuming a browse order has to find where the session got
/// to, and tiering by history has to weigh every candidate before it picks.
/// It's eight bytes a track off an index-ordered scan, so a hundred-thousand
/// track library is under a megabyte and a fraction of what one batch of
/// scoring already costs.
pub fn all_ids(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM tracks WHERE source = 'local'
         ORDER BY album_artist, album, disc_no, track_no, title",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

/// Resolve a playable path to its track id, for marking the playing row.
/// Ok(None) when the path is not in the library.
pub fn id_for_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM tracks WHERE source = 'local' AND path = ?1")?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// The display tags for one track, what a path-keyed lookup returns.
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: u16,
    /// The album grouping and column metadata, for the queue's headings and
    /// columns; the other callers read only the tags above.
    pub album_artist: String,
    pub year: u16,
    pub genre: String,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub rating: u8,
}

/// Resolve a playable path back to its tags, for showing what is playing.
/// Ok(None) when the path is not in the library.
pub fn meta_for_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<TrackMeta>> {
    let mut stmt = conn.prepare_cached(
        "SELECT title, artist, album, track_no,
                album_artist, year, genre, duration_ms, codec, bitrate,
                sample_rate, bit_depth, rating
         FROM tracks
         WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(TrackMeta {
            title: row.get(0)?,
            artist: row.get(1)?,
            album: row.get(2)?,
            track_no: row.get::<_, i64>(3)? as u16,
            album_artist: row.get(4)?,
            year: row.get(5)?,
            genre: row.get(6)?,
            duration_ms: row.get(7)?,
            codec: row.get(8)?,
            bitrate_kbps: row.get(9)?,
            sample_rate_hz: row.get(10)?,
            bit_depth: row.get(11)?,
            rating: row.get(12)?,
        })),
        None => Ok(None),
    }
}

/// Resolve a path to its track id and tags in one query, for callers that want
/// both. Ok(None) when the path is not in the library. Saves the round trip of
/// calling [`id_for_path`] and [`meta_for_path`] back to back on the same path.
pub fn meta_row_for_path(
    conn: &Connection,
    path: &str,
) -> rusqlite::Result<Option<(i64, TrackMeta)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, album, track_no,
                album_artist, year, genre, duration_ms, codec, bitrate,
                sample_rate, bit_depth, rating
         FROM tracks
         WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some((
            row.get(0)?,
            TrackMeta {
                title: row.get(1)?,
                artist: row.get(2)?,
                album: row.get(3)?,
                track_no: row.get::<_, i64>(4)? as u16,
                album_artist: row.get(5)?,
                year: row.get(6)?,
                genre: row.get(7)?,
                duration_ms: row.get(8)?,
                codec: row.get(9)?,
                bitrate_kbps: row.get(10)?,
                sample_rate_hz: row.get(11)?,
                bit_depth: row.get(12)?,
                rating: row.get(13)?,
            },
        ))),
        None => Ok(None),
    }
}

pub fn max_rowid(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tracks", [], |r| r.get(0))
}

/// Stream the projection columns for one rowid range, in id order. The
/// sink's argument order mirrors the SELECT: path, title, artist, album
/// artist, album, genre, then codec and the stream numbers after the tag
/// numbers, the rating and scan time, then the two ReplayGain figures. The
/// path rides so the projection can derive each track's folder.
#[allow(clippy::type_complexity)]
pub fn scan_range(
    conn: &Connection,
    lo: i64,
    hi: i64,
    mut sink: impl FnMut(
        i64,
        &str,
        &str,
        &str,
        &str,
        &str,
        &str,
        u16,
        u16,
        u16,
        u32,
        &str,
        u16,
        u32,
        u8,
        u8,
        i64,
        Option<f32>,
        Option<f32>,
    ),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path, title, artist, album_artist, album, genre, year, disc_no, track_no,
                duration_ms, codec, bitrate, sample_rate, bit_depth, rating, added,
                rg_track_gain, rg_album_gain
         FROM tracks WHERE id > ?1 AND id <= ?2 ORDER BY id",
    )?;
    let mut rows = stmt.query(rusqlite::params![lo, hi])?;
    while let Some(row) = rows.next()? {
        sink(
            row.get(0)?,
            row.get_ref(1)?.as_str().unwrap_or(""),
            row.get_ref(2)?.as_str().unwrap_or(""),
            row.get_ref(3)?.as_str().unwrap_or(""),
            row.get_ref(4)?.as_str().unwrap_or(""),
            row.get_ref(5)?.as_str().unwrap_or(""),
            row.get_ref(6)?.as_str().unwrap_or(""),
            row.get::<_, i64>(7)? as u16,
            row.get::<_, i64>(8)? as u16,
            row.get::<_, i64>(9)? as u16,
            row.get::<_, i64>(10)? as u32,
            row.get_ref(11)?.as_str().unwrap_or(""),
            row.get::<_, i64>(12)? as u16,
            row.get::<_, i64>(13)? as u32,
            row.get::<_, i64>(14)? as u8,
            row.get::<_, i64>(15)? as u8,
            row.get::<_, i64>(16)?,
            row.get(17)?,
            row.get(18)?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, album_artist: &str, album: &str, size: u64) -> TrackRow {
        TrackRow {
            path: path.into(),
            title: String::new(),
            artist: String::new(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            size,
            mtime: 0,
        }
    }

    #[test]
    fn baseline_converges_an_old_database_and_stamps_the_version() {
        // A tracks table from before the version ladder: the earliest shape,
        // missing album_artist and every column added since, and sitting at
        // user_version 0 the way any pre-versioning file does.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tracks (
                id          INTEGER PRIMARY KEY,
                source      TEXT NOT NULL DEFAULT 'local',
                path        TEXT NOT NULL,
                title       TEXT NOT NULL,
                artist      TEXT NOT NULL,
                album       TEXT NOT NULL,
                genre       TEXT NOT NULL,
                year        INTEGER NOT NULL,
                track_no    INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                size        INTEGER NOT NULL,
                mtime       INTEGER NOT NULL,
                UNIQUE (source, path)
            );
            INSERT INTO tracks (path, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
                VALUES ('/m/1.mp3', 'One', 'A', 'First', 'rock', 0, 1, 0, 10, 5);",
        )
        .unwrap();

        init_schema(&conn).unwrap();

        // Every column the baseline probes for is present now, the pre-existing
        // row survived, and the file is stamped at the head of the ladder.
        let (album_artist, added): (String, i64) = conn
            .query_row(
                "SELECT album_artist, added FROM tracks WHERE path = '/m/1.mp3'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(album_artist, "", "the added column defaults empty");
        assert!(added > 0, "the added backfill stamped the old row");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        // The head of the ladder, whatever its length: a new rung raises
        // this on its own rather than failing an unrelated test.
        assert_eq!(version, MIGRATIONS.len() as i64);

        // A second open is a no-op: the baseline never re-probes a stamped file.
        init_schema(&conn).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
    }

    /// The snapshot-paths step backfills the new path columns from the live
    /// catalog, so playlists and history made before the column carry the
    /// reattach key without a re-add.
    #[test]
    fn snapshot_paths_backfill_from_the_live_catalog() {
        let conn = Connection::open_in_memory().unwrap();
        // Stop the ladder at the baseline: the pre-step-2 shape, member and
        // listen tables without a path column.
        crate::migrate::run(&conn, &MIGRATIONS[..1]).unwrap();
        // Written as plain SQL against the baseline's own columns, not
        // through insert_batch: that write path targets the head of the
        // ladder, so it would need columns this fixture deliberately
        // stops short of.
        conn.execute(
            "INSERT INTO tracks (path, title, artist, album_artist, album, genre, year,
                track_no, duration_ms, size, mtime)
             VALUES ('/m/a/1.mp3', '', 'X', 'X', 'Album', '', 0, 1, 0, 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO playlists (name, created, updated) VALUES ('Mix', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, title, artist, album)
             VALUES (1, 1, 0, '', 'X', 'Album')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO listens (track_id, played_at, title, artist, album, genre)
             VALUES (1, 100, '', 'X', 'Album', '')",
            [],
        )
        .unwrap();

        init_schema(&conn).unwrap();

        let member: String = conn
            .query_row("SELECT path FROM playlist_tracks", [], |r| r.get(0))
            .unwrap();
        let listen: String = conn
            .query_row("SELECT path FROM listens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(member, "/m/a/1.mp3");
        assert_eq!(listen, "/m/a/1.mp3");
    }

    /// The stream-format step adds the two columns and resets every mtime,
    /// which is the half that matters: without it the next scan skips the
    /// unchanged files and the columns never fill.
    #[test]
    fn stream_format_step_adds_columns_and_asks_for_a_rescan() {
        let conn = Connection::open_in_memory().unwrap();
        // The rung under test, found by name: a later rung that also resets
        // mtime would otherwise quietly take credit for these assertions.
        let rung = MIGRATIONS
            .iter()
            .position(|m| m.name == "stream-format")
            .expect("the stream-format rung is part of the ladder");
        // The ladder up to but not including it, then a row stamped as
        // already scanned.
        crate::migrate::run(&conn, &MIGRATIONS[..rung]).unwrap();
        conn.execute(
            "INSERT INTO tracks (path, title, artist, album_artist, album, genre, year,
                track_no, duration_ms, codec, bitrate, size, mtime)
             VALUES ('/m/1.flac', 'One', 'A', 'A', 'Album', '', 0, 1, 0, 'flac', 1006, 10, 500)",
            [],
        )
        .unwrap();

        // That one rung and nothing after it.
        crate::migrate::run(&conn, &MIGRATIONS[..=rung]).unwrap();

        let (rate, depth, mtime): (i64, i64, i64) = conn
            .query_row(
                "SELECT sample_rate, bit_depth, mtime FROM tracks WHERE path = '/m/1.flac'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((rate, depth), (0, 0), "the columns default unread");
        assert_eq!(mtime, 0, "the row is owed a re-read");
    }

    /// The replaygain-source step adds its column reading as tag-sourced and,
    /// unlike the rungs that add tag columns, leaves every mtime where it is:
    /// nothing about the marker needs a file reopened.
    #[test]
    fn replaygain_source_step_adds_a_column_without_asking_for_a_rescan() {
        let conn = Connection::open_in_memory().unwrap();
        let rung = MIGRATIONS
            .iter()
            .position(|m| m.name == "replaygain-source")
            .expect("the replaygain-source rung is part of the ladder");
        crate::migrate::run(&conn, &MIGRATIONS[..rung]).unwrap();
        conn.execute(
            "INSERT INTO tracks (path, title, artist, album_artist, album, genre, year,
                track_no, duration_ms, codec, bitrate, sample_rate, bit_depth, size, mtime,
                rg_track_gain)
             VALUES ('/m/1.flac', 'One', 'A', 'A', 'Album', '', 0, 1, 0, 'flac', 1006,
                     44100, 16, 10, 500, -7.35)",
            [],
        )
        .unwrap();

        crate::migrate::run(&conn, &MIGRATIONS[..=rung]).unwrap();

        let (source, mtime): (Option<i64>, i64) = conn
            .query_row(
                "SELECT rg_source, mtime FROM tracks WHERE path = '/m/1.flac'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, None, "an existing row's gain came off its tags");
        assert_eq!(
            crate::replaygain::Source::from_code(source),
            crate::replaygain::Source::Tags
        );
        assert_eq!(mtime, 500, "no file is owed a re-read for this column");
    }

    /// The SQL in [`KEEPS_MEASURED_GAIN`] and the upsert spells the measured
    /// code as a literal, since a CASE cannot call a method. This is the pin.
    #[test]
    fn measured_code_matches_the_sql() {
        use crate::replaygain::Source;
        assert_eq!(Source::Measured.code(), 1);
        assert_eq!(Source::Tags.code(), 0);
    }

    #[test]
    fn stats_roll_up_tracks_albums_and_bytes() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(
            &mut conn,
            &[
                // One album twice, the same title under another artist,
                // an untagged track, and one outside the folder.
                row("/m/a/1.mp3", "X", "Album", 100),
                row("/m/a/2.mp3", "X", "Album", 200),
                row("/m/b/1.mp3", "Y", "Album", 300),
                row("/m/c/1.mp3", "Z", "", 50),
                row("/n/d/1.mp3", "W", "Other", 400),
            ],
        )
        .unwrap();

        let whole = stats(&conn).unwrap();
        assert_eq!(
            (whole.tracks, whole.albums, whole.bytes, whole.dirs),
            (5, 3, 1050, 4),
            "an empty album tag counts no album; two tracks share one folder"
        );

        let under = stats_under(&conn, Path::new("/m")).unwrap();
        assert_eq!(
            (under.tracks, under.albums, under.bytes, under.dirs),
            (4, 2, 650, 3)
        );
    }

    /// A rating lands on the row and a rescan's upsert leaves it alone,
    /// since ratings are the app's own and never come back from tags.
    #[test]
    fn ratings_survive_a_rescan() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let track = || row("/m/a/1.mp3", "X", "Album", 100);
        insert_batch(&mut conn, &[track()]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        set_rating(&conn, id, 75).unwrap();
        insert_batch(&mut conn, &[track()]).unwrap();

        let p = crate::projection::Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.resolve(0).rating, 75);
    }

    /// ReplayGain rides the row and comes back through the one lookup the
    /// player makes per queued path, with an untagged file's fields still
    /// None rather than zero: the two mean different things at play time.
    #[test]
    fn replaygain_round_trips_to_the_queue_lookup() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let rg = crate::replaygain::ReplayGain {
            track_db: Some(-7.35),
            track_peak: Some(0.98),
            album_db: Some(-8.1),
            album_peak: None,
        };
        let tagged = || {
            let mut row = row("/m/a/1.mp3", "X", "Album", 100);
            row.replay_gain = rg;
            row
        };
        insert_batch(&mut conn, &[tagged(), row("/m/a/2.mp3", "X", "Album", 100)]).unwrap();

        let meta = queue_meta_for_path(&conn, "/m/a/1.mp3").unwrap();
        assert_eq!(meta.replay_gain, rg);
        assert_eq!(meta.group, crate::hash::album_group("X", "Album"));
        // Same album, no tags of its own: nothing invented for it.
        let untagged = queue_meta_for_path(&conn, "/m/a/2.mp3").unwrap();
        assert_eq!(untagged.replay_gain, Default::default());
        assert_eq!(untagged.group, meta.group);
        // A path the library has never seen plays ungrouped and unlevelled.
        assert_eq!(
            queue_meta_for_path(&conn, "/elsewhere/3.mp3").unwrap(),
            QueueMeta::default()
        );

        // Coverage counts the one tagged file of the two.
        let split = replaygain_breakdown(&conn).unwrap();
        assert_eq!((split.covered(), split.total()), (1, 2));

        // A rescan that finds the tags gone clears them: a stale gain would
        // keep levelling a track by a measurement the file no longer makes.
        insert_batch(&mut conn, &[row("/m/a/1.mp3", "X", "Album", 100)]).unwrap();
        let cleared = queue_meta_for_path(&conn, "/m/a/1.mp3").unwrap();
        assert_eq!(cleared.replay_gain, Default::default());
        let split = replaygain_breakdown(&conn).unwrap();
        assert_eq!((split.covered(), split.total()), (0, 2));
    }

    /// A file carrying only an album gain still counts as covered: album
    /// mode has something to level it by, and track mode falls back to it.
    #[test]
    fn coverage_counts_an_album_only_tagging() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut album_only = row("/m/a/1.mp3", "X", "Album", 100);
        album_only.replay_gain = crate::replaygain::ReplayGain {
            album_db: Some(-8.1),
            ..Default::default()
        };
        insert_batch(&mut conn, &[album_only]).unwrap();
        let split = replaygain_breakdown(&conn).unwrap();
        assert_eq!((split.covered(), split.total()), (1, 1));
    }

    /// What the store says a row's gain came from.
    fn gain_source(conn: &Connection, path: &str) -> crate::replaygain::Source {
        let code: Option<i64> = conn
            .query_row(
                "SELECT rg_source FROM tracks WHERE path = ?1",
                [path],
                |r| r.get(0),
            )
            .unwrap();
        crate::replaygain::Source::from_code(code)
    }

    /// The measurement pass writes its numbers onto the rows it measured and
    /// a rescan that finds the files still untagged leaves them alone. The
    /// tag-sourced half of the rule is unchanged: those still clear.
    #[test]
    fn a_rescan_without_tags_keeps_a_measured_gain() {
        use crate::replaygain::{ReplayGain, Source};
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let untagged = || row("/m/a/1.mp3", "X", "Album", 100);
        let tagged = || {
            let mut r = row("/m/a/2.mp3", "X", "Album", 100);
            r.replay_gain = ReplayGain {
                track_db: Some(-4.0),
                ..Default::default()
            };
            r
        };
        insert_batch(&mut conn, &[untagged(), tagged()]).unwrap();

        let measured = ReplayGain {
            track_db: Some(-6.5),
            track_peak: Some(0.91),
            album_db: Some(-6.0),
            album_peak: Some(0.99),
        };
        assert_eq!(
            set_measured_replaygain(&mut conn, &[("/m/a/1.mp3", measured)]).unwrap(),
            1
        );
        assert_eq!(
            queue_meta_for_path(&conn, "/m/a/1.mp3")
                .unwrap()
                .replay_gain,
            measured
        );
        assert_eq!(gain_source(&conn, "/m/a/1.mp3"), Source::Measured);

        // The rescan: neither file gained tags, and the second one lost the
        // tags it had.
        insert_batch(
            &mut conn,
            &[untagged(), row("/m/a/2.mp3", "X", "Album", 100)],
        )
        .unwrap();
        assert_eq!(
            queue_meta_for_path(&conn, "/m/a/1.mp3")
                .unwrap()
                .replay_gain,
            measured,
            "a measurement is rox's own, not the file's to clear"
        );
        assert_eq!(gain_source(&conn, "/m/a/1.mp3"), Source::Measured);
        assert_eq!(
            queue_meta_for_path(&conn, "/m/a/2.mp3")
                .unwrap()
                .replay_gain,
            ReplayGain::default(),
            "a tag-sourced gain still clears when the tags go"
        );
    }

    /// The other direction: a rescan that finds real tags overwrites a
    /// measurement and hands the row back to the tags, marker and all.
    #[test]
    fn tags_overwrite_a_measured_gain() {
        use crate::replaygain::{ReplayGain, Source};
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(&mut conn, &[row("/m/a/1.mp3", "X", "Album", 100)]).unwrap();
        set_measured_replaygain(
            &mut conn,
            &[(
                "/m/a/1.mp3",
                ReplayGain {
                    track_db: Some(-6.5),
                    track_peak: Some(0.91),
                    album_db: Some(-6.0),
                    album_peak: Some(0.99),
                },
            )],
        )
        .unwrap();

        // Somebody tagged the file and the rescan read it: only a track gain
        // and peak, so the measured album figures go with the rest.
        let from_tags = ReplayGain {
            track_db: Some(-8.25),
            track_peak: Some(1.01),
            ..Default::default()
        };
        let mut rescanned = row("/m/a/1.mp3", "X", "Album", 100);
        rescanned.replay_gain = from_tags;
        insert_batch(&mut conn, &[rescanned]).unwrap();

        assert_eq!(
            queue_meta_for_path(&conn, "/m/a/1.mp3")
                .unwrap()
                .replay_gain,
            from_tags,
            "tags win outright, measured leftovers included"
        );
        assert_eq!(gain_source(&conn, "/m/a/1.mp3"), Source::Tags);

        // And the measurement pass no longer writes over them.
        assert_eq!(
            set_measured_replaygain(
                &mut conn,
                &[(
                    "/m/a/1.mp3",
                    ReplayGain {
                        track_db: Some(-3.0),
                        ..Default::default()
                    }
                )]
            )
            .unwrap(),
            0,
            "a row the tags reached first is left to them"
        );
        assert_eq!(
            queue_meta_for_path(&conn, "/m/a/1.mp3")
                .unwrap()
                .replay_gain,
            from_tags
        );
    }

    /// The work list: untagged files grouped into their albums, in disc and
    /// track order, with the album's full size beside them so a caller knows
    /// whether it can measure an album gain at all. Album-less files come
    /// back one entry each.
    #[test]
    fn albums_missing_replaygain_groups_the_work_by_album() {
        use crate::replaygain::ReplayGain;
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let track = |path: &str, album: &str, no: u16| {
            let mut r = row(path, "X", album, 100);
            r.track_no = no;
            r
        };
        let tagged = |path: &str, album: &str, no: u16| {
            let mut r = track(path, album, no);
            r.replay_gain = ReplayGain {
                track_db: Some(-7.0),
                ..Default::default()
            };
            r
        };
        insert_batch(
            &mut conn,
            &[
                // A part-tagged album: two files to measure out of three.
                track("/m/a/2.mp3", "Album", 2),
                tagged("/m/a/1.mp3", "Album", 1),
                track("/m/a/3.mp3", "Album", 3),
                // A fully tagged album, which is no work at all.
                tagged("/m/b/1.mp3", "Done", 1),
                // Two files with no album tag, each its own unit.
                track("/m/c/1.mp3", "", 0),
                track("/m/c/2.mp3", "", 0),
            ],
        )
        .unwrap();

        let work = albums_missing_replaygain(&conn).unwrap();
        assert_eq!(work.len(), 3, "one album plus the two loose files");
        let album = work
            .iter()
            .find(|a| a.album == "Album")
            .expect("the part-tagged album is work");
        assert_eq!(album.group, crate::hash::album_group("X", "Album"));
        assert_eq!(
            album.paths,
            ["/m/a/2.mp3", "/m/a/3.mp3"],
            "the tagged track is not work, and the rest come in track order"
        );
        assert_eq!(
            album.total, 3,
            "the album is bigger than the work, so no album gain to measure"
        );
        assert!(!work.iter().any(|a| a.album == "Done"));
        for entry in work.iter().filter(|a| a.album.is_empty()) {
            assert_eq!(entry.group, None);
            assert_eq!(entry.paths.len(), 1);
            assert_eq!(entry.total, 1, "an untagged file is an album of one");
        }

        // Measuring the album's remaining files takes them off the list.
        let measured = ReplayGain {
            track_db: Some(-5.0),
            ..Default::default()
        };
        assert_eq!(
            set_measured_replaygain(
                &mut conn,
                &[("/m/a/2.mp3", measured), ("/m/a/3.mp3", measured)]
            )
            .unwrap(),
            2
        );
        let work = albums_missing_replaygain(&conn).unwrap();
        assert!(!work.iter().any(|a| a.album == "Album"));
    }

    /// The three-way split the settings page reads: what a tagger wrote,
    /// what rox measured, and what nothing levels yet.
    #[test]
    fn coverage_splits_tagged_from_measured() {
        use crate::replaygain::ReplayGain;
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut from_tags = row("/m/a/1.mp3", "X", "Album", 100);
        from_tags.replay_gain = ReplayGain {
            track_db: Some(-7.0),
            ..Default::default()
        };
        insert_batch(
            &mut conn,
            &[
                from_tags,
                row("/m/a/2.mp3", "X", "Album", 100),
                row("/m/a/3.mp3", "X", "Album", 100),
            ],
        )
        .unwrap();
        assert_eq!(
            replaygain_breakdown(&conn).unwrap(),
            GainCoverage {
                tagged: 1,
                measured: 0,
                missing: 2
            }
        );

        set_measured_replaygain(
            &mut conn,
            &[(
                "/m/a/2.mp3",
                ReplayGain {
                    track_db: Some(-5.0),
                    ..Default::default()
                },
            )],
        )
        .unwrap();
        let split = replaygain_breakdown(&conn).unwrap();
        assert_eq!(
            split,
            GainCoverage {
                tagged: 1,
                measured: 1,
                missing: 1
            }
        );
        // Both sources count as covered; only the missing row doesn't.
        assert_eq!((split.covered(), split.total()), (2, 3));
    }

    /// The scan timestamp stamps a row when it first lands and a rescan's
    /// upsert leaves it alone, so a re-read file keeps the moment it
    /// entered the library.
    #[test]
    fn added_stamps_once_and_survives_a_rescan() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let track = || row("/m/a/1.mp3", "X", "Album", 100);
        insert_batch(&mut conn, &[track()]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        let added: i64 = conn
            .query_row("SELECT added FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert!(added > 0, "a first insert stamps the scan time");

        // Pin it to a known past value, then rescan: the upsert must not
        // move it.
        conn.execute("UPDATE tracks SET added = 123 WHERE id = ?1", [id])
            .unwrap();
        insert_batch(&mut conn, &[track()]).unwrap();
        let after: i64 = conn
            .query_row("SELECT added FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 123, "a rescan keeps the first-seen scan time");

        let p = crate::projection::Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.resolve(0).added, 123, "the projection carries it through");
    }

    /// Pruning drops the stored rows under a root that the walk no longer
    /// found, leaves the ones it did, and never touches another root.
    #[test]
    fn prune_removes_only_missing_rows_under_root() {
        use std::collections::HashSet;
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(
            &mut conn,
            &[
                row("/m/a/1.mp3", "X", "Album", 100),
                row("/m/a/2.mp3", "X", "Album", 200),
                row("/m/b/1.mp3", "Y", "Album", 300),
                row("/n/d/1.mp3", "W", "Other", 400),
            ],
        )
        .unwrap();

        // The walk under /m found only a/1; a/2 and b/1 are gone. /n is a
        // different root and out of range, so it stays regardless.
        let present: HashSet<String> = ["/m/a/1.mp3".to_string()].into_iter().collect();
        let removed = prune_missing(&mut conn, Path::new("/m"), &present).unwrap();
        assert_eq!(removed, 2);

        let mut paths: Vec<String> = local_files(&conn).unwrap().into_keys().collect();
        paths.sort();
        assert_eq!(paths, ["/m/a/1.mp3", "/n/d/1.mp3"]);

        // A pass that found everything removes nothing.
        let present: HashSet<String> = ["/m/a/1.mp3".to_string(), "/n/d/1.mp3".to_string()]
            .into_iter()
            .collect();
        assert_eq!(
            prune_missing(&mut conn, Path::new("/m"), &present).unwrap(),
            0
        );
    }

    /// A deleted file drops just its row; a deleted folder drops the whole
    /// subtree, and a sibling folder that shares a name prefix is left alone.
    #[test]
    fn remove_subtree_drops_a_file_or_a_whole_folder() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(
            &mut conn,
            &[
                row("/m/Artist/Album/1.mp3", "Artist", "Album", 100),
                row("/m/Artist/Album/2.mp3", "Artist", "Album", 200),
                row("/m/Artist/Live/1.mp3", "Artist", "Live", 300),
                // A sibling whose name is a prefix of "Album": the range must
                // not reach across the separator into it.
                row("/m/Artist/Album Two/1.mp3", "Artist", "Album Two", 400),
            ],
        )
        .unwrap();

        // One deleted file: just its row.
        assert_eq!(
            remove_subtree(&conn, Path::new("/m/Artist/Album/2.mp3")).unwrap(),
            1
        );
        assert_eq!(count(&conn).unwrap(), 3);

        // The deleted album folder: its remaining track, and nothing from the
        // "Album Two" sibling or the "Live" folder.
        assert_eq!(
            remove_subtree(&conn, Path::new("/m/Artist/Album")).unwrap(),
            1
        );
        let mut paths: Vec<String> = local_files(&conn).unwrap().into_keys().collect();
        paths.sort();
        assert_eq!(paths, ["/m/Artist/Album Two/1.mp3", "/m/Artist/Live/1.mp3"]);
    }

    /// A renamed file and a renamed folder both keep their ids, so the
    /// `added` stamp, rating, and joins survive the move; a sibling folder
    /// that shares a name prefix is left where it was.
    #[test]
    fn rename_within_moves_the_subtree_and_keeps_ids() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(
            &mut conn,
            &[
                row("/m/Artist/Album/1.mp3", "Artist", "Album", 100),
                row("/m/Artist/Album/2.mp3", "Artist", "Album", 200),
                // A sibling whose name is a prefix of "Album": the range must
                // not reach across the separator into it.
                row("/m/Artist/Album Two/1.mp3", "Artist", "Album Two", 400),
            ],
        )
        .unwrap();

        // A single file rename keeps the row's id.
        let file_id = id_for_path(&conn, "/m/Artist/Album/1.mp3")
            .unwrap()
            .unwrap();
        assert_eq!(
            rename_within(
                &mut conn,
                Path::new("/m/Artist/Album/1.mp3"),
                Path::new("/m/Artist/Album/one.mp3"),
            )
            .unwrap(),
            1
        );
        assert!(id_for_path(&conn, "/m/Artist/Album/1.mp3")
            .unwrap()
            .is_none());
        assert_eq!(
            id_for_path(&conn, "/m/Artist/Album/one.mp3").unwrap(),
            Some(file_id),
            "a renamed file keeps its id"
        );

        // A folder rename moves the whole subtree, each row keeping its id,
        // and leaves the prefix-sibling folder untouched.
        let sibling_id = id_for_path(&conn, "/m/Artist/Album Two/1.mp3")
            .unwrap()
            .unwrap();
        let two_id = id_for_path(&conn, "/m/Artist/Album/2.mp3")
            .unwrap()
            .unwrap();
        assert_eq!(
            rename_within(
                &mut conn,
                Path::new("/m/Artist/Album"),
                Path::new("/m/Artist/Record")
            )
            .unwrap(),
            2
        );
        assert_eq!(
            id_for_path(&conn, "/m/Artist/Record/one.mp3").unwrap(),
            Some(file_id)
        );
        assert_eq!(
            id_for_path(&conn, "/m/Artist/Record/2.mp3").unwrap(),
            Some(two_id)
        );
        assert_eq!(
            id_for_path(&conn, "/m/Artist/Album Two/1.mp3").unwrap(),
            Some(sibling_id),
            "a name-prefix sibling folder is not swept up"
        );
    }

    /// The edit path's landing half: committed changes move exactly their
    /// columns, the reloaded projection shows them, and everything else
    /// holds still.
    #[test]
    fn apply_changes_moves_only_named_columns() {
        use crate::writer::{Change, Field};
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut before = row("/m/a/1.mp3", "X", "Album", 100);
        before.title = "Before".into();
        before.artist = "Someone".into();
        before.year = 1999;
        insert_batch(&mut conn, &[before]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        apply_changes(
            &conn,
            id,
            &[
                Change {
                    field: Field::Title,
                    value: Some("After".into()),
                },
                Change {
                    field: Field::Year,
                    value: Some("2020-05-01".into()),
                },
                Change {
                    field: Field::TrackNo,
                    value: Some("5/12".into()),
                },
                Change {
                    field: Field::AlbumArtist,
                    value: None,
                },
                Change {
                    field: Field::Comment,
                    value: Some("no column".into()),
                },
            ],
        )
        .unwrap();

        let p = crate::projection::Projection::load_serial(&conn, false).unwrap();
        let v = p.resolve(0);
        assert_eq!(v.title, "After");
        assert_eq!(v.year, 2020, "the date's leading digits land as the year");
        assert_eq!(v.track_no, 5, "a track fraction lands as its number");
        assert_eq!(
            v.album_artist, "Someone",
            "a cleared album artist falls back to the artist"
        );
        assert_eq!(
            (v.artist, v.album),
            ("Someone", "Album"),
            "untouched columns hold"
        );
    }
}
