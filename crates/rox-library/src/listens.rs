//! ADR 11's listen history: an append-only events table in the library
//! database. A listen row holds the track id, when the play began, and
//! a snapshot of the identifying tags at play time, the deletion hedge.
//! While a track exists, reads resolve through the live catalog, so a
//! fixed tag re-buckets history with it; once the track is gone the
//! snapshot keeps the row readable. Every stat is derived from these
//! rows by SQL; nothing stores a counter as the source.
//!
//! Append-only covers the event itself: when it played and what the tags
//! said then never change. The join back to the catalog is maintenance,
//! not history: a prune kills the track id, and when the file returns
//! under a fresh id [`reattach`] moves the events onto it by the path
//! recorded at play time, so a track's play count is kept across its file
//! leaving and coming back. The path column is that join hint, nothing a
//! view shows.

use std::collections::HashMap;

use rusqlite::Connection;

/// The events table beside the tracks it keys to. No foreign key here:
/// deleting a track keeps its history, which is the snapshot's whole job.
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

/// The store ladder's snapshot-paths step, the listens half: events learn
/// the path that played, the content key [`reattach`] matches on. Live
/// rows backfill from the catalog; rows already dangling keep the empty
/// default and rely on the tag fallback.
pub(crate) fn add_path_snapshot(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE listens ADD COLUMN path TEXT NOT NULL DEFAULT '';
         UPDATE listens SET path = COALESCE(
             (SELECT t.path FROM tracks t
              WHERE t.id = listens.track_id AND t.source = 'local'), '');",
    )
}

/// Match events back to the catalog after a scan, the same maintenance
/// [`crate::playlists::reattach`] runs for members: a pruned-and-returned
/// file comes back under a fresh id, and the events that played its old row
/// relink to it: by the recorded path first, then by the tag snapshot
/// when it names exactly one track. Events with a live track just keep
/// their path current. The event itself (played_at, the tag snapshot)
/// never changes. Returns how many events relinked.
pub fn reattach(conn: &Connection) -> rusqlite::Result<usize> {
    // The snapshot key is the fragment form a TrackKey serializes to: the
    // bare path for a plain file, path#sub for a cue track. Matching on the
    // bare path would attach every listen of a rip to whichever of its rows
    // sorts first, so both the refresh and the relink build the same
    // expression the recorder wrote.
    conn.execute(
        "UPDATE listens SET path =
             CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END
         FROM tracks t
         WHERE t.id = listens.track_id AND t.source = 'local'
           AND listens.path <>
             CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END",
        [],
    )?;
    let by_path = conn.execute(
        "UPDATE listens SET track_id = t.id FROM tracks t
         WHERE listens.path <> ''
           AND t.source = 'local'
           AND CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END
               = listens.path
           AND NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = listens.track_id)",
        [],
    )?;
    let by_tags = conn.execute(
        "UPDATE listens SET track_id = t.id, path = t.path FROM tracks t
         WHERE NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = listens.track_id)
           AND NOT (listens.title = '' AND listens.artist = '' AND listens.album = '')
           AND t.source = 'local'
           AND t.title = listens.title AND t.artist = listens.artist
           AND t.album = listens.album
           AND (SELECT COUNT(*) FROM tracks c WHERE c.source = 'local'
                AND c.title = listens.title AND c.artist = listens.artist
                AND c.album = listens.album) = 1",
        [],
    )?;
    Ok(by_path + by_tags)
}

/// One listen as it's recorded: the track's identity, when the play began
/// (unix seconds), its tags at play time, and the key that played in
/// fragment form (path#sub for a cue track), the reattach key.
pub struct Listen {
    pub track_id: i64,
    pub played_at: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub path: String,
}

/// Build the listen for a playing path from the live catalog. Ok(None)
/// when the path is not in the library: an unindexed file plays without
/// history, since events key to track identity. Works for plain files
/// only; a cue track's listen is built by the recorder from the row it
/// already resolved, since a bare path can't say which span played.
pub fn listen_for_path(
    conn: &Connection,
    path: &str,
    played_at: i64,
) -> rusqlite::Result<Option<Listen>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, album, genre FROM tracks
         WHERE source = 'local' AND path = ?1 AND sub = 0",
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
            path: path.to_string(),
        })),
        None => Ok(None),
    }
}

/// Append one event row. Append-only: nothing ever updates or deletes a
/// listen; [`reattach`] only re-ties the track join.
pub fn append(conn: &Connection, listen: &Listen) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    stmt.execute(rusqlite::params![
        listen.track_id,
        listen.played_at,
        listen.title,
        listen.artist,
        listen.album,
        listen.genre,
        listen.path,
    ])?;
    Ok(())
}

/// One track's line in a history view. Recent rows hold one event each
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
    /// The stream's sample rate in Hz and bits per sample, for the album
    /// headings' quality line; live-catalog only like the fields above.
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub rating: u8,
    /// The file path, for the cover column's thumbnail: the live catalog's
    /// while the track exists, the snapshot's once it is gone, so a pruned
    /// file whose bytes are still on disk keeps its cover.
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
        sample_rate_hz: row.get(12)?,
        bit_depth: row.get(13)?,
        rating: row.get(14)?,
        path: row.get(15)?,
    })
}

/// The tag columns of a listen read: title, artist, album, and the file
/// path from the live catalog while the track exists, the snapshot once it
/// is gone, then the album grouping and column metadata from the live
/// catalog only.
const SNAPSHOT_COLUMNS: &str = "COALESCE(t.title, l.title),
     COALESCE(t.artist, l.artist), COALESCE(t.album, l.album),
     COALESCE(t.album_artist, ''), COALESCE(t.year, 0), COALESCE(t.genre, ''),
     COALESCE(t.duration_ms, 0), COALESCE(t.codec, ''), COALESCE(t.bitrate, 0),
     COALESCE(t.sample_rate, 0), COALESCE(t.bit_depth, 0),
     COALESCE(t.rating, 0), COALESCE(t.path, l.path)";

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

/// What the never-played read orders by. Browse is the canonical album
/// artist, album, disc, track order [`crate::store::all_ids`] reads in;
/// the rest sort on one column with that order as the tie-break, so equal
/// keys stay browsable.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum NeverOrder {
    #[default]
    Browse,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Rating,
    Added,
}

/// The canonical browse order, and what every other key falls back to on
/// a tie.
const BROWSE_ORDER: &str = "album_artist, album, disc_no, track_no";

impl NeverOrder {
    /// The ORDER BY expression this key sorts on. Text sorts fold case, so
    /// a lowercase title sorts among its peers rather than after Z.
    fn column(self) -> &'static str {
        match self {
            NeverOrder::Browse => BROWSE_ORDER,
            NeverOrder::Title => "title COLLATE NOCASE",
            NeverOrder::Artist => "artist COLLATE NOCASE",
            NeverOrder::Album => "album COLLATE NOCASE",
            NeverOrder::Year => "year",
            NeverOrder::Duration => "duration_ms",
            NeverOrder::Rating => "rating",
            NeverOrder::Added => "added",
        }
    }
}

/// Library tracks no event has ever named. Local rows only, the bound
/// [`crate::store::all_ids`] reads the browse order under. The order runs
/// over the whole set before the limit cuts it, so a sort picks the top of
/// the library rather than re-arranging the first page of the browse order.
pub fn never_played(
    conn: &Connection,
    order: NeverOrder,
    descending: bool,
    limit: usize,
) -> rusqlite::Result<Vec<TrackPlays>> {
    let dir = if descending { " DESC" } else { "" };
    // The fragments come from the match above, never from a caller's string.
    let by = match order {
        // Browse is already a four-column order; reversing it means
        // reversing each part, not appending itself as a tie-break.
        NeverOrder::Browse if descending => {
            "album_artist DESC, album DESC, disc_no DESC, track_no DESC".to_string()
        }
        NeverOrder::Browse => BROWSE_ORDER.to_string(),
        other => format!("{}{dir}, {BROWSE_ORDER}", other.column()),
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, 0, 0, title, artist, album,
                album_artist, year, genre, duration_ms, codec, bitrate,
                sample_rate, bit_depth, rating, path
         FROM tracks
         WHERE source = 'local' AND id NOT IN (SELECT track_id FROM listens)
         ORDER BY {by} LIMIT ?1"
    ))?;
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
/// text: the album rollup puts the album artist there (an album name
/// alone reads ambiguous), the others leave it empty.
#[derive(Clone)]
pub struct NamePlays {
    pub name: String,
    pub sub: String,
    pub plays: u64,
    /// A file under the name, for the row's cover: the live catalog's
    /// path where the group still has a local track, the snapshot's
    /// otherwise, so a pruned file whose bytes are still on disk keeps
    /// its art. Empty when neither has one.
    pub art: String,
}

/// Play counts grouped under one tag, most first, over the events at or
/// after `since` (0 counts them all), the stats panel's range knob.
/// Grouping goes through the live catalog first, so fixing a tag
/// re-buckets its history; untagged plays (empty name) stay out of the
/// list.
pub fn rollup(
    conn: &Connection,
    by: Rollup,
    since: i64,
    limit: usize,
    fold: bool,
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
    // Genre lists re-bucket their plays onto each value, and a folded
    // library merges case variants; either way the SQL groups are only
    // an intermediate, so they fetch unclipped and the limit applies to
    // the merged names below.
    let merges = fold || matches!(by, Rollup::Genre);
    let clip = if merges { i64::MAX } else { limit as i64 };
    // The row's cover comes off one file under the name: a local track
    // the group still has, or the newest snapshot path when every one of
    // them is gone. Which file it is doesn't matter for an album, and for
    // an artist or a genre one of their records is the point.
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT COALESCE(t.{column}, l.{column}) AS name, {sub}, COUNT(*) AS plays,
                COALESCE(MAX(CASE WHEN t.source = 'local' THEN t.path END), MAX(l.path)) AS art
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE l.played_at >= ?1 AND name <> ''
         GROUP BY name
         ORDER BY plays DESC, name LIMIT ?2"
    ))?;
    let rows = stmt.query_map([since, clip], |row| {
        Ok(NamePlays {
            name: row.get(0)?,
            sub: row.get(1)?,
            plays: row.get::<_, i64>(2)? as u64,
            art: row.get(3)?,
        })
    })?;
    if !merges {
        return rows.collect();
    }
    let groups: Vec<NamePlays> = rows.collect::<Result<_, _>>()?;
    // Merged tally per (folded) name; the display casing and sub follow
    // the variant with the most plays, ties to the smaller string so the
    // list stays stable across refreshes.
    struct Merged {
        name: String,
        sub: String,
        plays: u64,
        art: String,
        best: u64,
    }
    let mut merged: HashMap<String, Merged> = HashMap::new();
    let mut tally = |name: &str, sub: &str, plays: u64, art: &str| {
        let key = if fold {
            name.to_lowercase()
        } else {
            name.to_string()
        };
        let entry = merged.entry(key).or_insert_with(|| Merged {
            name: name.to_string(),
            sub: sub.to_string(),
            plays: 0,
            art: art.to_string(),
            best: 0,
        });
        entry.plays += plays;
        if plays > entry.best || (plays == entry.best && *name < *entry.name) {
            entry.best = plays;
            entry.name = name.to_string();
            entry.sub = sub.to_string();
            entry.art = art.to_string();
        }
    };
    for group in &groups {
        match by {
            Rollup::Genre => {
                // Aliases first, then dedup within one list, so "Rock;
                // rock" under a folded library and "DnB; Drum & Bass"
                // under an alias each count their plays once.
                let mut parts: Vec<String> = crate::genre::split(&group.name)
                    .map(crate::genre::resolve)
                    .collect();
                if fold {
                    parts.sort_unstable_by_key(|p| p.to_lowercase());
                    parts.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
                } else {
                    parts.sort_unstable();
                    parts.dedup();
                }
                for part in parts {
                    tally(&part, "", group.plays, &group.art);
                }
            }
            _ => tally(&group.name, &group.sub, group.plays, &group.art),
        }
    }
    let mut out: Vec<NamePlays> = merged
        .into_values()
        .map(|m| NamePlays {
            name: m.name,
            sub: m.sub,
            plays: m.plays,
            art: m.art,
        })
        .collect();
    out.sort_unstable_by(|a, b| b.plays.cmp(&a.plays).then_with(|| a.name.cmp(&b.name)));
    out.truncate(limit);
    Ok(out)
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

/// When each track was last heard, unix seconds. Tracks with no listens
/// stay out of the map, the same way [`counts`] leaves them out, so a
/// missing key reads as never played rather than as played at the epoch.
///
/// The other half of what a history-weighted continuation provider (ADR 17)
/// tiers on: [`counts`] says how often, this says how long ago, and the two
/// together are what sinks the album you played all week behind the record
/// you forgot you own.
pub fn last_played(conn: &Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT track_id, MAX(played_at) FROM listens GROUP BY track_id")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// When the first listen landed (unix seconds); None before any has.
/// The all-time chart picks its span off this.
pub fn earliest(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MIN(played_at) FROM listens", [], |row| row.get(0))
}

/// Listens bucketed over time for the chart: one count per `bucket`
/// seconds from `since` up to `now`, empty buckets included, so the
/// bars show the quiet stretches too.
pub fn histogram(
    conn: &Connection,
    since: i64,
    bucket: i64,
    now: i64,
) -> rusqlite::Result<Vec<u64>> {
    // A non-positive bucket has no bar width; bail before it divides.
    if bucket <= 0 {
        return Ok(Vec::new());
    }
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
        // A listen stamped past `now` (clock skew) goes in the last bar
        // rather than out of bounds.
        let index = (index.max(0) as usize).min(n - 1);
        counts[index] += count;
    }
    Ok(counts)
}

/// Resolve one rollup name back to its library tracks in the canonical
/// browse order, so a stats row can queue what it counts. Live local
/// catalog only: a deleted track's snapshot keeps its rows in the rollup
/// but has no file left to play, and another source's row has nothing to
/// open either.
pub fn ids_for_name(
    conn: &Connection,
    by: Rollup,
    name: &str,
    limit: usize,
    fold: bool,
) -> rusqlite::Result<Vec<i64>> {
    let column = match by {
        Rollup::Artist => "artist",
        Rollup::Album => "album",
        Rollup::Genre => "genre",
    };
    // A genre name is one value out of the "; " lists and a folded name
    // is a casing class, neither of which SQL equality finds; read the
    // rows in the same order and match in Rust. The exact artist and
    // album lookups keep the indexed query.
    if fold || matches!(by, Rollup::Genre) {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT id, {column} FROM tracks WHERE source = 'local'
             ORDER BY album_artist, album, disc_no, track_no"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, value) = row?;
            let hit = match by {
                Rollup::Genre => crate::genre::has(&value, name, fold),
                _ => crate::value_eq(&value, name, fold),
            };
            if hit {
                out.push(id);
                if out.len() == limit {
                    break;
                }
            }
        }
        return Ok(out);
    }
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id FROM tracks WHERE source = 'local' AND {column} = ?1
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
            sub: 0,
            cue: None,
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
            sample_rate_hz: 0,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
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

        let never = never_played(&conn, NeverOrder::Browse, false, 10).unwrap();
        assert_eq!(never.len(), 1);
        assert_eq!(never[0].title, "Two");

        let genres = rollup(&conn, Rollup::Genre, 0, 10, false).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("rock", 2), ("jazz", 1)]
        );
        let albums = rollup(&conn, Rollup::Album, 0, 10, false).unwrap();
        assert_eq!(
            (albums[0].name.as_str(), albums[0].sub.as_str()),
            ("First", "A"),
            "the album rollup carries the album artist"
        );
        assert!(
            albums.iter().all(|a| a.art.starts_with("/m/")),
            "and a file under the name, for the row's cover"
        );

        let recent_genres = rollup(&conn, Rollup::Genre, 200, 10, false).unwrap();
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
            ids_for_name(&conn, Rollup::Artist, "A", 10, false)
                .unwrap()
                .len(),
            2,
            "a rollup name resolves to its library tracks"
        );
        assert_eq!(
            ids_for_name(&conn, Rollup::Genre, "jazz", 10, false)
                .unwrap()
                .len(),
            1
        );

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

    /// A "; " genre list re-buckets its plays onto each value: the rollup
    /// splits before ranking, and the drilldown resolves a value back to
    /// every track whose list includes it.
    #[test]
    fn genre_rollup_splits_lists() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "Rock; Shoegaze"),
                track("/m/2.mp3", "Two", "A", "First", "Rock"),
                track("/m/3.mp3", "Three", "B", "Second", "Shoegaze"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/2.mp3", 200);
        listen(&conn, "/m/3.mp3", 300);
        listen(&conn, "/m/3.mp3", 400);

        let genres = rollup(&conn, Rollup::Genre, 0, 10, false).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("Shoegaze", 3), ("Rock", 2)],
            "the list track counts under both of its values"
        );
        // The limit clips the split values, not the raw list strings.
        assert_eq!(rollup(&conn, Rollup::Genre, 0, 1, false).unwrap().len(), 1);

        assert_eq!(
            ids_for_name(&conn, Rollup::Genre, "Shoegaze", 10, false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            ids_for_name(&conn, Rollup::Genre, "Rock; Shoegaze", 10, false)
                .unwrap()
                .len(),
            0,
            "the raw list string is not a rollup name"
        );
    }

    /// A folded rollup merges case variants under one name displaying the
    /// most-played casing, and the drilldown resolves across casings.
    #[test]
    fn folded_rollup_merges_case_variants() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "Neu!", "First", "Krautrock"),
                track("/m/2.mp3", "Two", "neu!", "First", "krautrock"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/1.mp3", 200);
        listen(&conn, "/m/2.mp3", 300);

        let exact = rollup(&conn, Rollup::Artist, 0, 10, false).unwrap();
        assert_eq!(exact.len(), 2, "exact keeps casings apart");

        let artists = rollup(&conn, Rollup::Artist, 0, 10, true).unwrap();
        assert_eq!(
            artists
                .iter()
                .map(|a| (a.name.as_str(), a.plays))
                .collect::<Vec<_>>(),
            [("Neu!", 3)],
            "one line, the most-played casing"
        );
        let genres = rollup(&conn, Rollup::Genre, 0, 10, true).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("Krautrock", 3)]
        );

        assert_eq!(
            ids_for_name(&conn, Rollup::Artist, "Neu!", 10, true)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            ids_for_name(&conn, Rollup::Artist, "Neu!", 10, false)
                .unwrap()
                .len(),
            1
        );
    }

    /// The waiting set orders by any of its tag columns, and the sort runs
    /// in SQL so it picks the top of the library rather than re-arranging
    /// the page the limit already cut. Ties fall back to the browse order,
    /// which keeps a sort by year from scrambling the albums inside it.
    #[test]
    fn never_played_takes_a_sort() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        // Browse order runs Zebra, Mango, apple: the two A/First tracks by
        // track number, then the B/Second one.
        let mut zebra = track("/m/1.mp3", "Zebra", "A", "First", "rock");
        zebra.year = 2010;
        let mut mango = track("/m/2.mp3", "Mango", "A", "First", "rock");
        mango.track_no = 2;
        mango.year = 1999;
        let mut apple = track("/m/3.mp3", "apple", "B", "Second", "rock");
        apple.year = 1999;
        store::insert_batch(&mut conn, &[zebra, mango, apple]).unwrap();

        let titles = |order, desc, limit| {
            never_played(&conn, order, desc, limit)
                .unwrap()
                .iter()
                .map(|t| t.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(NeverOrder::Browse, false, 10),
            ["Zebra", "Mango", "apple"]
        );
        assert_eq!(
            titles(NeverOrder::Browse, true, 10),
            ["apple", "Mango", "Zebra"],
            "descending browse reverses every part of the order"
        );
        assert_eq!(
            titles(NeverOrder::Title, false, 10),
            ["apple", "Mango", "Zebra"],
            "a title sort folds case, so a lowercase name lands among its peers"
        );
        assert_eq!(
            titles(NeverOrder::Year, false, 10),
            ["Mango", "apple", "Zebra"],
            "equal years keep the browse order between them"
        );
        assert_eq!(
            titles(NeverOrder::Title, false, 1),
            ["apple"],
            "the limit cuts after the sort, not before it"
        );
    }

    /// The browse order this reads down is the local library's, so a row
    /// from another source is not a track waiting to be heard. Nothing
    /// writes one yet; the schema says streaming sources will add rows
    /// rather than a table, and the first of them would otherwise turn up
    /// in a list of files to go and play.
    #[test]
    fn another_sources_row_is_not_waiting_to_be_heard() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        conn.execute(
            "INSERT INTO tracks (source, path, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
             VALUES ('stream', 'rox://1', 'Streamed', 'B', 'Second', 'jazz', 0, 1, 0, 0, 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            never_played(&conn, NeverOrder::Browse, false, 10)
                .unwrap()
                .iter()
                .map(|t| t.title.clone())
                .collect::<Vec<_>>(),
            ["One"]
        );
    }

    /// A rollup name resolves to tracks so a stats row can queue what it
    /// counts, which means it can only offer rows with a file behind them.
    /// Both lookups need the bound: the indexed one an exact artist takes,
    /// and the row scan a genre or a folded name falls back to.
    #[test]
    fn another_sources_row_is_not_queueable() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        conn.execute(
            "INSERT INTO tracks (source, path, title, artist, album_artist, album, genre, year,
                track_no, duration_ms, size, mtime)
             VALUES ('stream', 'rox://1', 'Streamed', 'A', 'A', 'First', 'rock', 0, 2, 0, 0, 0)",
            [],
        )
        .unwrap();
        let local = ids_for_name(&conn, Rollup::Artist, "A", 10, false).unwrap();
        assert_eq!(local.len(), 1, "the indexed lookup skips another source");
        assert_eq!(
            ids_for_name(&conn, Rollup::Artist, "a", 10, true).unwrap(),
            local,
            "so does the walk a folded name takes"
        );
        assert_eq!(
            ids_for_name(&conn, Rollup::Genre, "rock", 10, false).unwrap(),
            local,
            "and the one a genre takes"
        );
    }

    #[test]
    fn reattach_carries_history_to_a_returned_file() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "rock"),
                // A second track keeps MAX(id) alive across the delete, so
                // the returned file cannot just reuse its old rowid.
                track("/m/2.mp3", "Two", "A", "First", "rock"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/1.mp3", 200);

        // The file's row prunes and it comes back under a fresh id; its
        // two plays must follow rather than restart at zero.
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let new_id: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = '/m/1.mp3'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_ne!(new_id, 1, "the returned file lands under a fresh id");

        assert_eq!(reattach(&conn).unwrap(), 2);
        let most = most_played(&conn, 10).unwrap();
        assert_eq!((most[0].track_id, most[0].plays), (new_id, 2));
        assert!(
            never_played(&conn, NeverOrder::Browse, false, 10)
                .unwrap()
                .iter()
                .all(|t| t.track_id != new_id),
            "the returned file is not a stranger to its own history"
        );
    }

    #[test]
    fn reattach_keeps_a_rips_listens_on_their_own_tracks() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        // Two spans of one image with identical tags, so the tag fallback's
        // exactly-one guard can never answer and only the fragment snapshot
        // can say which track a listen belongs to.
        let cue_track = |sub: u16, start_ms: u32| {
            let mut row = track("/m/disc.flac", "Untitled", "A", "Rip", "rock");
            row.sub = sub;
            row.track_no = sub;
            row.cue = Some(crate::CueSlice {
                cue_path: "/m/disc.cue".into(),
                span: crate::cue::Span {
                    start_ms,
                    end_ms: Some(start_ms + 1000),
                },
            });
            row
        };
        // The keeper holds MAX(id) across the delete, the same move the
        // plain-file reattach test makes, so the returned rip can't just
        // reuse its old rowids and dodge the relink.
        store::insert_batch(
            &mut conn,
            &[
                cue_track(1, 0),
                cue_track(2, 1000),
                track("/m/keep.mp3", "Keeper", "B", "Other", "jazz"),
            ],
        )
        .unwrap();
        let id_for = |conn: &Connection, sub: u16| -> i64 {
            conn.query_row(
                "SELECT id FROM tracks WHERE path = '/m/disc.flac' AND sub = ?1",
                [sub],
                |row| row.get(0),
            )
            .unwrap()
        };
        for sub in [1u16, 2] {
            append(
                &conn,
                &Listen {
                    track_id: id_for(&conn, sub),
                    played_at: sub as i64 * 100,
                    title: "Untitled".into(),
                    artist: "A".into(),
                    album: "Rip".into(),
                    genre: "rock".into(),
                    path: format!("/m/disc.flac#{sub}"),
                },
            )
            .unwrap();
        }

        // The rip prunes and returns under fresh ids, the reattach scenario.
        conn.execute("DELETE FROM tracks WHERE path = '/m/disc.flac'", [])
            .unwrap();
        store::insert_batch(&mut conn, &[cue_track(1, 0), cue_track(2, 1000)]).unwrap();

        assert_eq!(reattach(&conn).unwrap(), 2);
        for sub in [1u16, 2] {
            let relinked: i64 = conn
                .query_row(
                    "SELECT track_id FROM listens WHERE path = ?1",
                    [format!("/m/disc.flac#{sub}")],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                relinked,
                id_for(&conn, sub),
                "each listen lands on the row of its own span"
            );
        }
    }

    #[test]
    fn snapshot_outlives_a_deleted_track() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[track("/m/1.mp3", "Gone", "A", "First", "rock")],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        conn.execute("DELETE FROM tracks", []).unwrap();

        let recent = recent(&conn, 0, 10).unwrap();
        assert_eq!(
            recent[0].title, "Gone",
            "the snapshot keeps the row readable"
        );
        assert_eq!(
            recent[0].path, "/m/1.mp3",
            "and the snapshot path keeps the cover column resolvable"
        );
        let artists = rollup(&conn, Rollup::Artist, 0, 10, false).unwrap();
        assert_eq!(artists[0].name, "A");
    }
}
