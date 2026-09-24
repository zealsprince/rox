//! The artist metadata table: sort names the library knows and the files
//! don't, laid over the tags without rewriting a file (ADR 14). Almost
//! nothing tags `ARTISTSORT`: 24 files of 53,343 in Andrew's library,
//! against 5,999 artists.
//!
//! `source` ranks the rows: `user` beats `musicbrainz` beats `romanized`.
//! The rank lives in the `ON CONFLICT` guard every meta table shares, so
//! there's one place it can be got wrong.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

/// A fetched row. The next fetch may replace it.
pub const MUSICBRAINZ: &str = "musicbrainz";

/// A typed row. Nothing else overwrites it.
pub const USER: &str = "user";

/// Written by the romanization pass (`rox/src/romanize_job.rs`). The
/// weakest: it fills gaps and never replaces a person or a service.
pub const ROMANIZED: &str = "romanized";

/// The pass's versioned marker, `romanized:<n>`, so a later spelling can
/// find and redo exactly its own old answers. Bare `romanized` is version 0.
pub fn romanized_marker(version: u32) -> String {
    format!("{ROMANIZED}:{version}")
}

pub fn is_romanized(source: &str) -> bool {
    source == ROMANIZED || source.starts_with("romanized:")
}

fn romanized_sql(side: &str) -> String {
    format!("({side}.source = '{ROMANIZED}' OR {side}.source LIKE '{ROMANIZED}:%')")
}

/// Rows the pass wrote under a marker other than `current`.
pub fn stale_romanized(conn: &Connection, current: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT name FROM artist_meta WHERE {} AND source <> ?1",
        romanized_sql("artist_meta")
    ))?;
    let rows = stmt.query_map([current], |row| row.get(0))?;
    rows.collect()
}

/// The source rank as SQL for one side of an upsert. An unknown source ranks
/// zero, the safe way for a newer build's row to be wrong.
fn rank(side: &str) -> String {
    format!(
        "CASE WHEN {side}.source = '{USER}' THEN 3 WHEN {side}.source = '{MUSICBRAINZ}' THEN 2 \
         WHEN {} THEN 1 ELSE 0 END",
        romanized_sql(side)
    )
}

/// The `ON CONFLICT` guard every meta table shares.
pub(crate) fn guard(table: &str) -> String {
    format!("WHERE ({}) >= ({})", rank("excluded"), rank(table))
}

/// Keyed by the artist name exactly as the tags spell it.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS artist_meta (
            name      TEXT PRIMARY KEY,
            sort_name TEXT NOT NULL,
            source    TEXT NOT NULL,
            fetched   INTEGER NOT NULL
        );",
    )
}

/// Record an artist's sort name, landing only over a source that ranks no
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
            "INSERT INTO artist_meta (name, sort_name, source, fetched)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                 sort_name = excluded.sort_name,
                 source    = excluded.source,
                 fetched   = excluded.fetched
             {}",
            guard("artist_meta")
        ),
        rusqlite::params![name, sort_name, source, now],
    )?;
    Ok(())
}

pub fn clear(conn: &Connection, name: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM artist_meta WHERE name = ?1", [name.trim()])
}

pub fn load_all(conn: &Connection) -> rusqlite::Result<HashMap<String, String>> {
    let mut stmt = conn.prepare_cached("SELECT name, sort_name FROM artist_meta")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn a_fetched_row_round_trips_and_a_refetch_replaces_it() {
        let conn = conn();
        set(&conn, "米津玄師", "Yonezu, Kenshi", MUSICBRAINZ).unwrap();
        assert_eq!(load_all(&conn).unwrap()["米津玄師"], "Yonezu, Kenshi");
        set(&conn, "米津玄師", "Yonezu Kenshi", MUSICBRAINZ).unwrap();
        assert_eq!(load_all(&conn).unwrap()["米津玄師"], "Yonezu Kenshi");
        assert_eq!(clear(&conn, "米津玄師").unwrap(), 1);
        assert!(load_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_typed_row_survives_every_fetch() {
        let conn = conn();
        set(&conn, "Sigur Rós", "Sigur Ros", USER).unwrap();
        set(&conn, "Sigur Rós", "Rós, Sigur", MUSICBRAINZ).unwrap();
        assert_eq!(
            load_all(&conn).unwrap()["Sigur Rós"],
            "Sigur Ros",
            "a fetch never overwrites what a person typed"
        );
        set(&conn, "Sigur Rós", "Sigur Ros, the band", USER).unwrap();
        assert_eq!(load_all(&conn).unwrap()["Sigur Rós"], "Sigur Ros, the band");
    }

    #[test]
    fn a_romanized_row_sits_under_both_of_the_others() {
        let conn = conn();
        set(&conn, "崎山蒼志", "sakiyamasoushi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "sakiyamasoushi");
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "sakiyamasoshi");
        set(&conn, "崎山蒼志", "Sakiyama, Soushi", MUSICBRAINZ).unwrap();
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "Sakiyama, Soushi");
        set(&conn, "崎山蒼志", "Soushi", USER).unwrap();
        set(&conn, "崎山蒼志", "Sakiyama, Soushi", MUSICBRAINZ).unwrap();
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "Soushi");
    }

    #[test]
    fn a_versioned_marker_ranks_as_romanized_and_names_its_stale_rows() {
        let conn = conn();
        let two = romanized_marker(2);
        assert!(is_romanized(&two) && is_romanized(ROMANIZED) && !is_romanized(USER));
        set(&conn, "秋ノ風", "akinokaze", ROMANIZED).unwrap();
        set(&conn, "米津玄師", "Yonezu, Kenshi", MUSICBRAINZ).unwrap();
        set(&conn, "崎山蒼志", "Sakiyama Soushi", &two).unwrap();
        let stale = stale_romanized(&conn, &two).unwrap();
        assert_eq!(stale, HashSet::from(["秋ノ風".to_string()]));
        set(&conn, "秋ノ風", "Aki no kaze", &two).unwrap();
        assert_eq!(load_all(&conn).unwrap()["秋ノ風"], "Aki no kaze");
        assert!(stale_romanized(&conn, &two).unwrap().is_empty());
        set(&conn, "米津玄師", "Yonetsu genshi", &two).unwrap();
        assert_eq!(load_all(&conn).unwrap()["米津玄師"], "Yonezu, Kenshi");
    }

    #[test]
    fn an_empty_half_writes_nothing() {
        let conn = conn();
        set(&conn, "米津玄師", "", MUSICBRAINZ).unwrap();
        set(&conn, "", "Yonezu, Kenshi", MUSICBRAINZ).unwrap();
        assert!(load_all(&conn).unwrap().is_empty());
    }
}
