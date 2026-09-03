//! The album metadata table: sort names for album titles.
//!
//! [`crate::artist_meta`]'s twin, one row per album title, and it exists
//! for the half of the problem MusicBrainz can't answer. The service
//! models a sort name for an artist and nothing at all for a release, so
//! an album called 打上花火 had no source for a Latin spelling until the
//! romanization pass got one by reading the characters. That's why the
//! rows here are almost all `romanized` where the artist table's are
//! almost all `musicbrainz`; the ranking is shared all the same, so a
//! hand-typed sort name still wins and a later pass can correct its own
//! guess.
//!
//! Keyed by the album title as the tags spell it, which is the string the
//! projection interned and the string the pass looked up. Not by (album
//! artist, album): two different albums with the same title share a
//! symbol in the projection already, so keying finer here would produce
//! rows the merge could never find.
//!
//! [`crate::projection`] reads the flattened map once per build and lays
//! it over the album symbol table, the same move it makes for artists.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

/// The table beside the tracks it describes.
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

/// Record an album's sort name. `source` is one of
/// [`crate::artist_meta::USER`] or [`crate::artist_meta::ROMANIZED`] in
/// practice, and the write only lands over a row whose source ranks no
/// higher.
///
/// An empty name or sort name writes nothing, for the reason
/// [`crate::artist_meta::set`] gives.
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

/// Forget an album's sort name, whoever wrote it.
pub fn clear(conn: &Connection, name: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM album_meta WHERE name = ?1", [name.trim()])
}

/// The whole table as a name -> sort name map, one query per projection
/// build. A row per album title rather than per track, so it stays small.
/// The album names the romanization pass wrote under a marker other than
/// `current`; see [`crate::artist_meta::stale_romanized`].
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
