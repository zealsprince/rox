//! The album metadata table: sort names for album titles, sharing
//! [`crate::artist_meta`]'s ranking. MusicBrainz has no release sort name, so
//! rows here are nearly all `romanized`.
//!
//! Keyed by title alone, not (album artist, album): same-titled albums
//! already share a projection symbol, so a finer key could never be found.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS album_meta (
            name      TEXT PRIMARY KEY,
            sort_name TEXT NOT NULL,
            source    TEXT NOT NULL,
            fetched   INTEGER NOT NULL
        );",
    )
}

/// Record an album's sort name, landing only over a source that ranks no
/// higher. An empty name or sort name writes nothing.
pub fn set(conn: &Connection, name: &str, sort_name: &str, source: &str) -> rusqlite::Result<()> {
    let name = name.trim();
    let sort_name = sort_name.trim();
    if name.is_empty() || sort_name.is_empty() {
        return Ok(());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        &format!(
            "INSERT INTO album_meta (name, sort_name, source, fetched)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                 sort_name = excluded.sort_name,
                 source    = excluded.source,
                 fetched   = excluded.fetched
             {}",
            crate::artist_meta::guard("album_meta")
        ),
        rusqlite::params![name, sort_name, source, now],
    )?;
    Ok(())
}

pub fn clear(conn: &Connection, name: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM album_meta WHERE name = ?1", [name.trim()])
}

/// See [`crate::artist_meta::stale_romanized`].
pub fn stale_romanized(conn: &Connection, current: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM album_meta
         WHERE (source = 'romanized' OR source LIKE 'romanized:%') AND source <> ?1",
    )?;
    let rows = stmt.query_map([current], |row| row.get(0))?;
    rows.collect()
}

pub fn load_all(conn: &Connection) -> rusqlite::Result<HashMap<String, String>> {
    let mut stmt = conn.prepare_cached("SELECT name, sort_name FROM album_meta")?;
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
        set(&conn, "打上花火", "uchiagehanabi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["打上花火"], "uchiagehanabi");
        set(&conn, "打上花火", "uchiage hanabi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["打上花火"], "uchiage hanabi");
        assert_eq!(clear(&conn, "打上花火").unwrap(), 1);
        assert!(load_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_typed_row_survives_every_pass() {
        let conn = conn();
        set(&conn, "打上花火", "Fireworks", USER).unwrap();
        set(&conn, "打上花火", "uchiagehanabi", ROMANIZED).unwrap();
        assert_eq!(
            load_all(&conn).unwrap()["打上花火"],
            "Fireworks",
            "a pass never overwrites what a person typed"
        );
    }

    #[test]
    fn an_empty_half_writes_nothing() {
        let conn = conn();
        set(&conn, "打上花火", "", ROMANIZED).unwrap();
        set(&conn, "", "uchiagehanabi", ROMANIZED).unwrap();
        assert!(load_all(&conn).unwrap().is_empty());
    }
}
