//! ADR 11's listen history: an append-only events table in the library
//! database. A listen row carries the track id, when the play began, and
//! a snapshot of the identifying tags at play time - the deletion hedge.
//! While a track exists, reads resolve through the live catalog, so a
//! fixed tag re-buckets history with it; once the track is gone the
//! snapshot keeps the row readable. Every stat is derived from these
//! rows by SQL; nothing stores a counter as the source.

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
}

fn track_plays_row(row: &rusqlite::Row) -> rusqlite::Result<TrackPlays> {
    Ok(TrackPlays {
        track_id: row.get(0)?,
        plays: row.get::<_, i64>(1)? as u64,
        last_played: row.get(2)?,
        title: row.get(3)?,
        artist: row.get(4)?,
        album: row.get(5)?,
    })
}

/// The tag columns of a listen read: the live catalog's while the track
/// exists, the snapshot's once it is gone.
const SNAPSHOT_COLUMNS: &str = "COALESCE(t.title, l.title),
     COALESCE(t.artist, l.artist), COALESCE(t.album, l.album)";

/// The newest events first, one row per event.
pub fn recent(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT l.track_id, 1, l.played_at, {SNAPSHOT_COLUMNS}
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         ORDER BY l.played_at DESC, l.id DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit as i64], track_plays_row)?;
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
        "SELECT id, 0, 0, title, artist, album FROM tracks
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

/// One name's line in a stats rollup.
#[derive(Clone)]
pub struct NamePlays {
    pub name: String,
    pub plays: u64,
}

/// Play counts grouped under one tag, most first. Grouping goes through
/// the live catalog first, so fixing a tag re-buckets its history;
/// untagged plays (empty name) stay out of the list.
pub fn rollup(conn: &Connection, by: Rollup, limit: usize) -> rusqlite::Result<Vec<NamePlays>> {
    let column = match by {
        Rollup::Artist => "artist",
        Rollup::Album => "album",
        Rollup::Genre => "genre",
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT COALESCE(t.{column}, l.{column}) AS name, COUNT(*) AS plays
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE name <> ''
         GROUP BY name
         ORDER BY plays DESC, name LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(NamePlays {
            name: row.get(0)?,
            plays: row.get::<_, i64>(1)? as u64,
        })
    })?;
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

        let recent = recent(&conn, 10).unwrap();
        assert_eq!(
            recent.iter().map(|r| r.last_played).collect::<Vec<_>>(),
            [300, 200, 100],
            "recent runs newest first"
        );

        let most = most_played(&conn, 10).unwrap();
        assert_eq!((most[0].title.as_str(), most[0].plays), ("One", 2));

        let never = never_played(&conn, 10).unwrap();
        assert_eq!(never.len(), 1);
        assert_eq!(never[0].title, "Two");

        let genres = rollup(&conn, Rollup::Genre, 10).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("rock", 2), ("jazz", 1)]
        );

        assert_eq!(count_since(&conn, 0).unwrap(), 3);
        assert_eq!(count_since(&conn, 200).unwrap(), 2);
    }

    #[test]
    fn snapshot_outlives_a_deleted_track() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "Gone", "A", "First", "rock")])
            .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        conn.execute("DELETE FROM tracks", []).unwrap();

        let recent = recent(&conn, 10).unwrap();
        assert_eq!(recent[0].title, "Gone", "the snapshot keeps the row readable");
        let artists = rollup(&conn, Rollup::Artist, 10).unwrap();
        assert_eq!(artists[0].name, "A");
    }
}
