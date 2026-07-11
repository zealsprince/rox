//! The durable side of ADR 5: bundled SQLite in WAL mode. The write path is
//! batched upsert transactions from the scanner; the read path is the
//! projection load, either one reader or one reader per core over disjoint
//! rowid ranges (WAL gives concurrent readers for free).

use std::collections::HashMap;
use std::path::Path;

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

/// Every local path with its (mtime, size), so a rescan can skip files that
/// have not changed without reading their tags.
pub fn local_files(conn: &Connection) -> rusqlite::Result<HashMap<String, (i64, u64)>> {
    let mut stmt =
        conn.prepare("SELECT path, mtime, size FROM tracks WHERE source = 'local'")?;
    let mut rows = stmt.query([])?;
    let mut out = HashMap::new();
    while let Some(row) = rows.next()? {
        out.insert(row.get(0)?, (row.get(1)?, row.get::<_, i64>(2)? as u64));
    }
    Ok(out)
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
