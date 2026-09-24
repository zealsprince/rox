//! ADR 11's listen history: an append-only events table. A row holds the
//! track id, when the play began, and a tag snapshot; reads resolve live
//! while the track exists, from the snapshot after. Every stat is derived
//! by SQL; nothing stores a counter as the source.
//!
//! Rows record their origin, so an import's invented plays
//! ([`ORIGIN_ESTIMATE`]) are never mistaken for real ones.
//!
//! Append-only covers the event: when it played and what the tags said
//! never change. Relinking the track id after a prune ([`reattach`], by the
//! recorded path) is maintenance, not history.

use std::collections::HashMap;

use rusqlite::Connection;

/// No foreign key: deleting a track keeps its history.
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

/// The store ladder's snapshot-paths step, listens half. Live rows backfill
/// from the catalog; dangling ones rely on the tag fallback.
pub(crate) fn add_path_snapshot(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE listens ADD COLUMN path TEXT NOT NULL DEFAULT '';
         UPDATE listens SET path = COALESCE(
             (SELECT t.path FROM tracks t
              WHERE t.id = listens.track_id AND t.source = 'local'), '');",
    )
}

/// The store ladder's listen-origin step. Rows from before it keep the empty
/// origin and count as rox's own: they can't be told apart.
///
/// The (track_id, played_at) index isn't unique: a database may already
/// hold a duplicate (a stuck recorder), so the import probes the pair
/// instead.
pub(crate) fn add_origin(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE listens ADD COLUMN origin TEXT NOT NULL DEFAULT '';
         CREATE INDEX IF NOT EXISTS listens_track_played ON listens (track_id, played_at);",
    )
}

/// A play rox watched, including every row from before origins existed.
pub const ORIGIN_LOCAL: &str = "";

/// Imported from Last.fm with its real second.
pub const ORIGIN_SCROBBLE: &str = "lastfm";

/// Invented so a play count adds up.
pub const ORIGIN_ESTIMATE: &str = "estimate";

/// Relink events whose track was pruned and returned: by recorded path, then
/// by tag snapshot only when it names exactly one track. Live events refresh
/// their path. None when nothing dangled.
pub fn reattach(conn: &Connection) -> rusqlite::Result<Option<usize>> {
    // Match on the fragment form (path#sub), or every listen of a rip attaches
    // to whichever row sorts first.
    conn.execute(
        "UPDATE listens SET path =
             CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END
         FROM tracks t
         WHERE t.id = listens.track_id AND t.source = 'local'
           AND listens.path <>
             CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END",
        [],
    )?;
    // The matchers are expensive and this runs after every scan and reindex;
    // one indexed probe gates them.
    if !has_dangling(conn)? {
        return Ok(None);
    }
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
    Ok(Some(by_path + by_tags))
}

fn has_dangling(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM listens
            WHERE NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = listens.track_id))",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|found| found == 1)
}

/// `path` is the fragment form that played (path#sub for a cue track), the
/// reattach key.
pub struct Listen {
    pub track_id: i64,
    pub played_at: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub path: String,
}

/// Ok(None) when the path isn't in the library. Plain files only: a bare
/// path can't say which cue span played.
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

/// Nothing ever updates or deletes a listen; [`reattach`] only re-ties the
/// track join.
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

/// Guards against runaway inserts from a corrupt or hostile response.
pub const MAX_IMPORTED_PLAYS: u32 = 50_000;

/// Anchor for a never-played track's invented history, far enough back to
/// stay out of recent views and ADR 17's recency tiering.
pub const UNPLAYED_ANCHOR_OFFSET_SECS: i64 = 90 * 86_400;

/// How far back invented plays spread without a registration date: wide
/// enough that they don't pile into one bar of a weekly chart.
pub const FALLBACK_SPAN_SECS: i64 = 5 * 365 * 86_400;

/// Bounds an invented ladder: now, and the account's Last.fm registration.
#[derive(Clone, Copy, Debug)]
pub struct Ladder {
    pub now: i64,
    pub since: Option<i64>,
}

impl Ladder {
    /// No registration date.
    pub fn at(now: i64) -> Ladder {
        Ladder { now, since: None }
    }

    /// Widened past the registration when there's less than a second per rung.
    fn floor(&self, anchor: i64, needed: usize) -> i64 {
        let since = self
            .since
            .unwrap_or_else(|| self.now.saturating_sub(FALLBACK_SPAN_SECS));
        since.min(anchor.saturating_sub(needed as i64))
    }
}

/// At least a second, so every rung is its own moment.
fn ladder_step(anchor: i64, floor: i64, needed: usize) -> i64 {
    let span = anchor.saturating_sub(floor).max(needed as i64);
    (span / needed as i64).max(1)
}

/// A per-track offset into the first step, derived from the id. Without it
/// every track short one play lands on the same second: that put twenty
/// thousand invented listens on one afternoon. Derived, not random, so a
/// re-import lands where the last one did.
fn ladder_phase(track_id: i64, step: i64) -> i64 {
    // splitmix64's finalizer: sequential ids need mixing before a modulo.
    let mut z = (track_id as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // Never zero: a rung on the anchor would claim to be as recent as the real
    // play above it.
    1 + (z % step as u64) as i64
}

/// Past [`MAX_IMPORTED_PLAYS`] is a response nobody should trust: clamp, and
/// tell the caller.
fn capped(target: u32) -> (u32, bool) {
    (target.min(MAX_IMPORTED_PLAYS), target > MAX_IMPORTED_PLAYS)
}

/// The scrobble history import: each pair becomes a listen at exactly that
/// second, tagged [`ORIGIN_SCROBBLE`]. Idempotent by the pair. A stop keeps
/// what was written.
pub fn import_scrobbles<F>(
    conn: &mut Connection,
    plays: &[(i64, i64)],
    mut on_progress: F,
) -> rusqlite::Result<usize>
where
    F: FnMut(usize, usize) -> bool,
{
    if plays.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let mut seen_stmt =
        tx.prepare_cached("SELECT 1 FROM listens WHERE track_id = ?1 AND played_at = ?2")?;
    let mut track_stmt = tx.prepare_cached(
        "SELECT title, artist, album, genre,
                CASE WHEN sub = 0 THEN path ELSE path || '#' || sub END
         FROM tracks WHERE id = ?1",
    )?;
    let mut insert_stmt = tx.prepare_cached(
        "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    // One lookup per track: a heavy account scrobbles the same few hundred
    // tracks thousands of times.
    let mut tags: HashMap<i64, (String, String, String, String, String)> = HashMap::new();
    let mut added = 0usize;
    let total = plays.len();
    for (index, &(track_id, played_at)) in plays.iter().enumerate() {
        if !on_progress(index, total) {
            break;
        }
        if seen_stmt.exists(rusqlite::params![track_id, played_at])? {
            continue;
        }
        let entry = match tags.entry(track_id) {
            std::collections::hash_map::Entry::Occupied(found) => found.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let row = track_stmt.query_row([track_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                });
                // A track gone since the match waits for the next run rather than landing
                // tagless.
                let Ok(row) = row else { continue };
                slot.insert(row)
            }
        };
        let (title, artist, album, genre, path) = entry.clone();
        insert_stmt.execute(rusqlite::params![
            track_id,
            played_at,
            title,
            artist,
            album,
            genre,
            path,
            ORIGIN_SCROBBLE,
        ])?;
        added += 1;
    }
    on_progress(total, total);

    drop(seen_stmt);
    drop(track_stmt);
    drop(insert_stmt);
    tx.commit()?;
    Ok(added)
}

/// Invent listens up to each target count, spread evenly below the earliest
/// real one and phased per track ([`ladder_phase`]). Tagged
/// [`ORIGIN_ESTIMATE`]: only `user.getRecentTracks` dates a play, and this
/// is the fallback for counts with no dates.
pub fn backfill_plays_batch<F>(
    conn: &mut Connection,
    targets: &[(i64, u32)],
    ladder: Ladder,
    mut on_progress: F,
) -> rusqlite::Result<usize>
where
    F: FnMut(usize, usize) -> bool,
{
    if targets.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let mut count_stmt =
        tx.prepare_cached("SELECT COUNT(*), MIN(played_at) FROM listens WHERE track_id = ?1")?;
    let mut track_stmt = tx.prepare_cached(
        "SELECT title, artist, album, genre,
                CASE WHEN sub = 0 THEN path ELSE path || '#' || sub END
         FROM tracks WHERE id = ?1",
    )?;
    let mut insert_stmt = tx.prepare_cached(
        "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path, origin)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    let mut total_added = 0usize;
    let total = targets.len();
    for (idx, &(track_id, target_plays)) in targets.iter().enumerate() {
        if !on_progress(idx, total) {
            break;
        }
        if target_plays == 0 {
            continue;
        }
        let (target_plays, clamped) = capped(target_plays);
        if clamped {
            log::warn!(
                "listens: track {track_id} claims more plays than anyone has, \
                 clamping the backfill to {MAX_IMPORTED_PLAYS}"
            );
        }
        let (current_count, min_played): (u32, Option<i64>) = count_stmt
            .query_row([track_id], |row| {
                Ok((row.get::<_, i64>(0)? as u32, row.get::<_, Option<i64>>(1)?))
            })?;
        if current_count >= target_plays {
            continue;
        }
        let needed = (target_plays - current_count) as usize;
        let track_info = track_stmt.query_row([track_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        });
        let Ok((title, artist, album, genre, path)) = track_info else {
            continue;
        };

        // Hang below every existing play, so nothing invented is ever the most
        // recent.
        let anchor =
            min_played.unwrap_or_else(|| ladder.now.saturating_sub(UNPLAYED_ANCHOR_OFFSET_SECS));
        let floor = ladder.floor(anchor, needed);
        let step = ladder_step(anchor, floor, needed);
        let phase = ladder_phase(track_id, step);
        for i in 0..needed {
            let played_at = anchor.saturating_sub(i as i64 * step).saturating_sub(phase);
            insert_stmt.execute(rusqlite::params![
                track_id,
                played_at,
                title,
                artist,
                album,
                genre,
                path,
                ORIGIN_ESTIMATE,
            ])?;
        }
        total_added += needed;
    }
    on_progress(total, total);

    drop(count_stmt);
    drop(track_stmt);
    drop(insert_stmt);
    tx.commit()?;
    Ok(total_added)
}

/// Recent rows hold one event each; rollup rows aggregate; never-played
/// rows have zero for both.
#[derive(Clone)]
pub struct TrackPlays {
    pub track_id: i64,
    pub plays: u64,
    pub last_played: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Live-catalog only from here on; empty or zero once the track is gone.
    pub album_artist: String,
    pub year: u16,
    pub genre: String,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub rating: u8,
    /// Falls back to the snapshot, so a pruned file on disk keeps its cover.
    pub path: String,
    /// A station's listen carries the song's own tags, so this is the only way
    /// to tell. False once the track is gone.
    pub live: bool,
    /// Empty once the track is gone: the snapshot never kept it.
    pub source: String,
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
        live: row.get(16)?,
        source: row.get(17)?,
    })
}

/// The tags resolve live, snapshot as fallback, except on a live row: there
/// the row is the station and the snapshot is the song that was on, so the
/// snapshot wins. A dangling listen reads the snapshot.
const SNAPSHOT_COLUMNS: &str = "CASE WHEN t.remote_live THEN l.title
         ELSE COALESCE(t.title, l.title) END,
     CASE WHEN t.remote_live THEN l.artist
         ELSE COALESCE(t.artist, l.artist) END,
     CASE WHEN t.remote_live THEN l.album
         ELSE COALESCE(t.album, l.album) END,
     COALESCE(t.album_artist, ''), COALESCE(t.year, 0), COALESCE(t.genre, ''),
     COALESCE(t.duration_ms, 0), COALESCE(t.codec, ''), COALESCE(t.bitrate, 0),
     COALESCE(t.sample_rate, 0), COALESCE(t.bit_depth, 0),
     COALESCE(t.rating, 0), COALESCE(t.path, l.path),
     COALESCE(t.remote_live, 0), COALESCE(t.source, '')";

/// Newest first; 0 and i64::MAX read everything.
pub fn recent(
    conn: &Connection,
    since: i64,
    until: i64,
    limit: usize,
) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT l.track_id, 1, l.played_at, {SNAPSHOT_COLUMNS}
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE l.played_at >= ?1 AND l.played_at < ?2
         ORDER BY l.played_at DESC, l.id DESC LIMIT ?3"
    ))?;
    let rows = stmt.query_map([since, until, limit as i64], track_plays_row)?;
    rows.collect()
}

/// Bare snapshot columns come from the MAX(played_at) row, per SQLite's
/// documented min/max behavior.
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

/// Every non-browse key ties back to the browse order.
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

const BROWSE_ORDER: &str = "album_artist, album, disc_no, track_no";

impl NeverOrder {
    /// Text sorts fold case.
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

/// Local rows only. The sort runs before the limit, so it picks the top of
/// the library.
pub fn never_played(
    conn: &Connection,
    order: NeverOrder,
    descending: bool,
    limit: usize,
) -> rusqlite::Result<Vec<TrackPlays>> {
    let dir = if descending { " DESC" } else { "" };
    // The fragments come from the match above, never from a caller's string.
    let by = match order {
        // Reversing browse means reversing each of its four parts.
        NeverOrder::Browse if descending => {
            "album_artist DESC, album DESC, disc_no DESC, track_no DESC".to_string()
        }
        NeverOrder::Browse => BROWSE_ORDER.to_string(),
        other => format!("{}{dir}, {BROWSE_ORDER}", other.column()),
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, 0, 0, title, artist, album,
                album_artist, year, genre, duration_ms, codec, bitrate,
                sample_rate, bit_depth, rating, path, 0, source
         FROM tracks
         WHERE source = 'local' AND id NOT IN (SELECT track_id FROM listens)
         ORDER BY {by} LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit as i64], track_plays_row)?;
    rows.collect()
}

#[derive(Clone, Copy)]
pub enum Rollup {
    Artist,
    Album,
    Genre,
}

/// `sub` is secondary text: the album artist on an album rollup.
#[derive(Clone)]
pub struct NamePlays {
    pub name: String,
    pub sub: String,
    pub plays: u64,
    /// A file under the name for the cover, snapshot path as fallback.
    pub art: String,
}

/// None for a track with no events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackSummary {
    pub first_played: i64,
    pub last_played: i64,
    pub recent_plays: u64,
}

pub fn track_summary(
    conn: &Connection,
    track_id: i64,
    since: i64,
) -> rusqlite::Result<Option<TrackSummary>> {
    let mut stmt = conn.prepare_cached(
        "SELECT MIN(played_at), MAX(played_at),
                SUM(CASE WHEN played_at >= ?2 THEN 1 ELSE 0 END)
         FROM listens WHERE track_id = ?1",
    )?;
    let row = stmt.query_row(rusqlite::params![track_id, since], |r| {
        Ok((
            r.get::<_, Option<i64>>(0)?,
            r.get::<_, Option<i64>>(1)?,
            r.get::<_, Option<i64>>(2)?,
        ))
    })?;
    Ok(match row {
        (Some(first_played), Some(last_played), recent) => Some(TrackSummary {
            first_played,
            last_played,
            recent_plays: recent.unwrap_or(0).max(0) as u64,
        }),
        _ => None,
    })
}

/// Play counts per tag value, most first, within [since, until). Groups by
/// the live catalog first; untagged plays stay out.
pub fn rollup(
    conn: &Connection,
    by: Rollup,
    since: i64,
    until: i64,
    limit: usize,
    fold: bool,
) -> rusqlite::Result<Vec<NamePlays>> {
    let column = match by {
        Rollup::Artist => "artist",
        Rollup::Album => "album",
        Rollup::Genre => "genre",
    };
    // A deleted track's snapshot has no album artist; fall back to the artist.
    let sub = match by {
        Rollup::Album => "MAX(COALESCE(t.album_artist, l.artist))",
        _ => "''",
    };
    // Genre lists and folded names merge after the SQL, so fetch unclipped and
    // limit the merged list.
    let merges = fold || matches!(by, Rollup::Genre);
    let clip = if merges { i64::MAX } else { limit as i64 };
    // Cover: a local track the group still has, else the newest snapshot path.
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT COALESCE(t.{column}, l.{column}) AS name, {sub}, COUNT(*) AS plays,
                COALESCE(MAX(CASE WHEN t.source = 'local' THEN t.path END), MAX(l.path)) AS art
         FROM listens l LEFT JOIN tracks t ON t.id = l.track_id
         WHERE l.played_at >= ?1 AND l.played_at < ?2 AND name <> ''
         GROUP BY name
         ORDER BY plays DESC, name LIMIT ?3"
    ))?;
    let rows = stmt.query_map([since, until, clip], |row| {
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
    // Display follows the most-played variant, ties to the smaller string.
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
                // Resolve aliases, then dedup within one list, so each play counts once.
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

/// Tracks with no listens stay out of the map.
pub fn counts(conn: &Connection) -> rusqlite::Result<HashMap<i64, u32>> {
    let mut stmt =
        conn.prepare_cached("SELECT track_id, COUNT(*) FROM listens GROUP BY track_id")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u32))
    })?;
    rows.collect()
}

/// Tracks with no listens stay out, so a missing key means never played.
/// ADR 17's history-weighted continuation tiers on this and [`counts`].
pub fn last_played(conn: &Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT track_id, MAX(played_at) FROM listens GROUP BY track_id")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

pub fn earliest(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row("SELECT MIN(played_at) FROM listens", [], |row| row.get(0))
}

/// One count per `bucket` from `since` through `end`, empty buckets
/// included. A listen past `end` but under `until` lands in the last bar.
pub fn histogram(
    conn: &Connection,
    since: i64,
    bucket: i64,
    end: i64,
    until: i64,
) -> rusqlite::Result<Vec<u64>> {
    if bucket <= 0 {
        return Ok(Vec::new());
    }
    let n = ((end - since) / bucket).max(0) as usize + 1;
    let mut counts = vec![0u64; n];
    let mut stmt = conn.prepare_cached(
        "SELECT (played_at - ?1) / ?2 AS bucket, COUNT(*) FROM listens
         WHERE played_at >= ?1 AND played_at < ?3 GROUP BY bucket",
    )?;
    let rows = stmt.query_map([since, bucket, until], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u64))
    })?;
    for row in rows {
        let (index, count) = row?;
        let index = (index.max(0) as usize).min(n - 1);
        counts[index] += count;
    }
    Ok(counts)
}

/// Live local tracks only: a snapshot or a non-local row has no file to
/// queue.
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
    // Genre values and folded names aren't SQL equality; match in Rust.
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
    // usize::MAX would cast to a negative LIMIT.
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt.query_map(rusqlite::params![name, limit], |row| row.get(0))?;
    rows.collect()
}

pub fn count_between(conn: &Connection, since: i64, until: i64) -> rusqlite::Result<u64> {
    conn.query_row(
        "SELECT COUNT(*) FROM listens WHERE played_at >= ?1 AND played_at < ?2",
        [since, until],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n as u64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    /// Last.fm scrobbles and invented rows. Plays rox watched stay.
    Imported,
    Everything,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub total: u64,
    pub imported: u64,
}

/// Pre-origin rows count as rox's own: calling them imported would delete
/// history on a guess.
pub fn tally(conn: &Connection) -> rusqlite::Result<Tally> {
    conn.query_row(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE origin <> ?1) FROM listens",
        [ORIGIN_LOCAL],
        |row| {
            Ok(Tally {
                total: row.get::<_, i64>(0)? as u64,
                imported: row.get::<_, i64>(1)? as u64,
            })
        },
    )
}

/// The one delete in an append-only module; only ever behind a confirm.
pub fn clear(conn: &Connection, what: Clear) -> rusqlite::Result<usize> {
    let gone = match what {
        Clear::Imported => {
            conn.execute("DELETE FROM listens WHERE origin <> ?1", [ORIGIN_LOCAL])?
        }
        Clear::Everything => conn.execute("DELETE FROM listens", [])?,
    };
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackRow, store};

    fn track(path: &str, title: &str, artist: &str, album: &str, genre: &str) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
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

        let all = recent(&conn, 0, i64::MAX, 10).unwrap();
        assert_eq!(
            all.iter().map(|r| r.last_played).collect::<Vec<_>>(),
            [300, 200, 100],
            "recent runs newest first"
        );
        assert_eq!(
            recent(&conn, 200, i64::MAX, 10).unwrap().len(),
            2,
            "a range bound drops older events"
        );
        assert_eq!(
            recent(&conn, 0, 300, 10).unwrap().len(),
            2,
            "and the upper edge is exclusive"
        );

        let most = most_played(&conn, 10).unwrap();
        assert_eq!((most[0].title.as_str(), most[0].plays), ("One", 2));

        let never = never_played(&conn, NeverOrder::Browse, false, 10).unwrap();
        assert_eq!(never.len(), 1);
        assert_eq!(never[0].title, "Two");

        let genres = rollup(&conn, Rollup::Genre, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("rock", 2), ("jazz", 1)]
        );
        let albums = rollup(&conn, Rollup::Album, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(
            (albums[0].name.as_str(), albums[0].sub.as_str()),
            ("First", "A"),
            "the album rollup carries the album artist"
        );
        assert!(
            albums.iter().all(|a| a.art.starts_with("/m/")),
            "and a file under the name, for the row's cover"
        );

        let recent_genres = rollup(&conn, Rollup::Genre, 200, i64::MAX, 10, false).unwrap();
        assert_eq!(
            recent_genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("jazz", 1), ("rock", 1)],
            "a range bound re-counts the rollup"
        );

        assert_eq!(count_between(&conn, 0, i64::MAX).unwrap(), 3);
        assert_eq!(count_between(&conn, 200, i64::MAX).unwrap(), 2);
        assert_eq!(count_between(&conn, 100, 300).unwrap(), 2);
        assert_eq!(
            rollup(&conn, Rollup::Genre, 0, 300, 10, false)
                .unwrap()
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("jazz", 1), ("rock", 1)],
            "an upper bound re-counts the rollup too"
        );

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
            histogram(&conn, 100, 100, 400, i64::MAX).unwrap(),
            [1, 1, 1, 0],
            "one count per bucket, empty buckets included"
        );
        assert_eq!(
            histogram(&conn, 0, 1000, 400, i64::MAX).unwrap(),
            [3],
            "one bucket swallows everything"
        );
        assert_eq!(
            histogram(&conn, 100, 100, 299, 300).unwrap(),
            [1, 1],
            "a bounded span stops at its edge instead of folding the rest in"
        );
    }

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

        let genres = rollup(&conn, Rollup::Genre, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(
            genres
                .iter()
                .map(|g| (g.name.as_str(), g.plays))
                .collect::<Vec<_>>(),
            [("Shoegaze", 3), ("Rock", 2)],
            "the list track counts under both of its values"
        );
        assert_eq!(
            rollup(&conn, Rollup::Genre, 0, i64::MAX, 1, false)
                .unwrap()
                .len(),
            1
        );

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

        let exact = rollup(&conn, Rollup::Artist, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(exact.len(), 2, "exact keeps casings apart");

        let artists = rollup(&conn, Rollup::Artist, 0, i64::MAX, 10, true).unwrap();
        assert_eq!(
            artists
                .iter()
                .map(|a| (a.name.as_str(), a.plays))
                .collect::<Vec<_>>(),
            [("Neu!", 3)],
            "one line, the most-played casing"
        );
        let genres = rollup(&conn, Rollup::Genre, 0, i64::MAX, 10, true).unwrap();
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

    #[test]
    fn never_played_takes_a_sort() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
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
                // A second track keeps MAX(id) alive, so the returned file can't reuse its
                // rowid.
                track("/m/2.mp3", "Two", "A", "First", "rock"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/1.mp3", 200);

        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let new_id: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = '/m/1.mp3'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_ne!(new_id, 1, "the returned file lands under a fresh id");

        assert_eq!(reattach(&conn).unwrap(), Some(2));
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
    fn reattach_gates_on_a_library_with_nothing_dangling() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "rock"),
                track("/m/2.mp3", "Two", "A", "First", "rock"),
            ],
        )
        .unwrap();
        listen(&conn, "/m/1.mp3", 100);
        listen(&conn, "/m/2.mp3", 200);
        assert_eq!(reattach(&conn).unwrap(), None, "nothing to match");
        assert_eq!(reattach(&conn).unwrap(), None);

        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        assert_eq!(reattach(&conn).unwrap(), Some(1));
        assert_eq!(reattach(&conn).unwrap(), None, "and it closes again");
    }

    #[test]
    fn reattach_keeps_a_rips_listens_on_their_own_tracks() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        // Identical tags, so only the fragment snapshot can relink them.
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
        // Keeps MAX(id) alive so the rip can't reuse its rowids.
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

        conn.execute("DELETE FROM tracks WHERE path = '/m/disc.flac'", [])
            .unwrap();
        store::insert_batch(&mut conn, &[cue_track(1, 0), cue_track(2, 1000)]).unwrap();

        assert_eq!(reattach(&conn).unwrap(), Some(2));
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

        let recent = recent(&conn, 0, i64::MAX, 10).unwrap();
        assert_eq!(
            recent[0].title, "Gone",
            "the snapshot keeps the row readable"
        );
        assert_eq!(
            recent[0].path, "/m/1.mp3",
            "and the snapshot path keeps the cover column resolvable"
        );
        let artists = rollup(&conn, Rollup::Artist, 0, i64::MAX, 10, false).unwrap();
        assert_eq!(artists[0].name, "A");
    }

    #[test]
    fn backfill_plays_fills_counts_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "rock"),
                track("/m/2.mp3", "Two", "A", "First", "rock"),
            ],
        )
        .unwrap();

        let track1: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();
        let track2: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/2.mp3"], |r| {
                r.get(0)
            })
            .unwrap();
        let now = 1_700_000_000i64;

        let added = backfill_plays_batch(
            &mut conn,
            &[(track1, 1000), (track2, 42)],
            Ladder::at(now),
            |_, _| true,
        )
        .unwrap();
        assert_eq!(added, 1042);

        let count_map = counts(&conn).unwrap();
        assert_eq!(count_map.get(&track1).copied(), Some(1000));
        assert_eq!(count_map.get(&track2).copied(), Some(42));

        let added_again = backfill_plays_batch(
            &mut conn,
            &[(track1, 1000), (track2, 42)],
            Ladder::at(now),
            |_, _| true,
        )
        .unwrap();
        assert_eq!(added_again, 0);

        let added_diff =
            backfill_plays_batch(&mut conn, &[(track1, 1005)], Ladder::at(now), |_, _| true)
                .unwrap();
        assert_eq!(added_diff, 5);
        let count_map_after = counts(&conn).unwrap();
        assert_eq!(count_map_after.get(&track1).copied(), Some(1005));
    }

    #[test]
    fn backfill_plays_preserves_existing_recent_timestamps() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let track_id: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        listen(&conn, "/m/1.mp3", 1_700_000_000);

        let added = backfill_plays_batch(
            &mut conn,
            &[(track_id, 10)],
            Ladder::at(1_700_050_000),
            |_, _| true,
        )
        .unwrap();
        assert_eq!(added, 9);

        let count_map = counts(&conn).unwrap();
        assert_eq!(count_map.get(&track_id).copied(), Some(10));

        let lp_map = last_played(&conn).unwrap();
        assert_eq!(lp_map.get(&track_id).copied(), Some(1_700_000_000));
    }

    #[test]
    fn backfill_plays_unplayed_tracks_anchored_in_past() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[track(
                "/m/unplayed.mp3",
                "Unplayed",
                "Artist",
                "Album",
                "pop",
            )],
        )
        .unwrap();
        let track_id: i64 = conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                ["/m/unplayed.mp3"],
                |r| r.get(0),
            )
            .unwrap();

        let now = 1_700_000_000i64;
        let added = backfill_plays_batch(&mut conn, &[(track_id, 5)], Ladder::at(now), |_, _| true)
            .unwrap();
        assert_eq!(added, 5);

        let lp_map = last_played(&conn).unwrap();
        let last = lp_map.get(&track_id).copied().unwrap();
        assert!(last <= now - UNPLAYED_ANCHOR_OFFSET_SECS);
    }

    #[test]
    fn backfill_plays_sanity_cap() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[track("/m/capped.mp3", "Capped", "Artist", "Album", "pop")],
        )
        .unwrap();
        let track_id: i64 = conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                ["/m/capped.mp3"],
                |r| r.get(0),
            )
            .unwrap();

        let now = 1_700_000_000i64;
        let added = backfill_plays_batch(
            &mut conn,
            &[(track_id, 1_000_000)],
            Ladder::at(now),
            |_, _| true,
        )
        .unwrap();
        assert_eq!(added, MAX_IMPORTED_PLAYS as usize);
    }

    #[test]
    fn imported_scrobbles_keep_their_own_seconds_and_survive_a_rerun() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First", "rock"),
                track("/m/2.mp3", "Two", "A", "First", "rock"),
            ],
        )
        .unwrap();
        let id = |path: &str| -> i64 {
            conn.query_row("SELECT id FROM tracks WHERE path = ?1", [path], |r| {
                r.get(0)
            })
            .unwrap()
        };
        let (one, two) = (id("/m/1.mp3"), id("/m/2.mp3"));

        let history = [
            (one, 1_700_000_000),
            (one, 1_600_000_000),
            (two, 1_650_000_000),
        ];
        assert_eq!(
            import_scrobbles(&mut conn, &history, |_, _| true).unwrap(),
            3
        );

        let played: Vec<i64> = recent(&conn, 0, i64::MAX, 10)
            .unwrap()
            .iter()
            .map(|row| row.last_played)
            .collect();
        assert_eq!(played, [1_700_000_000, 1_650_000_000, 1_600_000_000]);
        assert_eq!(counts(&conn).unwrap().get(&one).copied(), Some(2));

        assert_eq!(
            import_scrobbles(&mut conn, &history, |_, _| true).unwrap(),
            0
        );
        assert_eq!(counts(&conn).unwrap().get(&one).copied(), Some(2));

        let next = [(one, 1_700_000_000), (two, 1_710_000_000)];
        assert_eq!(import_scrobbles(&mut conn, &next, |_, _| true).unwrap(), 1);
        assert_eq!(
            counts(&conn).unwrap().get(&two).copied(),
            Some(2),
            "the second it hadn't seen lands beside the one it had"
        );
    }

    #[test]
    fn a_scrobble_import_stops_where_it_is_asked_to() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let one: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        let history = [(one, 100), (one, 200), (one, 300)];
        let added = import_scrobbles(&mut conn, &history, |done, _| done < 2).unwrap();
        assert_eq!(added, 2, "what it got through is kept");
        assert_eq!(counts(&conn).unwrap().get(&one).copied(), Some(2));
    }

    #[test]
    fn an_invented_ladder_spreads_across_the_accounts_own_span() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let one: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        let now = 1_700_000_000i64;
        let since = now - 4 * 365 * 86_400;
        let ladder = Ladder {
            now,
            since: Some(since),
        };
        assert_eq!(
            backfill_plays_batch(&mut conn, &[(one, 100)], ladder, |_, _| true).unwrap(),
            100
        );

        let mut played: Vec<i64> = conn
            .prepare("SELECT played_at FROM listens WHERE track_id = ?1 ORDER BY played_at")
            .unwrap()
            .query_map([one], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        played.sort_unstable();
        assert_eq!(played.len(), 100);
        assert!(
            played[0] >= since,
            "nothing lands before the account existed"
        );
        assert!(
            *played.last().unwrap() <= now - UNPLAYED_ANCHOR_OFFSET_SECS,
            "and nothing lands in the recent past"
        );

        let weeks: std::collections::HashSet<i64> =
            played.iter().map(|at| at / (7 * 86_400)).collect();
        assert!(
            weeks.len() > 50,
            "the ladder piled up: {} weeks for 100 plays",
            weeks.len()
        );
    }

    #[test]
    fn a_ladder_with_no_registration_still_spreads() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let one: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        let now = 1_700_000_000i64;
        backfill_plays_batch(&mut conn, &[(one, 20)], Ladder::at(now), |_, _| true).unwrap();
        let spread: i64 = conn
            .query_row(
                "SELECT MAX(played_at) - MIN(played_at) FROM listens WHERE track_id = ?1",
                [one],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            spread > 365 * 86_400,
            "a fallback span still has to be a span: {spread} seconds"
        );
    }

    #[test]
    fn a_tight_span_still_gives_every_rung_its_own_second() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let one: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        let now = 1_700_000_000i64;
        let ladder = Ladder {
            now,
            since: Some(now - 60),
        };
        backfill_plays_batch(&mut conn, &[(one, 10)], ladder, |_, _| true).unwrap();
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT played_at) FROM listens WHERE track_id = ?1",
                [one],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(distinct, 10, "no two rungs share a second");
    }

    #[test]
    fn ladders_from_one_import_dont_stack_on_the_same_seconds() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let rows: Vec<TrackRow> = (0..300)
            .map(|n| track(&format!("/m/{n}.mp3"), "Song", "A", "Alb", "pop"))
            .collect();
        store::insert_batch(&mut conn, &rows).unwrap();
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM tracks ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        // The shape behind the one-afternoon pile-up: many tracks short one play,
        // placed against one anchor and floor.
        let now = 1_700_000_000i64;
        let ladder = Ladder {
            now,
            since: Some(now - 10 * 365 * 86_400),
        };
        let targets: Vec<(i64, u32)> = ids.iter().map(|&id| (id, 1)).collect();
        assert_eq!(
            backfill_plays_batch(&mut conn, &targets, ladder, |_, _| true).unwrap(),
            300
        );

        let distinct: i64 = conn
            .query_row("SELECT COUNT(DISTINCT played_at) FROM listens", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(
            distinct > 290,
            "the ladders lined up again: 300 plays on {distinct} seconds"
        );
        let busiest: i64 = conn
            .query_row(
                "SELECT MAX(n) FROM (SELECT COUNT(*) n FROM listens GROUP BY played_at / 86400)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(busiest <= 3, "one day took {busiest} of the 300");
        let spread: i64 = conn
            .query_row(
                "SELECT MAX(played_at) - MIN(played_at) FROM listens",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            spread > 9 * 365 * 86_400,
            "a decade of account collapsed to {spread} seconds"
        );
    }

    #[test]
    fn a_clear_can_take_the_imported_rows_alone() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First", "rock")]).unwrap();
        let one: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        listen(&conn, "/m/1.mp3", 1_700_000_000);
        import_scrobbles(&mut conn, &[(one, 1_600_000_000)], |_, _| true).unwrap();
        backfill_plays_batch(&mut conn, &[(one, 8)], Ladder::at(1_700_000_000), |_, _| {
            true
        })
        .unwrap();

        let before = tally(&conn).unwrap();
        assert_eq!(before.total, 8);
        assert_eq!(before.imported, 7, "only the watched play is rox's own");

        assert_eq!(clear(&conn, Clear::Imported).unwrap(), 7);
        let after = tally(&conn).unwrap();
        assert_eq!((after.total, after.imported), (1, 0));
        assert_eq!(
            last_played(&conn).unwrap().get(&one).copied(),
            Some(1_700_000_000),
            "the play that survived is the one rox saw"
        );

        assert_eq!(clear(&conn, Clear::Everything).unwrap(), 1);
        assert_eq!(tally(&conn).unwrap(), Tally::default());
    }

    #[test]
    fn a_count_past_the_cap_says_so() {
        assert_eq!(capped(42), (42, false));
        assert_eq!(capped(MAX_IMPORTED_PLAYS), (MAX_IMPORTED_PLAYS, false));
        assert_eq!(
            capped(MAX_IMPORTED_PLAYS + 1),
            (MAX_IMPORTED_PLAYS, true),
            "past the cap it clamps, and the caller warns"
        );
    }

    #[test]
    fn backfill_plays_cancellation() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "Alb", "pop"),
                track("/m/2.mp3", "Two", "A", "Alb", "pop"),
            ],
        )
        .unwrap();
        let track1: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/1.mp3"], |r| {
                r.get(0)
            })
            .unwrap();
        let track2: i64 = conn
            .query_row("SELECT id FROM tracks WHERE path = ?1", ["/m/2.mp3"], |r| {
                r.get(0)
            })
            .unwrap();

        let now = 1_700_000_000i64;
        let added = backfill_plays_batch(
            &mut conn,
            &[(track1, 5), (track2, 5)],
            Ladder::at(now),
            |idx, _| idx < 1,
        )
        .unwrap();
        assert_eq!(added, 5);

        let count_map = counts(&conn).unwrap();
        assert_eq!(count_map.get(&track1).copied(), Some(5));
        assert_eq!(count_map.get(&track2).copied(), None);
    }

    /// A file's row is the song, so a retag re-buckets its history; a station's
    /// row is the station, so its snapshots name the songs.
    #[test]
    fn a_live_row_reads_its_snapshots() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[track("/m/1.mp3", "Typo", "A", "First", "rock")],
        )
        .unwrap();

        listen(&conn, "/m/1.mp3", 100);
        conn.execute("UPDATE tracks SET title = 'Fixed'", [])
            .unwrap();

        let mut station = track("https://host/stream", "The Station", "Radio", "", "");
        station.remote_url = "https://host/stream".into();
        station.remote_live = true;
        store::upsert_source_rows(&mut conn, "radio", &[station]).unwrap();
        let station_id = store::id_for_path(&conn, "radio", "https://host/stream")
            .unwrap()
            .unwrap();

        for (at, title, artist) in [
            (200, "Breathe", "The Prodigy"),
            (300, "Firestarter", "The Prodigy"),
        ] {
            append(
                &conn,
                &Listen {
                    track_id: station_id,
                    played_at: at,
                    title: title.into(),
                    artist: artist.into(),
                    album: "The Station".into(),
                    genre: String::new(),
                    path: String::new(),
                },
            )
            .unwrap();
        }

        let rows = recent(&conn, 0, i64::MAX, 10).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["Firestarter", "Breathe", "Fixed"],
            "the file follows its retagged row, each station play its own song"
        );
        assert_eq!(
            rows[0].artist, "The Prodigy",
            "the announced act, not the station's name"
        );
        assert_eq!(
            rows.iter().map(|r| r.live).collect::<Vec<_>>(),
            [true, true, false],
            "and the flag that tells the two kinds of row apart"
        );

        let played = most_played(&conn, 10).unwrap();
        let station_row = played
            .iter()
            .find(|r| r.track_id == station_id)
            .expect("the station's plays roll up");
        assert_eq!(station_row.plays, 2);
        assert_eq!(station_row.title, "Firestarter");
    }
}
