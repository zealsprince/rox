//! The artist metadata table: sort names the library knows and the files
//! don't. One row per artist name with the Latin sort name to file it
//! under, where that name came from, and when it was fetched.
//!
//! Almost nothing tags `ARTISTSORT`. Andrew's library has it on 24 files
//! out of 53,343, against 5,999 distinct artists, so a projection built
//! from tags alone leaves 米津玄師 in its own letter-rail bucket and
//! unreachable from a Latin keyboard. MusicBrainz knows the answer for
//! most of them, and this is where the answer lands: the library's own
//! opinion about a value, laid over the tags without ever rewriting a
//! file. Exactly the arrangement [`crate::genre_meta`] already has for
//! genre aliases, and the reason ADR 14's "a provider never touches a
//! file" is satisfied by a bulk pass that would otherwise be an
//! unconfirmed write.
//!
//! `source` is the whole safety story, and there are three of them now.
//! A row a person typed says `user` and nothing overwrites it. A row a
//! fetch wrote says `musicbrainz`, and the next fetch may replace it,
//! because the service is the authority on its own answer. A row the
//! romanization pass wrote says `romanized`, and it sits under both:
//! reading the characters is the guess of last resort, so it fills a gap
//! and never argues with an answer. That order is written once as SQL
//! and applied in the `ON CONFLICT` clause of [`set`] rather than in the
//! caller, so there's one place it can be got wrong. The two
//! sibling tables ([`crate::album_meta`], [`crate::track_meta`]) share
//! the same expression for the same reason.
//!
//! [`crate::projection`] reads the flattened map once per build and lays
//! it over the symbol tables, so ordering, the letter rails and search
//! all agree without knowing this table exists.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

/// A row a fetch wrote. Replaceable by the next fetch, since the service
/// is the authority on its own answer.
pub const MUSICBRAINZ: &str = "musicbrainz";

/// A row a person typed. Never overwritten by anything else.
pub const USER: &str = "user";

/// A row [`crate::projection`]'s romanization pass wrote by reading the
/// characters. The weakest of the three: it's a transliteration rather
/// than a fact anyone recorded, so it fills a gap and never replaces an
/// answer that came from a person or a service.
pub const ROMANIZED: &str = "romanized";

/// The romanization pass's source column, carrying the crate's spelling
/// version: `romanized:2`. A bare `romanized` is a row from before the
/// version rode along, and reads as version zero. The number is what lets
/// a build that spaces and cases its readings differently find its own
/// earlier answers and redo exactly those.
pub fn romanized_marker(version: u32) -> String {
    format!("{ROMANIZED}:{version}")
}

/// Whether a source is the romanization pass's, at any version.
pub fn is_romanized(source: &str) -> bool {
    source == ROMANIZED || source.starts_with("romanized:")
}

/// The SQL test for [`is_romanized`], on one side of an upsert.
fn romanized_sql(side: &str) -> String {
    format!("({side}.source = '{ROMANIZED}' OR {side}.source LIKE '{ROMANIZED}:%')")
}

/// The names the romanization pass wrote under a marker other than
/// `current`: readings an older spelling produced, which the next run
/// redoes. Rows from a person or a service are never in this set.
pub fn stale_romanized(conn: &Connection, current: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT name FROM artist_meta WHERE {} AND source <> ?1",
        romanized_sql("artist_meta")
    ))?;
    let rows = stmt.query_map([current], |row| row.get(0))?;
    rows.collect()
}

/// The three sources as a SQL expression that ranks them, qualified by
/// whichever side of an upsert is being weighed. A write lands when its
/// own source ranks at least as high as the row already there.
///
/// A `CASE` rather than a chain of comparisons because the rule is an
/// ordering, and writing it as an ordering means a fourth source is one
/// arm rather than a rethink. An unknown source ranks zero, below
/// everything, which is the safe way for a row written by a newer build
/// to be wrong.
fn rank(side: &str) -> String {
    format!(
        "CASE WHEN {side}.source = '{USER}' THEN 3 WHEN {side}.source = '{MUSICBRAINZ}' THEN 2 \
         WHEN {} THEN 1 ELSE 0 END",
        romanized_sql(side)
    )
}

/// The `ON CONFLICT` guard for a meta table, with `table` its name. All
/// three tables have the same source column and the same rule, so they
/// share the clause instead of each spelling it out and one of them
/// getting it subtly different.
pub(crate) fn guard(table: &str) -> String {
    format!("WHERE ({}) >= ({})", rank("excluded"), rank(table))
}

/// The table beside the tracks it describes. Keyed by the artist name
/// exactly as the tags spell it, which is what the projection looks the
/// row up by.
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

/// Record an artist's sort name. `source` is one of [`USER`],
/// [`MUSICBRAINZ`] or [`ROMANIZED`], and a write only lands over a row
/// whose source ranks no higher; see [`rank`]. The guard is in the
/// statement so no caller has to remember it.
///
/// An empty name or sort name writes nothing: there's no artist to file
/// and nothing to file it under, and a row of two empty strings would
/// only make the projection's merge do work for no answer.
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

/// Forget an artist's sort name, whoever wrote it: the row goes and the
/// projection falls back to whatever the files say, which for most of
/// these is nothing. The next pass will ask again.
pub fn clear(conn: &Connection, name: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM artist_meta WHERE name = ?1", [name.trim()])
}

/// The whole table as a name -> sort name map, which is how the
/// projection wants it: one query per build, then a lookup per symbol.
/// Small by construction, a row per artist rather than per track.
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
        // The service is the authority on its own answer, so a later
        // fetch is allowed to correct an earlier one.
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
        // A person changing their own mind still lands.
        set(&conn, "Sigur Rós", "Sigur Ros, the band", USER).unwrap();
        assert_eq!(load_all(&conn).unwrap()["Sigur Rós"], "Sigur Ros, the band");
    }

    /// The rank in both directions, which is the rule the romanization
    /// pass leans on: it may fill a gap and it may correct itself, and it
    /// may never argue with the other two.
    #[test]
    fn a_romanized_row_sits_under_both_of_the_others() {
        let conn = conn();
        set(&conn, "崎山蒼志", "sakiyamasoushi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "sakiyamasoushi");
        // A later run of the same pass may correct its own guess.
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "sakiyamasoshi");
        // A lookup and a person both outrank it.
        set(&conn, "崎山蒼志", "Sakiyama, Soushi", MUSICBRAINZ).unwrap();
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "Sakiyama, Soushi");
        set(&conn, "崎山蒼志", "Soushi", USER).unwrap();
        set(&conn, "崎山蒼志", "Sakiyama, Soushi", MUSICBRAINZ).unwrap();
        set(&conn, "崎山蒼志", "sakiyamasoshi", ROMANIZED).unwrap();
        assert_eq!(load_all(&conn).unwrap()["崎山蒼志"], "Soushi");
    }

    /// A versioned marker is the same rank as the bare one, so a newer
    /// spelling lands over an older reading and the stale set names
    /// exactly the rows an older marker wrote.
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
        // Still under a service.
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
