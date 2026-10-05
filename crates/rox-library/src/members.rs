//! Rows by membership, the plugin source shape from ADR 29's second
//! amendment. A plugin has no whole catalog to reconcile against: its rows
//! come from the collections the user syncs and the tracks they pick one at a
//! time. `source_members` records which collections hold each row, picks
//! living in a collection of their own ([`PICKED`]), and a row is pruned only
//! when nothing holds it or the user removes it.
//!
//! A pick plays a track without adding it (ADR 29, amended 2026-09-29): a
//! row held only by [`PICKED`] stays out of the library, and one the user
//! adds is recorded in `source_saved`, which holds it like a collection.
//! Picked-only rows nobody touched in [`PICK_KEEP_SECS`] expire.
//!
//! Writes here refuse local files, stations and Subsonic servers outright,
//! and every statement is scoped to the source it was handed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};

use crate::cue::{self, TrackKey};
use crate::replaygain::ReplayGain;
use crate::{TrackRow, bookmarks, listens, playlists, stations, store};

/// The collection single picks live in. Node ids from a plugin are never
/// empty, so it can't collide with a synced one.
pub const PICKED: &str = "";

/// How long a picked-only row survives without being picked or played.
pub const PICK_KEEP_SECS: i64 = 30 * 24 * 60 * 60;

/// Picked-only rows: held by [`PICKED`] and nothing else, and not saved.
/// Over `p`, a `source_members` alias.
const PICKED_ONLY: &str = "p.collection = ''
    AND NOT EXISTS (SELECT 1 FROM source_members m
                     WHERE m.source = p.source AND m.path = p.path AND m.collection <> '')
    AND NOT EXISTS (SELECT 1 FROM source_saved s
                     WHERE s.source = p.source AND s.path = p.path)";

/// One track as a plugin lists it. Every field is set, empty or zero when the
/// plugin doesn't know it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginTrack {
    /// Opaque to rox and stable across sessions, since it becomes the row's
    /// path.
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    /// The stream never ends: no duration, no gapless boundary.
    pub live: bool,
    /// The nodes Go to opens, as `rox-services` serializes them. Opaque here,
    /// and empty for none.
    pub go_to: String,
}

/// `(source, path)` backs the orphan prune, which asks per row whether any
/// collection still holds it.
pub(crate) fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS source_members (
            source     TEXT NOT NULL,
            collection TEXT NOT NULL,
            path       TEXT NOT NULL,
            position   INTEGER NOT NULL,
            PRIMARY KEY (source, collection, path)
        );
        CREATE INDEX IF NOT EXISTS source_members_path ON source_members (source, path);",
    )
}

/// Single tracks the user added to the library, apart from the collections
/// so no node id can collide with it.
pub(crate) fn add_saved(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS source_saved (
            source   TEXT NOT NULL,
            path     TEXT NOT NULL,
            saved_at INTEGER NOT NULL,
            PRIMARY KEY (source, path)
        );",
    )
}

/// Go to for kept rows (ADR 30, amended 2026-10-02): the nodes a row's album
/// and artists open, kept beside the row rather than as columns on `tracks`.
/// `source_resync` names the collections kept before this stored anything,
/// so their next sync ignores its token and fills it in once.
pub(crate) fn add_go_to(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS source_go_to (
            source TEXT NOT NULL,
            path   TEXT NOT NULL,
            go_to  TEXT NOT NULL,
            PRIMARY KEY (source, path)
        );
        CREATE TABLE IF NOT EXISTS source_resync (
            source     TEXT NOT NULL,
            collection TEXT NOT NULL,
            PRIMARY KEY (source, collection)
        );
        INSERT OR IGNORE INTO source_resync (source, collection)
            SELECT DISTINCT source, collection FROM source_members
             WHERE collection <> '{PICKED}';"
    ))
}

/// Plugin rows stored before [`album_artist_of`] took the track artist. Only
/// fills the empty ones, so an album artist a plugin did send stays.
pub(crate) fn backfill_album_artist(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET album_artist = artist
          WHERE substr(source, 1, ?1) = ?2 AND album_artist = ''",
        params![cue::PLUGIN_PREFIX.len(), cue::PLUGIN_PREFIX],
    )?;

    Ok(())
}

/// Falls back to the track artist, as the scanner does for a file with no album
/// artist tag. An empty one ranks first in the canonical order, so the row would
/// file ahead of every artist.
fn album_artist_of(track: &PluginTrack) -> &str {
    match track.album_artist.is_empty() {
        true => &track.artist,
        false => &track.album_artist,
    }
}

/// The row a plugin track becomes. It stores no stream URL: a plugin row
/// plays through the plugin, keyed by its path.
pub fn row_for(track: &PluginTrack, now: i64) -> TrackRow {
    TrackRow {
        path: track.key.clone(),
        sub: 0,
        cue: None,
        remote_url: String::new(),
        remote_live: track.live,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: album_artist_of(track).to_string(),
        album: track.album.clone(),
        title_sort: String::new(),
        artist_sort: String::new(),
        album_artist_sort: String::new(),
        album_sort: String::new(),
        genre: track.genre.clone(),
        year: track.year,
        disc_no: track.disc_no,
        track_no: track.track_no,
        duration_ms: track.duration_ms,
        codec: track.codec.clone(),
        bitrate_kbps: track.bitrate_kbps,
        sample_rate_hz: 0,
        bit_depth: 0,
        rating: 0,
        replay_gain: ReplayGain::default(),
        bpm: None,
        size: 0,
        // No file to stat; scans only ever walk local roots.
        mtime: now,
    }
}

/// Upsert `tracks` and hold each in [`PICKED`], answering their keys so the
/// caller can queue them. Never prunes: writing one row never removes
/// another, the rule `stations::put` follows.
pub fn pick(
    conn: &mut Connection,
    source: &str,
    tracks: &[PluginTrack],
) -> rusqlite::Result<Vec<TrackKey>> {
    refuse_other_shapes(source)?;

    if tracks.is_empty() {
        return Ok(Vec::new());
    }

    let tx = conn.transaction()?;
    upsert(&tx, source, tracks)?;
    hold(&tx, source, PICKED, tracks)?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    let id = cue::source_id(source);
    Ok(tracks
        .iter()
        .map(|track| TrackKey {
            source: id.clone(),
            path: PathBuf::from(&track.key),
            sub: 0,
        })
        .collect())
}

/// Add to Library: upsert `tracks` and save each, so they browse. Answers
/// their keys.
pub fn save(
    conn: &mut Connection,
    source: &str,
    tracks: &[PluginTrack],
) -> rusqlite::Result<Vec<TrackKey>> {
    refuse_other_shapes(source)?;

    if tracks.is_empty() {
        return Ok(Vec::new());
    }

    let tx = conn.transaction()?;
    upsert(&tx, source, tracks)?;
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO source_saved (source, path, saved_at) VALUES (?1, ?2, ?3)",
        )?;
        let now = unix_now();
        for track in tracks {
            insert.execute(params![source, track.key, now])?;
        }
    }
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    let id = cue::source_id(source);
    Ok(tracks
        .iter()
        .map(|track| TrackKey {
            source: id.clone(),
            path: PathBuf::from(&track.key),
            sub: 0,
        })
        .collect())
}

/// Remove from Library: let go of saved tracks. A row still held by a pick
/// or a collection stays; one held by nothing is pruned. Answers how many
/// rows went.
pub fn unsave(conn: &mut Connection, source: &str, paths: &[String]) -> rusqlite::Result<usize> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    {
        let mut delete =
            tx.prepare_cached("DELETE FROM source_saved WHERE source = ?1 AND path = ?2")?;
        for path in paths {
            delete.execute(params![source, path])?;
        }
    }
    let pruned = prune_orphans(&tx, source)?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    Ok(pruned)
}

/// The track ids of every picked-only row, which stay out of the library.
pub fn picked_only_ids(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let sql = format!(
        "SELECT t.id FROM source_members p
           JOIN tracks t ON t.source = p.source AND t.path = p.path
          WHERE {PICKED_ONLY}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt.query_map([], |r| r.get(0))?;

    ids.collect()
}

/// The track ids of every saved row, for Remove from Library.
pub fn saved_ids(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT t.id FROM source_saved s
           JOIN tracks t ON t.source = s.source AND t.path = s.path",
    )?;
    let ids = stmt.query_map([], |r| r.get(0))?;

    ids.collect()
}

/// What keeps a set of rows in the library, for taking them back out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// `(source, path)` of each row the user added on its own.
    pub saved: Vec<(String, String)>,
    /// `(source, collection)` of each kept collection holding any of the
    /// rows. [`PICKED`] is never one: a pick doesn't keep a row.
    pub kept: Vec<(String, String)>,
}

impl Holds {
    pub fn is_empty(&self) -> bool {
        self.saved.is_empty() && self.kept.is_empty()
    }
}

/// The saved rows among `ids` and the kept collections holding any of them,
/// each once, in the order the ids first reach them. Rows of other source
/// shapes hold nothing here and drop out.
pub fn holds(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Holds> {
    let mut row = conn.prepare_cached(
        "SELECT t.source, t.path,
                EXISTS (SELECT 1 FROM source_saved s
                         WHERE s.source = t.source AND s.path = t.path)
           FROM tracks t WHERE t.id = ?1",
    )?;
    let mut collections = conn.prepare_cached(
        "SELECT collection FROM source_members
          WHERE source = ?1 AND path = ?2 AND collection <> ''
          ORDER BY collection",
    )?;

    let mut holds = Holds::default();
    let (mut saved_seen, mut kept_seen) = (HashSet::new(), HashSet::new());

    for &id in ids {
        let found: Option<(String, String, bool)> = row
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let Some((source, path, saved)) = found else {
            continue;
        };
        if cue::Origin::of(&source) != cue::Origin::Plugin {
            continue;
        }

        let kept = collections
            .query_map(params![source, path], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for collection in kept {
            let pair = (source.clone(), collection);
            if kept_seen.insert(pair.clone()) {
                holds.kept.push(pair);
            }
        }

        let pair = (source, path);
        if saved && saved_seen.insert(pair.clone()) {
            holds.saved.push(pair);
        }
    }

    Ok(holds)
}

/// How many of a source's rows are in the library, and how many of those
/// the user added on their own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InLibrary {
    /// Rows a kept collection holds or the user saved. Picked-only rows
    /// stay out of the library, so they don't count.
    pub tracks: usize,
    pub saved: usize,
}

pub fn in_library(conn: &Connection, source: &str) -> rusqlite::Result<InLibrary> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(saved), 0) FROM (
             SELECT EXISTS (SELECT 1 FROM source_saved s
                             WHERE s.source = t.source AND s.path = t.path) AS saved,
                    EXISTS (SELECT 1 FROM source_members m
                             WHERE m.source = t.source AND m.path = t.path
                               AND m.collection <> '') AS kept
               FROM tracks t WHERE t.source = ?1)
          WHERE saved OR kept",
        [source],
        |r| {
            Ok(InLibrary {
                tracks: r.get::<_, i64>(0)? as usize,
                saved: r.get::<_, i64>(1)? as usize,
            })
        },
    )
}

/// Prune picked-only rows neither picked nor played since `cutoff` (unix
/// seconds), leaving the ids in `keep` alone: the saved queue restores by
/// row id. A bookmarked row stays, since a mark is the user keeping it.
/// Playlist entries and listens detach to their snapshots.
/// Answers how many rows went.
pub fn expire_picks(
    conn: &mut Connection,
    cutoff: i64,
    keep: &std::collections::HashSet<i64>,
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let stale: Vec<(i64, String, String)> = {
        let sql = format!(
            "SELECT t.id, t.source, t.path FROM source_members p
               JOIN tracks t ON t.source = p.source AND t.path = p.path
              WHERE {PICKED_ONLY}
                AND t.mtime < ?1
                AND NOT EXISTS (SELECT 1 FROM listens l
                                 WHERE l.track_id = t.id AND l.played_at >= ?1)
                AND NOT EXISTS (SELECT 1 FROM bookmarks b WHERE b.track_id = t.id)"
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut expired = 0;
    {
        let mut members =
            tx.prepare_cached("DELETE FROM source_members WHERE source = ?1 AND path = ?2")?;
        let mut tracks = tx.prepare_cached("DELETE FROM tracks WHERE id = ?1")?;
        for (id, source, path) in stale.iter().filter(|(id, ..)| !keep.contains(id)) {
            members.execute(params![source, path])?;
            expired += tracks.execute([id])?;
        }
    }

    if expired > 0 {
        let sources: HashSet<&String> = stale.iter().map(|(_, source, _)| source).collect();
        for source in sources {
            drop_stale_go_to(&tx, source)?;
        }

        playlists::reattach(&tx)?;
        listens::reattach(&tx)?;
        bookmarks::reattach(&tx)?;
    }
    tx.commit()?;

    Ok(expired)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Make `collection` hold exactly `tracks`, in order, then prune the rows
/// nothing holds any more. Answers how many went.
pub fn set_collection(
    conn: &mut Connection,
    source: &str,
    collection: &str,
    tracks: &[PluginTrack],
) -> rusqlite::Result<usize> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    upsert(&tx, source, tracks)?;
    tx.execute(
        "DELETE FROM source_members WHERE source = ?1 AND collection = ?2",
        params![source, collection],
    )?;
    hold(&tx, source, collection, tracks)?;
    tx.execute(
        "DELETE FROM source_resync WHERE source = ?1 AND collection = ?2",
        params![source, collection],
    )?;

    let pruned = prune_orphans(&tx, source)?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    Ok(pruned)
}

/// Let go of a collection, pruning the rows only it held. Answers how many
/// went.
pub fn drop_collection(
    conn: &mut Connection,
    source: &str,
    collection: &str,
) -> rusqlite::Result<usize> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM source_members WHERE source = ?1 AND collection = ?2",
        params![source, collection],
    )?;
    tx.execute(
        "DELETE FROM source_resync WHERE source = ?1 AND collection = ?2",
        params![source, collection],
    )?;

    let pruned = prune_orphans(&tx, source)?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    Ok(pruned)
}

/// The user's explicit remove: the row goes whatever still holds it.
pub fn remove_track(conn: &mut Connection, source: &str, path: &str) -> rusqlite::Result<()> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM source_members WHERE source = ?1 AND path = ?2",
        params![source, path],
    )?;
    tx.execute(
        "DELETE FROM source_saved WHERE source = ?1 AND path = ?2",
        params![source, path],
    )?;
    tx.execute(
        "DELETE FROM tracks WHERE source = ?1 AND path = ?2",
        params![source, path],
    )?;
    drop_stale_go_to(&tx, source)?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()
}

/// Removing the plugin: every row and membership of the source goes. Answers
/// how many rows went.
pub fn remove_source(conn: &mut Connection, source: &str) -> rusqlite::Result<usize> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM source_members WHERE source = ?1", [source])?;
    tx.execute("DELETE FROM source_saved WHERE source = ?1", [source])?;
    let removed = tx.execute("DELETE FROM tracks WHERE source = ?1", [source])?;
    tx.execute("DELETE FROM source_go_to WHERE source = ?1", [source])?;
    tx.execute("DELETE FROM source_resync WHERE source = ?1", [source])?;
    playlists::reattach(&tx)?;
    listens::reattach(&tx)?;
    bookmarks::reattach(&tx)?;
    tx.commit()?;

    Ok(removed)
}

/// A row read back as the plugin gave it, every tag a pick writes, so
/// picking it again rewrites nothing. None when the source holds no such
/// row.
pub fn track(conn: &Connection, source: &str, key: &str) -> rusqlite::Result<Option<PluginTrack>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.title, t.artist, t.album_artist, t.album, t.genre, t.year, t.disc_no,
                t.track_no, t.duration_ms, t.codec, t.bitrate, t.remote_live,
                COALESCE(g.go_to, '')
           FROM tracks t
           LEFT JOIN source_go_to g ON g.source = t.source AND g.path = t.path
          WHERE t.source = ?1 AND t.path = ?2 AND t.sub = 0",
    )?;
    let mut rows = stmt.query(params![source, key])?;

    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    Ok(Some(PluginTrack {
        key: key.to_string(),
        title: row.get(0)?,
        artist: row.get(1)?,
        album_artist: row.get(2)?,
        album: row.get(3)?,
        genre: row.get(4)?,
        year: row.get(5)?,
        disc_no: row.get(6)?,
        track_no: row.get(7)?,
        duration_ms: row.get(8)?,
        codec: row.get(9)?,
        bitrate_kbps: row.get(10)?,
        live: row.get::<_, i64>(11)? != 0,
        go_to: row.get(12)?,
    }))
}

/// A collection's tracks in the order it was last synced or picked.
pub fn list(conn: &Connection, source: &str, collection: &str) -> rusqlite::Result<Vec<TrackKey>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM source_members
          WHERE source = ?1 AND collection = ?2
          ORDER BY position",
    )?;

    let id = cue::source_id(source);
    let rows = stmt.query_map(params![source, collection], |r| {
        Ok(TrackKey {
            source: id.clone(),
            path: PathBuf::from(r.get::<_, String>(0)?),
            sub: 0,
        })
    })?;

    rows.collect()
}

/// The source's tracks the user added one at a time, the newest first.
pub fn saved(conn: &Connection, source: &str) -> rusqlite::Result<Vec<TrackKey>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM source_saved
          WHERE source = ?1
          ORDER BY saved_at DESC, path",
    )?;

    let id = cue::source_id(source);
    let rows = stmt.query_map([source], |r| {
        Ok(TrackKey {
            source: id.clone(),
            path: PathBuf::from(r.get::<_, String>(0)?),
            sub: 0,
        })
    })?;

    rows.collect()
}

/// Every collection of the source with how many tracks it holds, [`PICKED`]
/// included when anything was picked.
pub fn collections(conn: &Connection, source: &str) -> rusqlite::Result<Vec<(String, usize)>> {
    let mut stmt = conn.prepare(
        "SELECT collection, COUNT(*) FROM source_members
          WHERE source = ?1 GROUP BY collection ORDER BY collection",
    )?;

    let rows = stmt.query_map([source], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
    })?;

    rows.collect()
}

/// A membership write aimed at another source shape would prune rows its own
/// sync or directory owns, so it never gets as far as a statement.
fn refuse_other_shapes(source: &str) -> rusqlite::Result<()> {
    let other = source == cue::LOCAL
        || source == stations::SOURCE
        || source == crate::unknown::SOURCE
        || source.starts_with(cue::SUBSONIC_PREFIX);

    if other {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
            Some(format!("{source} doesn't hold its rows by membership")),
        ));
    }

    Ok(())
}

/// Runs inside the caller's transaction, so the rows land or fail together
/// with the membership that holds them. A row committed without its
/// membership would sit unheld until some later prune.
fn upsert(conn: &Connection, source: &str, tracks: &[PluginTrack]) -> rusqlite::Result<()> {
    let now = unix_now();
    let rows: Vec<TrackRow> = tracks.iter().map(|track| row_for(track, now)).collect();
    store::upsert_source_rows_in(conn, source, &rows)?;

    // A listing without Go to, like a plugin's older answer, leaves the
    // stored one alone.
    let mut go_to = conn.prepare_cached(
        "INSERT INTO source_go_to (source, path, go_to) VALUES (?1, ?2, ?3)
         ON CONFLICT (source, path) DO UPDATE SET go_to = excluded.go_to",
    )?;
    for track in tracks.iter().filter(|track| !track.go_to.is_empty()) {
        go_to.execute(params![source, track.key, track.go_to])?;
    }

    Ok(())
}

/// Go to for rows already in the library, from a listing that showed them.
/// Writes only what moved, and never makes a row.
pub fn keep_go_to(
    conn: &mut Connection,
    source: &str,
    found: &[(String, String)],
) -> rusqlite::Result<usize> {
    refuse_other_shapes(source)?;

    let tx = conn.transaction()?;
    let mut wrote = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO source_go_to (source, path, go_to)
             SELECT ?1, ?2, ?3 WHERE EXISTS
                 (SELECT 1 FROM tracks WHERE source = ?1 AND path = ?2)
             ON CONFLICT (source, path) DO UPDATE SET go_to = excluded.go_to
             WHERE source_go_to.go_to <> excluded.go_to",
        )?;
        for (key, go_to) in found.iter().filter(|(_, go_to)| !go_to.is_empty()) {
            wrote += stmt.execute(params![source, key, go_to])?;
        }
    }
    tx.commit()?;

    Ok(wrote)
}

/// Go to for rows that are gone. Every path that deletes plugin rows calls it.
fn drop_stale_go_to(conn: &Connection, source: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM source_go_to WHERE source = ?1
           AND NOT EXISTS (SELECT 1 FROM tracks t
                            WHERE t.source = ?1 AND t.path = source_go_to.path)",
        [source],
    )?;

    Ok(())
}

/// The stored Go to of each key that has one.
pub fn go_to(
    conn: &Connection,
    source: &str,
    keys: &[String],
) -> rusqlite::Result<HashMap<String, String>> {
    let mut stmt =
        conn.prepare_cached("SELECT go_to FROM source_go_to WHERE source = ?1 AND path = ?2")?;

    let mut found = HashMap::new();
    for key in keys {
        let stored: Option<String> = stmt
            .query_row(params![source, key], |r| r.get(0))
            .optional()?;
        if let Some(stored) = stored {
            found.insert(key.clone(), stored);
        }
    }

    Ok(found)
}

/// Every key the source has a row under, kept, added or picked.
pub fn keys(conn: &Connection, source: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT path FROM tracks WHERE source = ?1 AND sub = 0 ORDER BY id")?;
    let keys = stmt.query_map([source], |r| r.get(0))?;

    keys.collect()
}

/// Whether the collection's next sync has to ignore its token: it was kept
/// before Go to was stored.
pub fn needs_resync(conn: &Connection, source: &str, collection: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM source_resync WHERE source = ?1 AND collection = ?2)",
        params![source, collection],
        |r| r.get(0),
    )
}

/// Append `tracks` to the collection after its last member. A key already
/// held keeps its place, so a repeat pick or a key listed twice doesn't
/// duplicate.
fn hold(
    conn: &Connection,
    source: &str,
    collection: &str,
    tracks: &[PluginTrack],
) -> rusqlite::Result<()> {
    let mut next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM source_members
          WHERE source = ?1 AND collection = ?2",
        params![source, collection],
        |r| r.get(0),
    )?;

    let mut insert = conn.prepare_cached(
        "INSERT OR IGNORE INTO source_members (source, collection, path, position)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for track in tracks {
        // Advance only when a member landed, so a repeat leaves no gap.
        if insert.execute(params![source, collection, track.key, next])? > 0 {
            next += 1;
        }
    }

    Ok(())
}

/// One statement rather than `store::prune_source`'s read-then-delete: that
/// one reads paths into memory to stay under the bound-parameter ceiling,
/// and a subquery binds nothing but the source.
/// A saved row counts as held.
fn prune_orphans(conn: &Connection, source: &str) -> rusqlite::Result<usize> {
    let pruned = conn.execute(
        "DELETE FROM tracks WHERE source = ?1
           AND NOT EXISTS (SELECT 1 FROM source_members m
                            WHERE m.source = ?1 AND m.path = tracks.path)
           AND NOT EXISTS (SELECT 1 FROM source_saved s
                            WHERE s.source = ?1 AND s.path = tracks.path)",
        [source],
    )?;
    drop_stale_go_to(conn, source)?;

    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = "plugin:demo";

    fn track(key: &str, title: &str) -> PluginTrack {
        PluginTrack {
            key: key.into(),
            title: title.into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            ..Default::default()
        }
    }

    fn a_and_b() -> [PluginTrack; 2] {
        [track("a", "A"), track("b", "B")]
    }

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        conn
    }

    fn rows(conn: &Connection, source: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT path FROM tracks WHERE source = ?1 ORDER BY path")
            .unwrap();
        stmt.query_map([source], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn with_go_to(key: &str, go_to: &str) -> PluginTrack {
        PluginTrack {
            go_to: go_to.into(),
            ..track(key, key)
        }
    }

    fn stored_go_to(conn: &Connection, key: &str) -> Option<String> {
        go_to(conn, DEMO, &[key.to_string()]).unwrap().remove(key)
    }

    #[test]
    fn a_kept_rows_go_to_is_stored_and_read_back() {
        let mut conn = store();
        set_collection(
            &mut conn,
            DEMO,
            "liked",
            &[with_go_to("a", "{\"album\":null}")],
        )
        .unwrap();

        assert_eq!(
            stored_go_to(&conn, "a").as_deref(),
            Some("{\"album\":null}")
        );
        assert_eq!(
            crate::members::track(&conn, DEMO, "a")
                .unwrap()
                .unwrap()
                .go_to,
            "{\"album\":null}",
            "a row reads back as the plugin gave it"
        );
    }

    #[test]
    fn a_listing_without_go_to_leaves_the_stored_one() {
        let mut conn = store();
        set_collection(&mut conn, DEMO, "liked", &[with_go_to("a", "first")]).unwrap();
        pick(&mut conn, DEMO, &[track("a", "a")]).unwrap();
        assert_eq!(stored_go_to(&conn, "a").as_deref(), Some("first"));

        set_collection(&mut conn, DEMO, "liked", &[with_go_to("a", "second")]).unwrap();
        assert_eq!(stored_go_to(&conn, "a").as_deref(), Some("second"));
    }

    #[test]
    fn keys_are_every_row_the_source_has() {
        let mut conn = store();
        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        pick(&mut conn, DEMO, &[track("c", "C")]).unwrap();
        pick(&mut conn, "plugin:other", &[track("z", "Z")]).unwrap();

        let mut found = keys(&conn, DEMO).unwrap();
        found.sort();
        assert_eq!(found, ["a", "b", "c"]);
    }

    #[test]
    fn a_listing_fills_in_go_to_for_rows_already_kept() {
        let mut conn = store();
        pick(&mut conn, DEMO, &[track("a", "A")]).unwrap();

        let found = [
            ("a".to_string(), "x".to_string()),
            ("ghost".to_string(), "y".to_string()),
        ];
        assert_eq!(keep_go_to(&mut conn, DEMO, &found).unwrap(), 1);
        assert_eq!(stored_go_to(&conn, "a").as_deref(), Some("x"));
        assert_eq!(
            stored_go_to(&conn, "ghost"),
            None,
            "a listing never makes a row"
        );

        assert_eq!(
            keep_go_to(&mut conn, DEMO, &found).unwrap(),
            0,
            "nothing moved"
        );
    }

    #[test]
    fn go_to_leaves_with_its_row() {
        let mut conn = store();
        set_collection(
            &mut conn,
            DEMO,
            "liked",
            &[with_go_to("a", "x"), with_go_to("b", "y")],
        )
        .unwrap();

        drop_collection(&mut conn, DEMO, "liked").unwrap();
        assert_eq!(stored_go_to(&conn, "a"), None);
        assert_eq!(stored_go_to(&conn, "b"), None);
    }

    fn album_artists(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare("SELECT source, album_artist FROM tracks ORDER BY source, path")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_track_without_an_album_artist_takes_its_artist() {
        let mut conn = store();
        let bare = PluginTrack {
            album_artist: String::new(),
            ..track("a", "A")
        };
        let credited = PluginTrack {
            artist: "Guest".into(),
            ..track("b", "B")
        };
        pick(&mut conn, DEMO, &[bare, credited]).unwrap();

        assert_eq!(
            album_artists(&conn),
            [
                (DEMO.into(), "Artist".into()),
                (DEMO.into(), "Artist".into())
            ]
        );
    }

    #[test]
    fn plugin_rows_stored_without_an_album_artist_are_backfilled() {
        let conn = Connection::open_in_memory().unwrap();
        store::run_ladder_before(&conn, "plugin-album-artist").unwrap();
        for (source, path, album_artist) in [
            (DEMO, "a", ""),
            (DEMO, "b", "Various Artists"),
            ("local", "/m/c.flac", ""),
        ] {
            conn.execute(
                "INSERT INTO tracks (source, path, title, artist, album_artist, album, genre,
                                     year, track_no, duration_ms, size, mtime)
                 VALUES (?1, ?2, 'T', 'Artist', ?3, 'Album', '', 0, 0, 0, 0, 0)",
                params![source, path, album_artist],
            )
            .unwrap();
        }

        store::init_schema(&conn).unwrap();
        assert_eq!(
            album_artists(&conn),
            [
                ("local".into(), "".into()),
                (DEMO.into(), "Artist".into()),
                (DEMO.into(), "Various Artists".into()),
            ],
            "only an empty plugin album artist is filled"
        );
    }

    #[test]
    fn collections_kept_before_go_to_sync_in_full_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::run_ladder_before(&conn, "plugin-go-to").unwrap();
        conn.execute(
            "INSERT INTO source_members (source, collection, path, position)
             VALUES (?1, 'liked', 'a', 0), (?1, ?2, 'b', 0)",
            params![DEMO, PICKED],
        )
        .unwrap();

        store::init_schema(&conn).unwrap();
        assert!(needs_resync(&conn, DEMO, "liked").unwrap());
        assert!(
            !needs_resync(&conn, DEMO, PICKED).unwrap(),
            "picks aren't a collection anyone syncs"
        );

        set_collection(&mut conn, DEMO, "liked", &[track("a", "A")]).unwrap();
        assert!(
            !needs_resync(&conn, DEMO, "liked").unwrap(),
            "once is enough"
        );
    }

    fn paths(keys: &[TrackKey]) -> Vec<String> {
        keys.iter()
            .map(|key| key.path.to_string_lossy().into_owned())
            .collect()
    }

    fn id_of(conn: &Connection, path: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM tracks WHERE source = ?1 AND path = ?2",
            params![DEMO, path],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn age(conn: &Connection, path: &str, mtime: i64) {
        conn.execute(
            "UPDATE tracks SET mtime = ?1 WHERE source = ?2 AND path = ?3",
            params![mtime, DEMO, path],
        )
        .unwrap();
    }

    fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_row_reads_back_as_the_track_that_wrote_it() {
        let mut conn = store();
        let written = PluginTrack {
            disc_no: 2,
            track_no: 7,
            year: 2019,
            duration_ms: 213_000,
            codec: "FLAC".into(),
            bitrate_kbps: 1411,
            ..track("a", "A")
        };
        pick(&mut conn, DEMO, std::slice::from_ref(&written)).unwrap();

        assert_eq!(track_of(&conn, "a"), Some(written));
        assert_eq!(track_of(&conn, "missing"), None);
    }

    fn track_of(conn: &Connection, key: &str) -> Option<PluginTrack> {
        super::track(conn, DEMO, key).unwrap()
    }

    #[test]
    fn a_pick_stays_out_of_the_library_until_saved_or_synced() {
        let mut conn = store();
        pick(
            &mut conn,
            DEMO,
            &[track("a", "A"), track("b", "B"), track("c", "C")],
        )
        .unwrap();
        let (a, b, c) = (id_of(&conn, "a"), id_of(&conn, "b"), id_of(&conn, "c"));
        assert_eq!(
            sorted(picked_only_ids(&conn).unwrap()),
            sorted(vec![a, b, c])
        );

        save(&mut conn, DEMO, &[track("a", "A")]).unwrap();
        set_collection(&mut conn, DEMO, "liked", &[track("b", "B")]).unwrap();

        assert_eq!(picked_only_ids(&conn).unwrap(), vec![c]);
        assert_eq!(saved_ids(&conn).unwrap(), vec![a]);
    }

    #[test]
    fn saved_lists_only_this_sources_added_tracks_newest_first() {
        let mut conn = store();
        save(&mut conn, DEMO, &[track("a", "A")]).unwrap();
        conn.execute("UPDATE source_saved SET saved_at = 1", [])
            .unwrap();
        save(&mut conn, DEMO, &[track("b", "B")]).unwrap();
        set_collection(&mut conn, DEMO, "liked", &[track("c", "C")]).unwrap();
        save(&mut conn, "plugin:other", &[track("z", "Z")]).unwrap();

        assert_eq!(paths(&saved(&conn, DEMO).unwrap()), ["b", "a"]);
    }

    #[test]
    fn saving_needs_no_pick_and_unsaving_prunes_what_nothing_holds() {
        let mut conn = store();
        save(&mut conn, DEMO, &a_and_b()).unwrap();
        pick(&mut conn, DEMO, &[track("b", "B")]).unwrap();
        assert!(
            picked_only_ids(&conn).unwrap().is_empty(),
            "saved rows show"
        );

        let pruned = unsave(&mut conn, DEMO, &["a".into(), "b".into()]).unwrap();
        assert_eq!(pruned, 1, "only a went; b is still a pick");
        assert_eq!(rows(&conn, DEMO), ["b"]);
        assert_eq!(picked_only_ids(&conn).unwrap(), vec![id_of(&conn, "b")]);
    }

    #[test]
    fn dropping_a_collection_keeps_a_saved_row() {
        let mut conn = store();
        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        save(&mut conn, DEMO, &[track("a", "A")]).unwrap();

        drop_collection(&mut conn, DEMO, "liked").unwrap();
        assert_eq!(rows(&conn, DEMO), ["a"]);
    }

    #[test]
    fn a_stale_pick_expires_unless_something_still_wants_it() {
        let mut conn = store();
        let tracks: Vec<PluginTrack> = [
            "old", "played", "queued", "fresh", "saved", "synced", "marked",
        ]
        .iter()
        .map(|key| track(key, key))
        .collect();
        pick(&mut conn, DEMO, &tracks).unwrap();
        save(&mut conn, DEMO, &[track("saved", "saved")]).unwrap();
        set_collection(&mut conn, DEMO, "liked", &[track("synced", "synced")]).unwrap();

        let cutoff = 1_000_000;
        for key in ["old", "played", "queued", "saved", "synced", "marked"] {
            age(&conn, key, cutoff - 10);
        }
        age(&conn, "fresh", cutoff + 10);
        conn.execute(
            "INSERT INTO listens (track_id, played_at, title, artist, album, genre)
             VALUES (?1, ?2, 'played', '', '', '')",
            params![id_of(&conn, "played"), cutoff + 5],
        )
        .unwrap();
        let marked = id_of(&conn, "marked");
        bookmarks::add(&conn, marked, "plugin:demo|marked", 5_000, "", None).unwrap();
        let keep = std::collections::HashSet::from([id_of(&conn, "queued")]);

        assert_eq!(expire_picks(&mut conn, cutoff, &keep).unwrap(), 1);
        assert_eq!(
            rows(&conn, DEMO),
            ["fresh", "marked", "played", "queued", "saved", "synced"]
        );
        assert!(
            collections(&conn, DEMO)
                .unwrap()
                .iter()
                .all(|(_, n)| *n > 0),
            "the expired row's membership went with it"
        );
    }

    #[test]
    fn a_picked_again_track_gets_its_marks_back() {
        let mut conn = store();
        // A second row so the returning one can't reuse the freed id.
        pick(&mut conn, DEMO, &[track("a", "A"), track("z", "Z")]).unwrap();
        let first = id_of(&conn, "a");
        bookmarks::add(&conn, first, "plugin:demo|a", 5_000, "", None).unwrap();

        remove_track(&mut conn, DEMO, "a").unwrap();
        assert!(bookmarks::all(&conn).unwrap().is_empty());

        pick(&mut conn, DEMO, &[track("a", "A")]).unwrap();
        assert_ne!(id_of(&conn, "a"), first);
        let marks = bookmarks::all(&conn).unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].track_id, id_of(&conn, "a"));
    }

    #[test]
    fn row_for_keys_the_row_and_stores_no_url() {
        let row = row_for(
            &PluginTrack {
                live: true,
                ..track("k1", "One")
            },
            42,
        );

        assert_eq!(row.path, "k1");
        assert_eq!(row.sub, 0);
        assert_eq!(row.remote_url, "");
        assert!(row.remote_live);
        assert_eq!(row.mtime, 42);
    }

    #[test]
    fn pick_holds_rows_once_however_often_they_arrive() {
        let mut conn = store();

        let keys = pick(&mut conn, DEMO, &[track("a", "A"), track("b", "B")]).unwrap();
        assert_eq!(paths(&keys), ["a", "b"]);
        assert!(keys.iter().all(|key| &*key.source == DEMO && key.sub == 0));

        pick(&mut conn, DEMO, &[track("a", "A again")]).unwrap();
        assert_eq!(
            rows(&conn, DEMO),
            ["a", "b"],
            "the upsert refreshed a in place"
        );
        assert_eq!(paths(&list(&conn, DEMO, PICKED).unwrap()), ["a", "b"]);
        assert_eq!(collections(&conn, DEMO).unwrap(), [(PICKED.to_string(), 2)]);

        let title: String = conn
            .query_row(
                "SELECT title FROM tracks WHERE source = ?1 AND path = 'a'",
                [DEMO],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "A again");
    }

    #[test]
    fn a_resync_prunes_what_nothing_else_holds() {
        let mut conn = store();

        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        let pruned = set_collection(&mut conn, DEMO, "liked", &[track("b", "B")]).unwrap();

        assert_eq!(pruned, 1);
        assert_eq!(rows(&conn, DEMO), ["b"]);
        assert_eq!(paths(&list(&conn, DEMO, "liked").unwrap()), ["b"]);
    }

    #[test]
    fn a_picked_row_survives_leaving_a_collection() {
        let mut conn = store();

        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        pick(&mut conn, DEMO, &[track("a", "A")]).unwrap();
        let pruned = set_collection(&mut conn, DEMO, "liked", &[track("b", "B")]).unwrap();

        assert_eq!(pruned, 0);
        assert_eq!(rows(&conn, DEMO), ["a", "b"]);
    }

    #[test]
    fn syncing_one_collection_never_prunes_another() {
        let mut conn = store();

        set_collection(&mut conn, DEMO, "playlist-1", &[track("p", "P")]).unwrap();
        set_collection(&mut conn, DEMO, "liked", &[track("a", "A")]).unwrap();
        let pruned = set_collection(&mut conn, DEMO, "liked", &[]).unwrap();

        assert_eq!(pruned, 1, "only a went");
        assert_eq!(rows(&conn, DEMO), ["p"]);
    }

    #[test]
    fn a_collection_keeps_the_order_it_was_synced_in() {
        let mut conn = store();

        set_collection(
            &mut conn,
            DEMO,
            "liked",
            &[
                track("c", "C"),
                track("a", "A"),
                track("c", "C"),
                track("b", "B"),
            ],
        )
        .unwrap();

        assert_eq!(
            paths(&list(&conn, DEMO, "liked").unwrap()),
            ["c", "a", "b"],
            "a key listed twice holds its first place"
        );
    }

    #[test]
    fn a_sync_that_fails_after_the_upsert_leaves_nothing_behind() {
        let mut conn = store();
        set_collection(&mut conn, DEMO, "liked", &[track("a", "A")]).unwrap();

        // Any membership insert fails, which lands between the upsert and
        // the prune.
        conn.execute_batch(
            "CREATE TRIGGER refuse_members BEFORE INSERT ON source_members
             BEGIN SELECT RAISE(ABORT, 'forced'); END;",
        )
        .unwrap();

        assert!(set_collection(&mut conn, DEMO, "liked", &[track("b", "B")]).is_err());
        assert!(pick(&mut conn, DEMO, &[track("c", "C")]).is_err());

        assert_eq!(rows(&conn, DEMO), ["a"], "neither write landed a row");
        assert_eq!(
            paths(&list(&conn, DEMO, "liked").unwrap()),
            ["a"],
            "and the old membership stands"
        );
    }

    #[test]
    fn drop_collection_prunes_exactly_the_rows_only_it_held() {
        let mut conn = store();

        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        set_collection(&mut conn, DEMO, "playlist-1", &[track("b", "B")]).unwrap();
        pick(&mut conn, DEMO, &[track("c", "C")]).unwrap();

        assert_eq!(drop_collection(&mut conn, DEMO, "liked").unwrap(), 1);
        assert_eq!(rows(&conn, DEMO), ["b", "c"]);
        assert!(list(&conn, DEMO, "liked").unwrap().is_empty());
        assert_eq!(
            collections(&conn, DEMO).unwrap(),
            [(PICKED.to_string(), 1), ("playlist-1".to_string(), 1)]
        );
    }

    #[test]
    fn remove_track_goes_whatever_holds_it() {
        let mut conn = store();

        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        pick(&mut conn, DEMO, &[track("a", "A")]).unwrap();
        remove_track(&mut conn, DEMO, "a").unwrap();

        assert_eq!(rows(&conn, DEMO), ["b"]);
        assert!(list(&conn, DEMO, PICKED).unwrap().is_empty());
        assert_eq!(paths(&list(&conn, DEMO, "liked").unwrap()), ["b"]);
    }

    #[test]
    fn remove_source_touches_no_other_source() {
        let mut conn = store();
        let other = "plugin:other";

        set_collection(&mut conn, DEMO, "liked", &a_and_b()).unwrap();
        pick(&mut conn, other, &[track("a", "A")]).unwrap();
        stations::put(
            &mut conn,
            &[stations::Station {
                url: "a".into(),
                name: "Station".into(),
                genre: String::new(),
            }],
        )
        .unwrap();

        assert_eq!(remove_source(&mut conn, DEMO).unwrap(), 2);
        assert!(rows(&conn, DEMO).is_empty());
        assert!(collections(&conn, DEMO).unwrap().is_empty());
        assert_eq!(rows(&conn, other), ["a"]);
        assert_eq!(paths(&list(&conn, other, PICKED).unwrap()), ["a"]);
        assert_eq!(rows(&conn, stations::SOURCE), ["a"]);
    }

    #[test]
    fn every_write_refuses_another_source_shape() {
        let mut conn = store();
        stations::put(
            &mut conn,
            &[stations::Station {
                url: "a".into(),
                name: "Station".into(),
                genre: String::new(),
            }],
        )
        .unwrap();

        for source in [cue::LOCAL, stations::SOURCE, "subsonic:abc123"] {
            let a = [track("a", "A")];

            assert!(pick(&mut conn, source, &a).is_err(), "{source}");
            assert!(save(&mut conn, source, &a).is_err(), "{source}");
            assert!(
                unsave(&mut conn, source, &["a".into()]).is_err(),
                "{source}"
            );
            assert!(
                set_collection(&mut conn, source, "liked", &a).is_err(),
                "{source}"
            );
            assert!(
                set_collection(&mut conn, source, "liked", &[]).is_err(),
                "{source}"
            );
            assert!(
                drop_collection(&mut conn, source, "liked").is_err(),
                "{source}"
            );
            assert!(remove_track(&mut conn, source, "a").is_err(), "{source}");
            assert!(remove_source(&mut conn, source).is_err(), "{source}");
        }

        assert_eq!(rows(&conn, stations::SOURCE), ["a"], "the station stands");
        assert!(rows(&conn, cue::LOCAL).is_empty());
    }

    #[test]
    fn holds_name_the_saves_and_kept_collections_once_and_skip_the_rest() {
        let mut conn = store();
        pick(&mut conn, DEMO, &[track("picked", "Picked")]).unwrap();
        save(&mut conn, DEMO, &[track("a", "A"), track("both", "Both")]).unwrap();
        set_collection(
            &mut conn,
            DEMO,
            "liked",
            &[track("both", "Both"), track("b", "B")],
        )
        .unwrap();
        set_collection(&mut conn, DEMO, "mix", &[track("b", "B")]).unwrap();
        stations::put(
            &mut conn,
            &[stations::Station {
                url: "s".into(),
                name: "Station".into(),
                genre: String::new(),
            }],
        )
        .unwrap();
        let station: i64 = conn
            .query_row(
                "SELECT id FROM tracks WHERE source = ?1",
                [stations::SOURCE],
                |r| r.get(0),
            )
            .unwrap();

        let (a, b, both, picked) = (
            id_of(&conn, "a"),
            id_of(&conn, "b"),
            id_of(&conn, "both"),
            id_of(&conn, "picked"),
        );
        let held = holds(&conn, &[a, both, b, a, picked, station, -1]).unwrap();

        let demo = |path: &str| (DEMO.to_string(), path.to_string());
        assert_eq!(held.saved, [demo("a"), demo("both")]);
        assert_eq!(held.kept, [demo("liked"), demo("mix")]);

        assert!(holds(&conn, &[picked, station]).unwrap().is_empty());
    }

    #[test]
    fn in_library_counts_what_browses_and_leaves_picks_out() {
        let mut conn = store();
        pick(
            &mut conn,
            DEMO,
            &[track("picked", "Picked"), track("a", "A")],
        )
        .unwrap();
        save(&mut conn, DEMO, &[track("a", "A"), track("both", "Both")]).unwrap();
        set_collection(
            &mut conn,
            DEMO,
            "liked",
            &[track("both", "Both"), track("b", "B")],
        )
        .unwrap();
        set_collection(&mut conn, DEMO, "mix", &[track("b", "B")]).unwrap();
        save(&mut conn, "plugin:other", &[track("x", "X")]).unwrap();

        assert_eq!(
            in_library(&conn, DEMO).unwrap(),
            InLibrary {
                tracks: 3,
                saved: 2
            }
        );
        assert_eq!(
            in_library(&conn, "plugin:none").unwrap(),
            InLibrary::default()
        );
    }
}
