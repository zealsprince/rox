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

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    // Source-qualified identity per the components contract: local files are
    // the first source, streaming extensions add rows instead of migrations.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks (
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
        );",
    )
}

pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
}

/// Insert or refresh one batch of local rows inside a single transaction. An
/// existing (source, path) row keeps its id, so projection db_ids stay valid
/// across a rescan.
pub fn insert_batch(conn: &mut Connection, rows: &[TrackRow]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO tracks
             (path, title, artist, album, genre, year, track_no, duration_ms, size, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT (source, path) DO UPDATE SET
                title = excluded.title, artist = excluded.artist,
                album = excluded.album, genre = excluded.genre,
                year = excluded.year, track_no = excluded.track_no,
                duration_ms = excluded.duration_ms, size = excluded.size,
                mtime = excluded.mtime",
        )?;
        for r in rows {
            stmt.execute(rusqlite::params![
                r.path,
                r.title,
                r.artist,
                r.album,
                r.genre,
                r.year,
                r.track_no,
                r.duration_ms,
                r.size as i64,
                r.mtime,
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

/// How many local tracks live under one folder.
pub fn count_under(conn: &Connection, root: &Path) -> rusqlite::Result<u64> {
    let (lo, hi) = path_range(root);
    conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'local' AND path >= ?1 AND path < ?2",
        rusqlite::params![lo, hi],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u64)
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
}

/// Resolve a playable path back to its tags, for showing what is playing.
/// Ok(None) when the path is not in the library.
pub fn meta_for_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<TrackMeta>> {
    let mut stmt = conn.prepare_cached(
        "SELECT title, artist, album, track_no FROM tracks
         WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(TrackMeta {
            title: row.get(0)?,
            artist: row.get(1)?,
            album: row.get(2)?,
            track_no: row.get::<_, i64>(3)? as u16,
        })),
        None => Ok(None),
    }
}

pub fn max_rowid(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tracks", [], |r| r.get(0))
}

/// Stream the projection columns for one rowid range, in id order.
pub fn scan_range(
    conn: &Connection,
    lo: i64,
    hi: i64,
    mut sink: impl FnMut(i64, &str, &str, &str, &str, u16, u16, u32),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, album, genre, year, track_no, duration_ms
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
            row.get::<_, i64>(5)? as u16,
            row.get::<_, i64>(6)? as u16,
            row.get::<_, i64>(7)? as u32,
        );
    }
    Ok(())
}
