//! ADR 11's listen history: an append-only events table in the library
//! database. A listen row carries the track id, when the play began, and
//! a snapshot of the identifying tags at play time - the deletion hedge.
//! While a track exists, reads resolve through the live catalog, so a
//! fixed tag re-buckets history with it; once the track is gone the
//! snapshot keeps the row readable. Every stat is derived from these
//! rows by SQL; nothing stores a counter as the source.

use std::collections::HashMap;

use rusqlite::Connection;

/// The events table beside the tracks it keys to. No foreign key on
/// purpose: deleting a track keeps its history, that is the snapshot's
/// whole job.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS listens (
            id        INTEGER PRIMARY KEY,
            track_id  INTEGER NOT NULL,
            played_at INTEGER NOT NULL,
            title     TEXT NOT NULL,
            artist    TEXT NOT NULL,
            album     TEXT NOT NULL,
            genre     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS listens_track ON listens (track_id);
        CREATE INDEX IF NOT EXISTS listens_played ON listens (played_at);",
    )
}

/// One listen as it lands: the track's identity, when the play began
/// (unix seconds), and its tags at play time.
pub struct Listen {
    pub track_id: i64,
    pub played_at: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
}

/// Build the listen for a playing path from the live catalog. Ok(None)
/// when the path is not in the library: an unindexed file plays without
/// history, since events key to track identity.
pub fn listen_for_path(
    conn: &Connection,
    path: &str,
    played_at: i64,
) -> rusqlite::Result<Option<Listen>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, album, genre FROM tracks
         WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(Listen {
            track_id: row.get(0)?,
            played_at,
            title: row.get(1)?,
            artist: row.get(2)?,
            album: row.get(3)?,
            genre: row.get(4)?,
        })),
        None => Ok(None),
    }
}

/// Append one event row. Append-only: nothing ever updates or deletes a
/// listen.
pub fn append(conn: &Connection, listen: &Listen) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO listens (track_id, played_at, title, artist, album, genre)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    stmt.execute(rusqlite::params![
        listen.track_id,
        listen.played_at,
        listen.title,
        listen.artist,
        listen.album,
        listen.genre,
    ])?;
    Ok(())
}

/// One track's line in a history view. Recent rows carry one event each
/// (plays 1, last_played that event's time); rollup rows aggregate a
/// track's whole history; never-played rows have neither (both 0).
#[derive(Clone)]
pub struct TrackPlays {
    pub track_id: i64,
    pub plays: u64,
    pub last_played: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The album grouping and column metadata, read live from the catalog;
    /// empty or zero once the track is gone, since the snapshot keeps only
    /// title, artist, and album.
    pub album_artist: String,
    pub year: u16,
    pub genre: String,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub rating: u8,
    /// The file path, for the cover column's thumbnail; empty once the track
    /// is gone from the catalog.
    pub path: String,
}

fn track_plays_row(row: &rusqlite::Row) -> rusqlite::Result<TrackPlays> {
    Ok(TrackPlays {
        track_id: row.get(0)?,
        plays: row.get::<_, i64>(1)? as u64,
        last_played: row.get(2)?,
        title: row.get(3)?,
        artist: row.get(4)?,
        album: row.get(5)?,
        album_artist: row.get(6)?,
        year: row.get(7)?,
        genre: row.get(8)?,
        duration_ms: row.get(9)?,
        codec: row.get(10)?,
        bitrate_kbps: row.get(11)?,
        rating: row.get(12)?,
        path: row.get(13)?,
    })
}

/// The tag columns of a listen read: title, artist, and album from the live
/// catalog while the track exists, the snapshot once it is gone, then the
/// album grouping and column metadata (and the file path) from the live
/// catalog only.
const SNAPSHOT_COLUMNS: &str = "COALESCE(t.title, l.title),
     COALESCE(t.artist, l.artist), COALESCE(t.album, l.album),
     COALESCE(t.album_artist, ''), COALESCE(t.year, 0), COALESCE(t.genre, ''),
     COALESCE(t.duration_ms, 0), COALESCE(t.codec, ''), COALESCE(t.bitrate, 0),
     COALESCE(t.rating, 0), COALESCE(t.path, '')";

/// The newest events at or after `since` first, one row per event; 0
/// reads them all.
pub fn recent(conn: &Connection, since: i64, limit: usize) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT l.track_id, 1, l.played_at, {SNAPSHOT_COLUMNS}
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE l.played_at >= ?1
         ORDER BY l.played_at DESC, l.id DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map([since, limit as i64], track_plays_row)?;
    rows.collect()
}

/// Tracks by play count, most first. The bare snapshot columns resolve
/// from the MAX(played_at) row, SQLite's documented min/max behavior,
/// so a retagged-then-deleted track shows its newest snapshot.
pub fn most_played(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT l.track_id, COUNT(*) AS plays, MAX(l.played_at), {SNAPSHOT_COLUMNS}
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         GROUP BY l.track_id
         ORDER BY plays DESC, MAX(l.played_at) DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit as i64], track_plays_row)?;
    rows.collect()
}

/// Library tracks no event has ever named, in the canonical browse
/// order.
pub fn never_played(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, 0, 0, title, artist, album,
                album_artist, year, genre, duration_ms, codec, bitrate, rating, path
         FROM tracks
         WHERE id NOT IN (SELECT track_id FROM listens)
         ORDER BY album_artist, album, disc_no, track_no LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], track_plays_row)?;
    rows.collect()
}

/// What a name rollup groups by.
#[derive(Clone, Copy)]
pub enum Rollup {
    Artist,
    Album,
    Genre,
}

/// One name's line in a stats rollup. `sub` is the line's secondary
/// text: the album rollup carries the album artist there (an album name
/// alone reads ambiguous), the others leave it empty.
#[derive(Clone)]
pub struct NamePlays {
    pub name: String,
    pub sub: String,
    pub plays: u64,
}

/// Play counts grouped under one tag, most first, over the events at or
/// after `since` (0 counts them all) - the stats panel's range knob.
/// Grouping goes through the live catalog first, so fixing a tag
/// re-buckets its history; untagged plays (empty name) stay out of the
/// list.
pub fn rollup(
    conn: &Connection,
    by: Rollup,
    since: i64,
    limit: usize,
) -> rusqlite::Result<Vec<NamePlays>> {
    let column = match by {
        Rollup::Artist => "artist",
        Rollup::Album => "album",
        Rollup::Genre => "genre",
    };
    // The album rollup's secondary text. The snapshot has no
    // album_artist column, so a deleted track's rows fall back to the
    // plain artist; MAX() keeps the pick deterministic when a group
    // spans several.
    let sub = match by {
        Rollup::Album => "MAX(COALESCE(t.album_artist, l.artist))",
        _ => "''",
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT COALESCE(t.{column}, l.{column}) AS name, {sub}, COUNT(*) AS plays
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE l.played_at >= ?1 AND name <> ''
         GROUP BY name
         ORDER BY plays DESC, name LIMIT ?2"
    ))?;
    let rows = stmt.query_map([since, limit as i64], |row| {
        Ok(NamePlays {
            name: row.get(0)?,
            sub: row.get(1)?,
            plays: row.get::<_, i64>(2)? as u64,
        })
    })?;
    rows.collect()
}

/// Every track's play count in one aggregate, for the projection's
/// plays column. Tracks with no listens stay out of the map.
pub fn counts(conn: &Connection) -> rusqlite::Result<HashMap<i64, u32>> {
    let mut stmt =
        conn.prepare_cached("SELECT track_id, COUNT(*) FROM listens GROUP BY track_id")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u32))
    })?;
    rows.collect()
}

/// When the first listen landed (unix seconds); None before any has.
/// The all-time chart picks its span off this.
pub fn earliest(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MIN(played_at) FROM listens", [], |row| row.get(0))
}

/// Listens bucketed over time for the chart: one count per `bucket`
/// seconds from `since` up to `now`, empty buckets included, so the
/// bars carry the quiet stretches too.
pub fn histogram(
    conn: &Connection,
    since: i64,
    bucket: i64,
    now: i64,
) -> rusqlite::Result<Vec<u64>> {
    let n = ((now - since) / bucket).max(0) as usize + 1;
    let mut counts = vec![0u64; n];
    let mut stmt = conn.prepare_cached(
        "SELECT (played_at - ?1) / ?2 AS bucket, COUNT(*) FROM listens
         WHERE played_at >= ?1 GROUP BY bucket",
    )?;
    let rows = stmt.query_map([since, bucket], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u64))
    })?;
    for row in rows {
        let (index, count) = row?;
        // A listen stamped past `now` (clock skew) lands in the last bar
        // rather than out of bounds.
        let index = (index.max(0) as usize).min(n - 1);
        counts[index] += count;
    }
    Ok(counts)
}

/// Resolve one rollup name back to its library tracks in the canonical
/// browse order, so a stats row can queue what it counts. Live catalog
/// only: a deleted track's snapshot keeps its rows in the rollup, but
/// there is no file left to play.
pub fn ids_for_name(
    conn: &Connection,
    by: Rollup,
    name: &str,
    limit: usize,
) -> rusqlite::Result<Vec<i64>> {
    let column = match by {
        Rollup::Artist => "artist",
        Rollup::Album => "album",
        Rollup::Genre => "genre",
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id FROM tracks WHERE {column} = ?1
         ORDER BY album_artist, album, disc_no, track_no LIMIT ?2"
    ))?;
    let rows = stmt.query_map(rusqlite::params![name, limit as i64], |row| row.get(0))?;
    rows.collect()
}

/// How many listens landed at or after `since` (unix seconds); 0 counts
/// them all.
pub fn count_since(conn: &Connection, since: i64) -> rusqlite::Result<u64> {
    conn.query_row(
        "SELECT COUNT(*) FROM listens WHERE played_at >= ?1",
        [since],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{store, TrackRow};

    fn track(path: &str, title: &str, artist: &str, album: &str, genre: &str) -> TrackRow {
        TrackRow {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: album.into(),
            genre: genre.into(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            rating: 0,
            size: 0,
            mtime: 0,
        }
    }

    fn listen(conn: &Connection, path: &str, at: i64) {
        let listen = listen_for_path(conn, path, at).unwrap().unwrap();
        append(conn, &listen).unwrap();
    }

    #[test]
    fn stats_derive_from_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "rock"),
                track("/m/2.mp3", "Two", "A", "First", "rock"),
                track("/m/3.mp3", "Three", "B", "Second", "jazz"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/1.mp3", 300);
        listen(&conn, "/m/3.mp3", 200);

        let all = recent(&conn, 0, 10).unwrap();
        assert_eq!(
            all.iter().map(|r| r.last_played).collect::<Vec<_>>(),
            [300, 200, 100],
            "recent runs newest first"
        );
        assert_eq!(
            recent(&conn, 200, 10).unwrap().len(),
            2,
            "a range bound drops older events"
        );

        let most = most_played(&conn, 10).unwrap();
        assert_eq!((most[0].title.as_str(), most[0].plays), ("One", 2));

        let never = never_played(&conn, 10).unwrap();
        assert_eq!(never.len(), 1);
        assert_eq!(never[0].title, "Two");

        let genres = rollup(&conn, Rollup::Genre, 0, 10).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("rock", 2), ("jazz", 1)]
        );
        let albums = rollup(&conn, Rollup::Album, 0, 10).unwrap();
        assert_eq!(
            (albums[0].name.as_str(), albums[0].sub.as_str()),
            ("First", "A"),
            "the album rollup carries the album artist"
        );

        let recent_genres = rollup(&conn, Rollup::Genre, 200, 10).unwrap();
        assert_eq!(
            recent_genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("jazz", 1), ("rock", 1)],
            "a range bound re-counts the rollup"
        );

        assert_eq!(count_since(&conn, 0).unwrap(), 3);
        assert_eq!(count_since(&conn, 200).unwrap(), 2);

        assert_eq!(
            ids_for_name(&conn, Rollup::Artist, "A", 10).unwrap().len(),
            2,
            "a rollup name resolves to its library tracks"
        );
        assert_eq!(ids_for_name(&conn, Rollup::Genre, "jazz", 10).unwrap().len(), 1);

        assert_eq!(earliest(&conn).unwrap(), Some(100));
        assert_eq!(
            histogram(&conn, 100, 100, 400).unwrap(),
            [1, 1, 1, 0],
            "one count per bucket, empty buckets included"
        );
        assert_eq!(
            histogram(&conn, 0, 1000, 400).unwrap(),
            [3],
            "one bucket swallows everything"
        );
    }

    #[test]
    fn snapshot_outlives_a_deleted_track() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "Gone", "A", "First", "rock")])
            .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        conn.execute("DELETE FROM tracks", []).unwrap();

        let recent = recent(&conn, 0, 10).unwrap();
        assert_eq!(recent[0].title, "Gone", "the snapshot keeps the row readable");
        let artists = rollup(&conn, Rollup::Artist, 0, 10).unwrap();
        assert_eq!(artists[0].name, "A");
    }
}
