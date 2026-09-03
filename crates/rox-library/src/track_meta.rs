//! The track metadata table: sort names for track titles.
//!
//! The third of the sort-name tables, and the only one keyed by row rather
//! than by value. A title isn't interned in the projection the way an
//! artist or an album is: there are as many titles as there are tracks and
//! most of them appear once, so they live in an arena addressed by row and
//! there's no symbol to hang a sort name off. This table follows that,
//! keyed by the track id the store gave the row.
//!
//! What follows from the key is worth knowing. A track id belongs to a
//! (source, path, subsong), so a file that's moved and rescanned under a
//! new id leaves its old row here orphaned. That's dead weight rather than
//! a bug: the projection only ever looks a row up by an id it holds, an
//! orphan is a few dozen bytes, and the pass fills the new id the next
//! time it runs. Nothing prunes them, deliberately, because the thing that
//! looks like an orphan is often a file that's about to come back.
//!
//! Sources and their ranking are [`crate::artist_meta`]'s; in practice a
//! row here says `romanized` or `user`, since no service publishes a sort
//! title.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

/// The table beside the tracks it describes, keyed by track id.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS track_meta (
            track_id   INTEGER PRIMARY KEY,
            title_sort TEXT NOT NULL,
            source     TEXT NOT NULL,
            fetched    INTEGER NOT NULL
        );",
    )
}

/// Record a track's sort title. The write only lands over a row whose
/// source ranks no higher; see [`crate::artist_meta`].
///
/// An empty sort title writes nothing: a row that files a track under
/// nothing is a row the projection's merge would have to skip anyway.
pub fn set(
    conn: &Connection,
    track_id: i64,
    title_sort: &str,
    source: &str,
) -> rusqlite::Result<()> {
    let title_sort = title_sort.trim();
    if title_sort.is_empty() {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        &format!(
            "INSERT INTO track_meta (track_id, title_sort, source, fetched)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(track_id) DO UPDATE SET
                 title_sort = excluded.title_sort,
                 source     = excluded.source,
                 fetched    = excluded.fetched
             {}",
            crate::artist_meta::guard("track_meta")
        ),
        rusqlite::params![track_id, title_sort, source, now],
    )?;
    Ok(())
}

/// Forget a track's sort title, whoever wrote it.
pub fn clear(conn: &Connection, track_id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM track_meta WHERE track_id = ?1", [track_id])
}

/// The whole table as a track id -> sort title map, one query per
/// projection build. Bigger than the other two by construction, a row per
/// romanized track rather than per value, but still only the rows the pass
/// actually filled.
/// The track ids the romanization pass wrote under a marker other than
/// `current`; see [`crate::artist_meta::stale_romanized`].
pub fn stale_romanized(conn: &Connection, current: &str) -> rusqlite::Result<HashSet<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id FROM track_meta
         WHERE (source = 'romanized' OR source LIKE 'romanized:%') AND source <> ?1",
    )?;
    let rows = stmt.query_map([current], |row| row.get(0))?;
    rows.collect()
}

pub fn load_all(conn: &Connection) -> rusqlite::Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare_cached("SELECT track_id, title_sort FROM track_meta")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artist_meta::{ROMANIZED, USER};

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn a_romanized_row_round_trips_and_a_rerun_corrects_it() {
        let conn = conn();
        set(&conn, 7, "remon", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()[&7], "remon");
        set(&conn, 7, "lemon", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()[&7], "lemon");
        assert_eq!(clear(&conn, 7).unwrap(), 1);
        assert!(load_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_typed_row_survives_every_pass() {
        let conn = conn();
        set(&conn, 7, "Lemon", USER).unwrap();
        set(&conn, 7, "remon", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()[&7], "Lemon");
    }

    #[test]
    fn an_empty_sort_title_writes_nothing() {
        let conn = conn();
        set(&conn, 7, "   ", ROMANIZED).unwrap();
        assert!(load_all(&conn).unwrap().is_empty());
    }
}
