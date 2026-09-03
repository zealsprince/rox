//! Custom playlists in the library database (ADR 16). A playlist is a named,
//! ordered list of member rows; a member holds the track id, its position,
//! and a snapshot of the identifying tags at add time, the same deletion
//! hedge the listen events use (ADR 11). While a track exists, reads resolve
//! through the live catalog, so a fixed tag shows on the playlist row too;
//! once the track is gone the snapshot keeps the row readable, though there is
//! no file left to play. Track identity is kept across a rescan on the rowid
//! (ADR 5), so a playlist follows its tracks across scans.
//!
//! Members are addressed by their own row id, not the track id: a playlist may
//! hold the same track more than once, so removing or moving a member acts on
//! one occurrence, not every copy of a track.
//!
//! The one thing a rowid doesn't outlast is a prune: a file missing at scan
//! time loses its row, and coming back it gets a fresh id the member knows
//! nothing about. So a member also snapshots the track's path, and
//! [`reattach`] runs after every scan to match dangling members back to the
//! catalog: by that path first, then by the tag snapshot when it names
//! exactly one track. A playlist holds together across its files leaving and
//! returning, even at a new path.

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::projection::{FilterSet, Filterable, Projection, SortKey, TrackFields};

/// The playlists and their member rows beside the tracks they key to. No
/// foreign key here, matching the listens table: deleting a track keeps its
/// playlist rows, which is the snapshot's job. Duplicates are allowed, so
/// there's no uniqueness on (playlist, track).
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS playlists (
            id        INTEGER PRIMARY KEY,
            name      TEXT NOT NULL,
            created   INTEGER NOT NULL,
            updated   INTEGER NOT NULL,
            favourite INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS playlist_tracks (
            id          INTEGER PRIMARY KEY,
            playlist_id INTEGER NOT NULL,
            track_id    INTEGER NOT NULL,
            position    INTEGER NOT NULL,
            title       TEXT NOT NULL,
            artist      TEXT NOT NULL,
            album       TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS playlist_tracks_list
            ON playlist_tracks (playlist_id, position);",
    )?;
    // An earlier cut of this table carried UNIQUE (playlist_id, track_id),
    // which forbade duplicates. SQLite can't drop a constraint in place, so
    // rebuild the table without it when the old shape is found.
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'playlist_tracks'",
            [],
            |row| row.get(0),
        )
        .ok();
    if sql.is_some_and(|sql| sql.contains("UNIQUE")) {
        conn.execute_batch(
            "CREATE TABLE playlist_tracks_new (
                id          INTEGER PRIMARY KEY,
                playlist_id INTEGER NOT NULL,
                track_id    INTEGER NOT NULL,
                position    INTEGER NOT NULL,
                title       TEXT NOT NULL,
                artist      TEXT NOT NULL,
                album       TEXT NOT NULL
            );
            INSERT INTO playlist_tracks_new
                SELECT id, playlist_id, track_id, position, title, artist, album
                FROM playlist_tracks;
            DROP TABLE playlist_tracks;
            ALTER TABLE playlist_tracks_new RENAME TO playlist_tracks;
            CREATE INDEX IF NOT EXISTS playlist_tracks_list
                ON playlist_tracks (playlist_id, position);",
        )?;
    }
    // A playlists table from before the favourites flag: add it. The default
    // 0 leaves every existing playlist a normal one; ensure_favourites makes
    // the marked one on next open.
    let has_favourite = conn
        .prepare("SELECT 1 FROM pragma_table_info('playlists') WHERE name = 'favourite'")?
        .exists([])?;
    if !has_favourite {
        conn.execute_batch(
            "ALTER TABLE playlists ADD COLUMN favourite INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // The path snapshot column arrives via the store ladder's snapshot-paths
    // step, which runs after this baseline.
    Ok(())
}

/// The store ladder's snapshot-paths step, the playlist half: members learn
/// the track's path, the content key [`reattach`] matches on. Live members
/// backfill from the catalog so existing playlists get the durability
/// without a re-add; dangling ones keep the empty default and rely on the
/// tag fallback.
pub(crate) fn add_path_snapshot(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE playlist_tracks ADD COLUMN path TEXT NOT NULL DEFAULT '';
         UPDATE playlist_tracks SET path = COALESCE(
             (SELECT t.path FROM tracks t
              WHERE t.id = playlist_tracks.track_id AND t.source = 'local'), '');",
    )
}

/// The store ladder's smart-playlists step: every playlist row learns
/// which kind it is and, for a smart one, what query stands in for its
/// members. Widening `playlists` rather than opening a side table follows
/// the favourite column above: kind is something every row records, and
/// [`list`] keeps reading both kinds in one pass. The default 0 leaves
/// every existing playlist static with a NULL definition, which is
/// exactly what they are.
pub(crate) fn add_smart_columns(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE playlists ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE playlists ADD COLUMN definition TEXT;",
    )
}

/// Match member rows back to the catalog after a scan. A member keys to its
/// track by rowid, which holds across rescans and renames (ADR 5) but dies
/// with a prune: a drive missing at scan time, an album deleted and restored, a
/// reorganize done with the app closed all bring the file back under a fresh
/// id the member knows nothing about. Dangling members relink by their path
/// snapshot first, then by their tag snapshot when it names exactly one
/// track, so an ambiguous match never guesses. Members with a live track
/// just keep their path snapshot current (a rename moves the path under the
/// same id).
///
/// Returns how many members relinked, or None when nothing was dangling and
/// the matchers never ran at all.
pub fn reattach(conn: &Connection) -> rusqlite::Result<Option<usize>> {
    conn.execute(
        "UPDATE playlist_tracks SET path = t.path FROM tracks t
         WHERE t.id = playlist_tracks.track_id AND t.source = 'local'
           AND playlist_tracks.path <> t.path",
        [],
    )?;
    // Nothing dangling, nothing to match. The two passes below are the
    // expensive half (the tag one joins on the tag triple and counts the
    // matches to refuse an ambiguous one), and a healthy library runs this
    // after every scan and every reindex, so it pays one indexed probe
    // instead.
    if !has_dangling(conn)? {
        return Ok(None);
    }
    let by_path = conn.execute(
        "UPDATE playlist_tracks SET track_id = t.id FROM tracks t
         WHERE playlist_tracks.path <> ''
           AND t.source = 'local' AND t.path = playlist_tracks.path
           AND NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = playlist_tracks.track_id)",
        [],
    )?;
    let by_tags = conn.execute(
        "UPDATE playlist_tracks SET track_id = t.id, path = t.path FROM tracks t
         WHERE NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = playlist_tracks.track_id)
           AND NOT (playlist_tracks.title = '' AND playlist_tracks.artist = ''
                    AND playlist_tracks.album = '')
           AND t.source = 'local'
           AND t.title = playlist_tracks.title AND t.artist = playlist_tracks.artist
           AND t.album = playlist_tracks.album
           AND (SELECT COUNT(*) FROM tracks c WHERE c.source = 'local'
                AND c.title = playlist_tracks.title AND c.artist = playlist_tracks.artist
                AND c.album = playlist_tracks.album) = 1",
        [],
    )?;
    Ok(Some(by_path + by_tags))
}

/// Whether any member points at a track row that no longer exists. One
/// indexed lookup per member and it stops at the first hit, so the answer
/// costs nothing on a library whose playlists all still have their files.
fn has_dangling(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM playlist_tracks
            WHERE NOT EXISTS (SELECT 1 FROM tracks x WHERE x.id = playlist_tracks.track_id))",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|found| found == 1)
}

/// Which kind of list a playlist row is. A static playlist owns member
/// rows; a smart one owns a query and no members at all, and materializes
/// against the projection whenever something asks what's in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaylistKind {
    #[default]
    Static,
    Smart,
}

impl PlaylistKind {
    /// The `kind` column's integer. Anything unknown reads as static: an
    /// older binary pointed at a newer file must still show the row, and a
    /// list with no members it can explain is the safe reading.
    fn from_column(value: i64) -> PlaylistKind {
        match value {
            1 => PlaylistKind::Smart,
            _ => PlaylistKind::Static,
        }
    }

    fn column(self) -> i64 {
        match self {
            PlaylistKind::Static => 0,
            PlaylistKind::Smart => 1,
        }
    }
}

/// What a smart playlist is: the saved query, in the same syntax the
/// search boxes use, plus the structured filter, sort, and cap a view
/// takes. Held as JSON in the playlist row's `definition` column and
/// evaluated live, so a smart playlist never holds member rows and never
/// goes stale against the catalog.
///
/// `sort` is a column and whether it runs descending, the pair
/// [`crate::view::ViewSpec`] takes; None keeps the canonical browse order.
/// `limit` caps the result after the sort, so "my top 50" is a rating sort
/// with a 50.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartDef {
    pub query: String,
    pub filter: FilterSet,
    pub sort: Option<(SortKey, bool)>,
    pub limit: Option<u32>,
}

impl SmartDef {
    /// The track ids this definition names, in the order it asks for. The
    /// whole of what a smart playlist "holds": there are no member rows, so
    /// every read that wants its tracks runs this.
    ///
    /// The kernel is the same [`crate::view::view_for`] the library table
    /// runs, so a saved query means exactly what the same string typed into
    /// a search box means. Nothing is cached: a pass is one sweep of the
    /// projection, and a cache would need invalidating on every rating,
    /// play, and scan.
    pub fn ids(&self, projection: &Projection, order: Arc<Vec<u32>>) -> Vec<i64> {
        self.rows(projection, order)
            .iter()
            .filter_map(|&row| projection.db_id.get(row as usize).copied())
            .collect()
    }

    /// The same pass as [`SmartDef::ids`] stopped one step earlier: the
    /// projection rows, before they turn into db ids. What a caller wants
    /// when it draws the tracks rather than hands them on, since drawing a
    /// row is resolving it, and going through ids would mean mapping every
    /// one back to the row it came from.
    pub fn rows(&self, projection: &Projection, order: Arc<Vec<u32>>) -> Vec<u32> {
        let (rows, _) = crate::view::view_for(
            projection,
            order,
            &crate::view::ViewSpec {
                query: &self.query,
                filter: &self.filter,
                similar: None,
                sort: self.sort,
                grouping: None,
            },
        );
        let mut rows: Vec<u32> = rows
            .iter()
            .filter_map(|row| match row {
                crate::view::Row::Track(row) => Some(*row),
                _ => None,
            })
            .collect();
        // The cap applies after the sort, so "top 50" means the first fifty
        // of the order the definition asked for.
        if let Some(limit) = self.limit {
            rows.truncate(limit as usize);
        }
        rows
    }
}

/// A playlist in the sidebar list: its id, name, and how many tracks it holds.
/// `favourite` marks the one default playlist behind the heart column and the
/// Favourites menu; the panel pins it to the top and shields it from delete
/// and rename.
///
/// `tracks` counts member rows, so a smart playlist always reports 0 here:
/// it has no members to count and the real number costs a projection pass.
/// The panel fills that in from the materialization it already ran.
#[derive(Clone)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub tracks: u64,
    pub favourite: bool,
    pub kind: PlaylistKind,
}

/// One member's line in a playlist view. `member_id` addresses this exact
/// occurrence for remove, move, and reorder; the tags resolve from the live
/// catalog while the track exists, from the snapshot once it is gone.
#[derive(Clone)]
pub struct PlaylistTrack {
    pub member_id: i64,
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Album grouping metadata, read live from the catalog for the panel's
    /// album headings. A deleted track has no live row, so these fall back
    /// to empty or zero; the snapshot only keeps title, artist, and album.
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
    /// The 0-5 star rating, 0 when unrated. Read live from the catalog for
    /// the panel's rating cell, like the album grouping fields.
    pub rating: u8,
    /// The file path, for the cover column's thumbnail: the live catalog's
    /// while the track exists, the snapshot's once it is gone, so a pruned
    /// file whose bytes are still on disk keeps its cover.
    pub path: String,
}

impl Filterable for PlaylistTrack {
    fn fields(&self) -> TrackFields<'_> {
        TrackFields {
            db_id: Some(self.track_id),
            title: &self.title,
            artist: &self.artist,
            album_artist: &self.album_artist,
            album: &self.album,
            genre: &self.genre,
            year: self.year,
            codec: &self.codec,
            path: &self.path,
        }
    }
}

/// Create an empty playlist, returning its id. `now` is unix seconds.
pub fn create(conn: &Connection, name: &str, now: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO playlists (name, created, updated) VALUES (?1, ?2, ?2)",
        rusqlite::params![name, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Create a smart playlist around a definition, returning its id. `now` is
/// unix seconds.
pub fn create_smart(
    conn: &Connection,
    name: &str,
    def: &SmartDef,
    now: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO playlists (name, created, updated, kind, definition)
         VALUES (?1, ?2, ?2, ?3, ?4)",
        rusqlite::params![name, now, PlaylistKind::Smart.column(), encode(def)],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rewrite a smart playlist's definition and stamp it updated. Also flips
/// the row to smart, so the one call covers both a saved edit and the
/// first definition a row is given.
pub fn set_definition(
    conn: &Connection,
    id: i64,
    def: &SmartDef,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE playlists SET definition = ?2, kind = ?3, updated = ?4 WHERE id = ?1",
        rusqlite::params![id, encode(def), PlaylistKind::Smart.column(), now],
    )?;
    Ok(())
}

/// One playlist's definition, None when it is static or its stored JSON
/// no longer parses. An unreadable definition reads as none rather than an
/// error: the row is still a playlist, it just resolves to nothing until
/// the editor writes it again.
pub fn definition(conn: &Connection, id: i64) -> rusqlite::Result<Option<SmartDef>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT definition FROM playlists WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(stored.and_then(|json| serde_json::from_str(&json).ok()))
}

/// A definition as the JSON the `definition` column holds. A `SmartDef` is
/// plain data, so the encode cannot fail; an empty string would read back
/// as no definition, which is the harmless answer if it somehow did.
fn encode(def: &SmartDef) -> String {
    serde_json::to_string(def).unwrap_or_default()
}

/// Rename a playlist and stamp it updated.
pub fn rename(conn: &Connection, id: i64, name: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE playlists SET name = ?2, updated = ?3 WHERE id = ?1",
        rusqlite::params![id, name, now],
    )?;
    Ok(())
}

/// Delete a playlist and all its member rows.
pub fn delete(conn: &mut Connection, id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
    tx.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
    tx.commit()
}

/// Every playlist with its track count. Favourites pins to the top, the rest
/// follow newest updated first.
pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Playlist>> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.id, p.name,
                (SELECT COUNT(*) FROM playlist_tracks m WHERE m.playlist_id = p.id),
                p.favourite, p.kind
         FROM playlists p
         ORDER BY p.favourite DESC, p.updated DESC, p.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            name: row.get(1)?,
            tracks: row.get::<_, i64>(2)? as u64,
            favourite: row.get::<_, i64>(3)? != 0,
            kind: PlaylistKind::from_column(row.get(4)?),
        })
    })?;
    rows.collect()
}

/// The id of the one favourites playlist, creating it if this library has
/// none yet. Called on startup so the default playlist is always present, and
/// again by the favourite toggles so they never race a missing row. Idempotent:
/// a library that already has the favourites playlist just gets its id back.
pub fn ensure_favourites(conn: &Connection, now: i64) -> rusqlite::Result<i64> {
    if let Some(id) = favourites_id(conn)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO playlists (name, created, updated, favourite)
         VALUES ('Favourites', ?1, ?1, 1)",
        [now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The favourites playlist's id, if it exists yet.
pub fn favourites_id(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM playlists WHERE favourite = 1 ORDER BY id LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
}

/// The track ids in the favourites playlist, for the library's heart column.
/// Empty when there is no favourites playlist yet.
pub fn favourite_track_ids(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let Some(fav) = favourites_id(conn)? else {
        return Ok(Vec::new());
    };
    let mut stmt =
        conn.prepare_cached("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")?;
    let rows = stmt.query_map([fav], |row| row.get(0))?;
    rows.collect()
}

/// Drop duplicate members from the favourites playlist, keeping the first row
/// per track. The heart is on or off, so its playlist holds a track once
/// however the track got in. [`set_favourite`] never duplicates, but a menu
/// add or a drag onto the list is a plain playlist write and would, and a
/// second row makes the heart's off switch look broken: one delete, still
/// favourited. Returns how many rows it dropped.
fn dedupe_favourite_members(conn: &Connection, fav: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM playlist_tracks
          WHERE playlist_id = ?1
            AND id NOT IN (SELECT MIN(id) FROM playlist_tracks
                            WHERE playlist_id = ?1 GROUP BY track_id)",
        [fav],
    )
}

/// Clear duplicates a library picked up before the writes started keeping
/// them out. Startup's one sweep, cheap on a list that's already clean;
/// every write since holds the line on its own.
pub fn dedupe_favourites(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let Some(fav) = favourites_id(conn)? else {
        return Ok(0);
    };
    let dropped = dedupe_favourite_members(conn, fav)?;
    if dropped > 0 {
        conn.execute(
            "UPDATE playlists SET updated = ?2 WHERE id = ?1",
            rusqlite::params![fav, now],
        )?;
    }
    Ok(dropped)
}

/// Whether a track is in the favourites playlist.
pub fn is_favourite(conn: &Connection, track_id: i64) -> rusqlite::Result<bool> {
    let Some(fav) = favourites_id(conn)? else {
        return Ok(false);
    };
    Ok(conn
        .query_row(
            "SELECT 1 FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2 LIMIT 1",
            rusqlite::params![fav, track_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Turn a track's favourite on or off. On adds it to the favourites playlist
/// once, off drops every copy; unlike a normal playlist add this never
/// duplicates, so the heart stays a clean on/off. Creates the favourites
/// playlist if it is somehow missing. A no-op when the track is already in the
/// wanted state.
pub fn set_favourite(
    conn: &mut Connection,
    track_id: i64,
    on: bool,
    now: i64,
) -> rusqlite::Result<()> {
    let fav = ensure_favourites(conn, now)?;
    let member = conn
        .query_row(
            "SELECT 1 FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2 LIMIT 1",
            rusqlite::params![fav, track_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    match (on, member) {
        (true, false) => add(conn, fav, &[track_id], now),
        (false, true) => {
            conn.execute(
                "DELETE FROM playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
                rusqlite::params![fav, track_id],
            )?;
            conn.execute(
                "UPDATE playlists SET updated = ?2 WHERE id = ?1",
                rusqlite::params![fav, now],
            )?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Append tracks to a playlist in the given order, snapshotting each track's
/// tags from the live catalog. Duplicates are kept: a track already in the
/// playlist gets a second member row. The favourites playlist is the one
/// exception, per [`dedupe_favourite_members`]. Stamps the playlist updated.
pub fn add(
    conn: &mut Connection,
    playlist_id: i64,
    track_ids: &[i64],
    now: i64,
) -> rusqlite::Result<()> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let mut next: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_tracks WHERE playlist_id = ?1",
            [playlist_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO playlist_tracks
                (playlist_id, track_id, position, title, artist, album, path)
             SELECT ?1, t.id, ?3, t.title, t.artist, t.album, t.path
             FROM tracks t WHERE t.id = ?2",
        )?;
        for &track_id in track_ids {
            let added = insert.execute(rusqlite::params![playlist_id, track_id, next])?;
            // Only advance the position when a row actually landed, so a track
            // id with no catalog row (nothing to snapshot) leaves no gap.
            if added > 0 {
                next += 1;
            }
        }
    }
    // A track added to favourites it was already in keeps the row it had, so
    // the heart reads the same before and after and one click still clears it.
    if favourites_id(&tx)? == Some(playlist_id) {
        dedupe_favourite_members(&tx, playlist_id)?;
    }
    tx.execute(
        "UPDATE playlists SET updated = ?2 WHERE id = ?1",
        rusqlite::params![playlist_id, now],
    )?;
    tx.commit()
}

/// Remove one member from a playlist by its row id. Leaves the remaining
/// positions as they are; they stay ordered, just with a gap the next
/// reorder closes.
pub fn remove_member(conn: &Connection, member_id: i64, now: i64) -> rusqlite::Result<()> {
    let playlist_id: Option<i64> = conn
        .query_row(
            "SELECT playlist_id FROM playlist_tracks WHERE id = ?1",
            [member_id],
            |row| row.get(0),
        )
        .ok();
    conn.execute("DELETE FROM playlist_tracks WHERE id = ?1", [member_id])?;
    if let Some(playlist_id) = playlist_id {
        conn.execute(
            "UPDATE playlists SET updated = ?2 WHERE id = ?1",
            rusqlite::params![playlist_id, now],
        )?;
    }
    Ok(())
}

/// Move a member to the end of another playlist, keeping its snapshot. Both
/// playlists stamp updated. A no-op when the member is already there.
pub fn move_member(
    conn: &mut Connection,
    member_id: i64,
    to_playlist: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let from: Option<i64> = conn
        .query_row(
            "SELECT playlist_id FROM playlist_tracks WHERE id = ?1",
            [member_id],
            |row| row.get(0),
        )
        .ok();
    let Some(from) = from else { return Ok(()) };
    if from == to_playlist {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let next: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_tracks WHERE playlist_id = ?1",
            [to_playlist],
            |row| row.get(0),
        )
        .unwrap_or(0);
    tx.execute(
        "UPDATE playlist_tracks SET playlist_id = ?2, position = ?3 WHERE id = ?1",
        rusqlite::params![member_id, to_playlist, next],
    )?;
    tx.execute(
        "UPDATE playlists SET updated = ?2 WHERE id = ?1 OR id = ?3",
        rusqlite::params![from, now, to_playlist],
    )?;
    tx.commit()
}

/// Rewrite a playlist's order to exactly `member_ids`, positions 0..n. The
/// caller passes the full ordered member list (a drag-reorder result); ids
/// not in the list keep their old position and sort after.
pub fn reorder(
    conn: &mut Connection,
    playlist_id: i64,
    member_ids: &[i64],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE playlist_tracks SET position = ?2 WHERE id = ?1 AND playlist_id = ?3",
        )?;
        for (pos, &member_id) in member_ids.iter().enumerate() {
            stmt.execute(rusqlite::params![member_id, pos as i64, playlist_id])?;
        }
    }
    tx.execute(
        "UPDATE playlists SET updated = ?2 WHERE id = ?1",
        rusqlite::params![playlist_id, now],
    )?;
    tx.commit()
}

/// Move `members` into `playlist_id` and drop them in as one contiguous block
/// just before `before` (a member id already in the target), or at the end
/// when `before` is None. Members from other playlists are pulled in keeping
/// their snapshot; members already there are repositioned. The dragged block
/// keeps the given order, the rest of the target keeps its relative order.
/// This is the one primitive behind every playlist drag, single or multi,
/// reorder or cross-playlist move. The target and any source playlists stamp
/// updated. `before` must not name one of `members`; the caller drops a
/// self-drop before it gets here.
pub fn place_members(
    conn: &mut Connection,
    playlist_id: i64,
    members: &[i64],
    before: Option<i64>,
    now: i64,
) -> rusqlite::Result<()> {
    if members.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    // Source playlists losing a member want their stamp bumped too.
    let mut touched: Vec<i64> = vec![playlist_id];
    {
        let mut src = tx.prepare("SELECT playlist_id FROM playlist_tracks WHERE id = ?1")?;
        let mut mv = tx.prepare("UPDATE playlist_tracks SET playlist_id = ?2 WHERE id = ?1")?;
        for &member in members {
            if let Ok(from) = src.query_row([member], |row| row.get::<_, i64>(0)) {
                if from != playlist_id && !touched.contains(&from) {
                    touched.push(from);
                }
            }
            mv.execute(rusqlite::params![member, playlist_id])?;
        }
    }
    // The target's remaining members in order, without the dragged block.
    let moved: std::collections::HashSet<i64> = members.iter().copied().collect();
    let existing: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position, id",
        )?;
        let rows = stmt.query_map([playlist_id], |row| row.get::<_, i64>(0))?;
        rows.filter_map(Result::ok)
            .filter(|id| !moved.contains(id))
            .collect()
    };
    // Splice the dragged block in before the target member, else at the end.
    let at = before
        .and_then(|b| existing.iter().position(|&id| id == b))
        .unwrap_or(existing.len());
    let mut order: Vec<i64> = Vec::with_capacity(existing.len() + members.len());
    order.extend_from_slice(&existing[..at]);
    order.extend_from_slice(members);
    order.extend_from_slice(&existing[at..]);
    {
        let mut up = tx.prepare("UPDATE playlist_tracks SET position = ?2 WHERE id = ?1")?;
        for (pos, &id) in order.iter().enumerate() {
            up.execute(rusqlite::params![id, pos as i64])?;
        }
    }
    // Dragging a track onto favourites it was already in leaves it favourited
    // once, not twice. The row that stays keeps its place in the order.
    if favourites_id(&tx)? == Some(playlist_id) {
        dedupe_favourite_members(&tx, playlist_id)?;
    }
    {
        let mut stamp = tx.prepare("UPDATE playlists SET updated = ?2 WHERE id = ?1")?;
        for id in touched {
            stamp.execute(rusqlite::params![id, now])?;
        }
    }
    tx.commit()
}

/// Drop several members at once by row id, across whatever playlists they
/// belong to. Each playlist they leave stamps updated. Positions keep their
/// gaps, the same as the single remove; the next reorder closes them.
pub fn remove_members(conn: &mut Connection, member_ids: &[i64], now: i64) -> rusqlite::Result<()> {
    if member_ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let mut touched: Vec<i64> = Vec::new();
    {
        let mut src = tx.prepare("SELECT playlist_id FROM playlist_tracks WHERE id = ?1")?;
        let mut del = tx.prepare("DELETE FROM playlist_tracks WHERE id = ?1")?;
        for &member in member_ids {
            if let Ok(from) = src.query_row([member], |row| row.get::<_, i64>(0)) {
                if !touched.contains(&from) {
                    touched.push(from);
                }
            }
            del.execute([member])?;
        }
    }
    {
        let mut stamp = tx.prepare("UPDATE playlists SET updated = ?2 WHERE id = ?1")?;
        for id in touched {
            stamp.execute(rusqlite::params![id, now])?;
        }
    }
    tx.commit()
}

/// A playlist's members in order, tags resolved live with the snapshot as
/// fallback so a deleted track still shows a name.
pub fn tracks(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Vec<PlaylistTrack>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.id, m.track_id,
                COALESCE(t.title, m.title),
                COALESCE(t.artist, m.artist),
                COALESCE(t.album, m.album),
                COALESCE(t.album_artist, ''),
                COALESCE(t.year, 0),
                COALESCE(t.genre, ''),
                COALESCE(t.duration_ms, 0),
                COALESCE(t.codec, ''),
                COALESCE(t.bitrate, 0),
                COALESCE(t.sample_rate, 0),
                COALESCE(t.bit_depth, 0),
                COALESCE(t.rating, 0),
                COALESCE(t.path, m.path)
         FROM playlist_tracks m LEFT JOIN tracks t ON t.id = m.track_id
         WHERE m.playlist_id = ?1
         ORDER BY m.position, m.id",
    )?;
    let rows = stmt.query_map([playlist_id], |row| {
        Ok(PlaylistTrack {
            member_id: row.get(0)?,
            track_id: row.get(1)?,
            title: row.get(2)?,
            artist: row.get(3)?,
            album: row.get(4)?,
            album_artist: row.get(5)?,
            year: row.get(6)?,
            genre: row.get(7)?,
            duration_ms: row.get(8)?,
            codec: row.get(9)?,
            bitrate_kbps: row.get(10)?,
            sample_rate_hz: row.get(11)?,
            bit_depth: row.get(12)?,
            rating: row.get(13)?,
            path: row.get(14)?,
        })
    })?;
    rows.collect()
}

/// A playlist's track ids in play order. What the panel hands the player to
/// start the whole list. Only tracks still in the catalog, since a snapshot
/// row has no file to play.
pub fn ids(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.track_id FROM playlist_tracks m JOIN tracks t ON t.id = m.track_id
         WHERE m.playlist_id = ?1 ORDER BY m.position, m.id",
    )?;
    let rows = stmt.query_map([playlist_id], |row| row.get(0))?;
    rows.collect()
}

/// One row for an M3U export: the file to point at, display tags for the
/// `#EXTINF` line, and the duration in whole seconds. Only local members whose
/// track is still in the catalog, since a snapshot row has no file to write.
pub struct ExportTrack {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub duration_secs: i64,
}

/// A playlist's playable members in order, resolved to what an M3U needs.
/// Deleted and non-local tracks fall away, the same way [`ids`] drops what
/// has no file behind it.
pub fn export_rows(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Vec<ExportTrack>> {
    let mut stmt = conn.prepare_cached(
        "SELECT t.path, t.title, t.artist, t.duration_ms
         FROM playlist_tracks m JOIN tracks t ON t.id = m.track_id
         WHERE m.playlist_id = ?1 AND t.source = 'local'
         ORDER BY m.position, m.id",
    )?;
    let rows = stmt.query_map([playlist_id], |row| {
        Ok(ExportTrack {
            path: row.get(0)?,
            title: row.get(1)?,
            artist: row.get(2)?,
            // Round to the nearest second, the resolution #EXTINF wants.
            duration_secs: (row.get::<_, i64>(3)? + 500) / 1000,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::{store, TrackRow};

    fn track(path: &str, title: &str, artist: &str, album: &str) -> TrackRow {
        TrackRow {
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
            genre: String::new(),
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

    fn seed() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First"),
                track("/m/2.mp3", "Two", "A", "First"),
                track("/m/3.mp3", "Three", "B", "Second"),
            ],
        )
        .unwrap();
        conn
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
        conn.prepare(&format!(
            "SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}'"
        ))
        .unwrap()
        .exists([])
        .unwrap()
    }

    /// The smart-playlists rung adds its columns to a fresh database and to
    /// one written before the step existed. The ladder is forward-only and
    /// additive, so the older file has to converge by running the tail, not
    /// by being rebuilt.
    #[test]
    fn the_smart_columns_land_on_a_fresh_db_and_an_older_one() {
        let fresh = Connection::open_in_memory().unwrap();
        store::init_schema(&fresh).unwrap();
        assert!(has_column(&fresh, "playlists", "kind"));
        assert!(has_column(&fresh, "playlists", "definition"));

        // What a binary from before the step wrote: the ladder run up to the
        // rung and stopped there, rather than a current file wound back,
        // which would leave every later rung's columns in place for the
        // rerun to trip over.
        let conn = Connection::open_in_memory().unwrap();
        store::run_ladder_before(&conn, "smart-playlists").unwrap();
        assert!(!has_column(&conn, "playlists", "kind"));
        create(&conn, "From The Old Build", 100).unwrap();

        store::init_schema(&conn).unwrap();
        assert!(has_column(&conn, "playlists", "kind"));
        assert!(has_column(&conn, "playlists", "definition"));
        let existing = list(&conn).unwrap();
        assert_eq!(existing.len(), 1);
        assert_eq!(
            existing[0].kind,
            PlaylistKind::Static,
            "a playlist from before the step is a plain one"
        );
    }

    /// A definition comes back intact through the column, filter and all.
    #[test]
    fn a_definition_round_trips_through_the_row() {
        let conn = seed();
        let def = SmartDef {
            query: "rating:>=4 plays:0".into(),
            filter: FilterSet {
                fields: vec![(
                    crate::projection::FilterField::Genre,
                    vec!["Shoegaze".into(), "Dream Pop".into()],
                )],
                ids: None,
            },
            sort: Some((SortKey::Rating, true)),
            limit: Some(50),
        };
        let id = create_smart(&conn, "Best Of", &def, 100).unwrap();

        assert_eq!(definition(&conn, id).unwrap().as_ref(), Some(&def));
        let row = list(&conn)
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        assert_eq!(row.kind, PlaylistKind::Smart);
        assert_eq!(row.tracks, 0, "a smart playlist holds no member rows");

        // An edit rewrites it whole.
        let edited = SmartDef {
            query: "artist:air".into(),
            ..SmartDef::default()
        };
        set_definition(&conn, id, &edited, 110).unwrap();
        assert_eq!(definition(&conn, id).unwrap().as_ref(), Some(&edited));

        // A static playlist has none, and unreadable JSON reads as none
        // rather than an error.
        let plain = create(&conn, "Plain", 100).unwrap();
        assert_eq!(definition(&conn, plain).unwrap(), None);
        conn.execute(
            "UPDATE playlists SET definition = 'not json' WHERE id = ?1",
            [id],
        )
        .unwrap();
        assert_eq!(definition(&conn, id).unwrap(), None);
    }

    /// The saved query evaluates against the projection, and the limit cuts
    /// the sorted result rather than the order it was read in.
    #[test]
    fn a_smart_definition_resolves_to_the_tracks_it_names() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut loved = track("/m/1.mp3", "Loved", "A", "First");
        loved.rating = 100;
        let mut liked = track("/m/2.mp3", "Liked", "B", "Second");
        liked.rating = 80;
        let mut played = track("/m/3.mp3", "Played", "C", "Third");
        played.rating = 100;
        let plain = track("/m/4.mp3", "Plain", "D", "Fourth");
        store::insert_batch(&mut conn, &[loved, liked, played, plain]).unwrap();
        crate::listens::append(
            &conn,
            &crate::listens::Listen {
                track_id: 3,
                played_at: 1_700_000_000,
                title: "Played".into(),
                artist: "C".into(),
                album: "Third".into(),
                genre: String::new(),
                path: "/m/3.mp3".into(),
            },
        )
        .unwrap();
        let projection = Projection::load_serial(&conn, false).unwrap();
        let order = Arc::new(projection.sort_canonical());

        let def = SmartDef {
            query: "rating:>=4 plays:0".into(),
            ..SmartDef::default()
        };
        assert_eq!(
            def.ids(&projection, order.clone()),
            [1, 2],
            "the four-star-and-up tracks nobody has played"
        );

        // The limit takes the head of the sort it asked for, not of the
        // canonical order: descending by title puts Loved first.
        let capped = SmartDef {
            query: "rating:>=4".into(),
            sort: Some((SortKey::Title, true)),
            limit: Some(2),
            ..SmartDef::default()
        };
        assert_eq!(capped.ids(&projection, order.clone()), [3, 1]);

        // An empty query is the whole library through the sort.
        let everything = SmartDef {
            limit: Some(1),
            ..SmartDef::default()
        };
        assert_eq!(everything.ids(&projection, order).len(), 1);
    }

    /// The "never played" list, which is the whole promise of the feature:
    /// a track that gets played leaves it on the next materialization, with
    /// nothing written to any playlist row. Nothing invalidates in between,
    /// so the panel showing the old answer until it refreshes is the known
    /// cost, not a bug in the evaluation.
    #[test]
    fn a_never_played_list_drops_a_track_once_it_plays() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One", "A", "First"),
                track("/m/2.mp3", "Two", "A", "First"),
            ],
        )
        .unwrap();
        let def = SmartDef {
            query: "plays:0".into(),
            ..SmartDef::default()
        };
        let id = create_smart(&conn, "Never Played", &def, 100).unwrap();

        let materialize = |conn: &Connection| -> Vec<i64> {
            let def = definition(conn, id).unwrap().unwrap();
            let projection = Projection::load_serial(conn, false).unwrap();
            let order = Arc::new(projection.sort_canonical());
            def.ids(&projection, order)
        };
        assert_eq!(materialize(&conn), [1, 2]);

        crate::listens::append(
            &conn,
            &crate::listens::Listen {
                track_id: 1,
                played_at: 1_700_000_000,
                title: "One".into(),
                artist: "A".into(),
                album: "First".into(),
                genre: String::new(),
                path: "/m/1.mp3".into(),
            },
        )
        .unwrap();
        assert_eq!(
            materialize(&conn),
            [2],
            "the played track leaves the list on the next pass"
        );
        assert_eq!(
            list(&conn).unwrap()[0].tracks,
            0,
            "and no member row was written either way"
        );
    }

    #[test]
    fn duplicates_are_kept_and_addressed_by_member() {
        let mut conn = seed();
        let pl = create(&conn, "Favourites", 100).unwrap();

        add(&mut conn, pl, &[1, 2, 1], 100).unwrap();
        let members = tracks(&conn, pl).unwrap();
        assert_eq!(
            members.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
            ["One", "Two", "One"],
            "the same track lands twice"
        );

        // Remove only the first occurrence, by its member id.
        remove_member(&conn, members[0].member_id, 110).unwrap();
        assert_eq!(
            tracks(&conn, pl)
                .unwrap()
                .iter()
                .map(|m| m.title.as_str())
                .collect::<Vec<_>>(),
            ["Two", "One"],
            "the other copy stays"
        );
        assert_eq!(ids(&conn, pl).unwrap(), [2, 1]);
    }

    #[test]
    fn reorder_and_move_between_playlists() {
        let mut conn = seed();
        let a = create(&conn, "A", 100).unwrap();
        let b = create(&conn, "B", 100).unwrap();
        add(&mut conn, a, &[1, 2, 3], 100).unwrap();

        let members = tracks(&conn, a).unwrap();
        let order: Vec<i64> = vec![
            members[2].member_id,
            members[0].member_id,
            members[1].member_id,
        ];
        reorder(&mut conn, a, &order, 110).unwrap();
        assert_eq!(ids(&conn, a).unwrap(), [3, 1, 2]);

        // Move the first member of A into B.
        let first = tracks(&conn, a).unwrap()[0].member_id;
        move_member(&mut conn, first, b, 120).unwrap();
        assert_eq!(ids(&conn, a).unwrap(), [1, 2]);
        assert_eq!(ids(&conn, b).unwrap(), [3]);
    }

    #[test]
    fn favourites_playlist_is_made_once_and_toggles_cleanly() {
        let mut conn = seed();
        let fav = ensure_favourites(&conn, 100).unwrap();
        // Idempotent: a second call returns the same playlist, makes no other.
        assert_eq!(ensure_favourites(&conn, 100).unwrap(), fav);
        assert_eq!(
            list(&conn).unwrap().len(),
            1,
            "just the one favourites playlist"
        );
        assert!(list(&conn).unwrap()[0].favourite, "and it carries the flag");

        assert!(!is_favourite(&conn, 1).unwrap());
        set_favourite(&mut conn, 1, true, 110).unwrap();
        // On twice does not duplicate the member.
        set_favourite(&mut conn, 1, true, 111).unwrap();
        assert!(is_favourite(&conn, 1).unwrap());
        assert_eq!(favourite_track_ids(&conn).unwrap(), [1]);

        set_favourite(&mut conn, 1, false, 120).unwrap();
        assert!(!is_favourite(&conn, 1).unwrap());
        assert!(favourite_track_ids(&conn).unwrap().is_empty());
    }

    #[test]
    fn favourites_hold_a_track_once_however_it_arrives() {
        let mut conn = seed();
        let fav = ensure_favourites(&conn, 100).unwrap();

        // The menu's Add to Playlist path, over a track the heart already has.
        set_favourite(&mut conn, 1, true, 110).unwrap();
        add(&mut conn, fav, &[1, 2], 111).unwrap();
        assert_eq!(
            ids(&conn, fav).unwrap(),
            [1, 2],
            "the add lands the new track and leaves the old one alone"
        );

        // And the drag path, pulling a member in from another playlist.
        let other = create(&conn, "Other", 100).unwrap();
        add(&mut conn, other, &[1], 112).unwrap();
        let dragged = tracks(&conn, other).unwrap()[0].member_id;
        place_members(&mut conn, fav, &[dragged], None, 113).unwrap();
        assert_eq!(
            ids(&conn, fav).unwrap(),
            [1, 2],
            "still favourited once, and the drag emptied its source"
        );
        assert!(tracks(&conn, other).unwrap().is_empty());

        // One click clears it, which is the whole point of holding the line.
        set_favourite(&mut conn, 1, false, 120).unwrap();
        assert!(!is_favourite(&conn, 1).unwrap());
    }

    #[test]
    fn startup_clears_duplicates_an_older_build_left_behind() {
        let mut conn = seed();
        let fav = ensure_favourites(&conn, 100).unwrap();
        add(&mut conn, fav, &[1, 2], 100).unwrap();
        // A second row for track 1, the way a menu add made one before the
        // writes kept them out.
        conn.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position, title, artist, album, path)
             VALUES (?1, 1, 9, 'One', 'A', 'First', '/m/1.mp3')",
            [fav],
        )
        .unwrap();
        assert_eq!(ids(&conn, fav).unwrap(), [1, 2, 1]);

        assert_eq!(dedupe_favourites(&conn, 130).unwrap(), 1, "one row dropped");
        assert_eq!(ids(&conn, fav).unwrap(), [1, 2]);
        assert_eq!(
            dedupe_favourites(&conn, 131).unwrap(),
            0,
            "and the sweep is a no-op on a list that's already clean"
        );
    }

    #[test]
    fn favourites_pins_to_the_top_of_the_list() {
        let conn = seed();
        create(&conn, "Later", 200).unwrap();
        ensure_favourites(&conn, 100).unwrap();
        let names: Vec<String> = list(&conn).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            ["Favourites", "Later"],
            "favourites leads even though it is older"
        );
    }

    #[test]
    fn place_members_reorders_a_block_before_a_target() {
        let mut conn = seed();
        let pl = create(&conn, "A", 100).unwrap();
        add(&mut conn, pl, &[1, 2, 3], 100).unwrap();
        let m = tracks(&conn, pl).unwrap();
        // Move members 1 and 3 (the block) to just before member 2.
        place_members(
            &mut conn,
            pl,
            &[m[0].member_id, m[2].member_id],
            Some(m[1].member_id),
            110,
        )
        .unwrap();
        assert_eq!(
            ids(&conn, pl).unwrap(),
            [1, 3, 2],
            "the block lands before the target, in order"
        );
    }

    #[test]
    fn place_members_appends_when_no_target() {
        let mut conn = seed();
        let pl = create(&conn, "A", 100).unwrap();
        add(&mut conn, pl, &[1, 2, 3], 100).unwrap();
        let m = tracks(&conn, pl).unwrap();
        place_members(&mut conn, pl, &[m[0].member_id], None, 110).unwrap();
        assert_eq!(
            ids(&conn, pl).unwrap(),
            [2, 3, 1],
            "no target sends the block to the end"
        );
    }

    #[test]
    fn place_members_moves_across_playlists_at_a_spot() {
        let mut conn = seed();
        let a = create(&conn, "A", 100).unwrap();
        let b = create(&conn, "B", 100).unwrap();
        add(&mut conn, a, &[1, 2], 100).unwrap();
        add(&mut conn, b, &[3], 100).unwrap();
        let from_a = tracks(&conn, a).unwrap()[0].member_id; // track 1
        let target_b = tracks(&conn, b).unwrap()[0].member_id; // track 3
        place_members(&mut conn, b, &[from_a], Some(target_b), 110).unwrap();
        assert_eq!(ids(&conn, a).unwrap(), [2], "it leaves A");
        assert_eq!(
            ids(&conn, b).unwrap(),
            [1, 3],
            "and lands before the target in B"
        );
    }

    #[test]
    fn remove_members_drops_several_across_playlists() {
        let mut conn = seed();
        let a = create(&conn, "A", 100).unwrap();
        let b = create(&conn, "B", 100).unwrap();
        add(&mut conn, a, &[1, 2], 100).unwrap();
        add(&mut conn, b, &[3], 100).unwrap();
        let drop_a = tracks(&conn, a).unwrap()[0].member_id; // track 1
        let drop_b = tracks(&conn, b).unwrap()[0].member_id; // track 3
        remove_members(&mut conn, &[drop_a, drop_b], 110).unwrap();
        assert_eq!(ids(&conn, a).unwrap(), [2]);
        assert!(ids(&conn, b).unwrap().is_empty());
    }

    #[test]
    fn export_rows_are_ordered_and_playable_only() {
        let mut conn = seed();
        let pl = create(&conn, "Export", 100).unwrap();
        add(&mut conn, pl, &[3, 1], 100).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();

        let rows = export_rows(&conn, pl).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["/m/3.mp3"],
            "order follows the playlist, the deleted track drops with no file behind it"
        );
        assert_eq!(rows[0].title, "Three");
    }

    /// A member's stored path snapshot, for asserting on the column the
    /// reattach passes maintain.
    fn member_path(conn: &Connection, member_id: i64) -> String {
        conn.query_row(
            "SELECT path FROM playlist_tracks WHERE id = ?1",
            [member_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn reattach_relinks_by_path_after_a_prune() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();

        // The file goes missing at scan time and its row prunes; later it
        // comes back at the same path under a fresh id.
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        assert!(ids(&conn, pl).unwrap().is_empty(), "the member dangles");
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First")]).unwrap();
        let new_id = store::id_for_path(&conn, "/m/1.mp3").unwrap().unwrap();
        assert_ne!(new_id, 1, "the returned file lands under a fresh id");

        assert_eq!(reattach(&conn).unwrap(), Some(1));
        assert_eq!(ids(&conn, pl).unwrap(), [new_id], "the member plays again");
    }

    #[test]
    fn reattach_falls_back_to_the_tag_snapshot() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();

        // The file returns at a different path (a reorganize done with the
        // app closed), so the path snapshot misses; its tags still name it.
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/new/1.mp3", "One", "A", "First")]).unwrap();
        let new_id = store::id_for_path(&conn, "/new/1.mp3").unwrap().unwrap();

        assert_eq!(reattach(&conn).unwrap(), Some(1));
        let members = tracks(&conn, pl).unwrap();
        assert_eq!(members[0].track_id, new_id);
        assert_eq!(
            member_path(&conn, members[0].member_id),
            "/new/1.mp3",
            "the tag match rewrites the path snapshot for next time"
        );
    }

    #[test]
    fn reattach_never_guesses_between_ambiguous_tags() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        // Two candidates have the snapshot's tags and neither is at its
        // path; picking one would be a coin flip, so neither is taken.
        store::insert_batch(
            &mut conn,
            &[
                track("/x/1.mp3", "One", "A", "First"),
                track("/y/1.mp3", "One", "A", "First"),
            ],
        )
        .unwrap();

        assert_eq!(reattach(&conn).unwrap(), Some(0));
        assert!(
            ids(&conn, pl).unwrap().is_empty(),
            "the member stays a snapshot"
        );
        assert_eq!(tracks(&conn, pl).unwrap()[0].title, "One", "still readable");
    }

    /// Playlists whose members all still have their tracks never reach the
    /// matchers, the same gate the listen pass runs: this is called after
    /// every scan and every reindex, and the tag matcher is far too
    /// expensive to run for nothing.
    #[test]
    fn reattach_gates_on_a_library_with_nothing_dangling() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1, 2], 100).unwrap();
        assert_eq!(reattach(&conn).unwrap(), None, "nothing to match");
        assert_eq!(reattach(&conn).unwrap(), None);

        // One pruned file is enough to open the gate, and the pass behind it
        // does what it always did.
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/m/1.mp3", "One", "A", "First")]).unwrap();
        let new_id = store::id_for_path(&conn, "/m/1.mp3").unwrap().unwrap();
        assert_eq!(reattach(&conn).unwrap(), Some(1));
        assert_eq!(ids(&conn, pl).unwrap(), [new_id, 2]);
        assert_eq!(reattach(&conn).unwrap(), None, "and it closes again");
    }

    #[test]
    fn reattach_keeps_the_path_snapshot_current_across_renames() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();

        // A rename keeps the id, so the member never dangles; the refresh
        // pass moves its path snapshot along so a later prune can still be
        // matched back.
        store::rename_within(&mut conn, Path::new("/m/1.mp3"), Path::new("/m/one.mp3")).unwrap();
        assert_eq!(
            reattach(&conn).unwrap(),
            None,
            "nothing dangles on a rename, so the matchers never run"
        );
        let member = tracks(&conn, pl).unwrap()[0].member_id;
        assert_eq!(member_path(&conn, member), "/m/one.mp3");

        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();
        store::insert_batch(&mut conn, &[track("/m/one.mp3", "One", "A", "First")]).unwrap();
        let new_id = store::id_for_path(&conn, "/m/one.mp3").unwrap().unwrap();
        assert_eq!(reattach(&conn).unwrap(), Some(1));
        assert_eq!(
            ids(&conn, pl).unwrap(),
            [new_id],
            "the refreshed path relinks"
        );
    }

    #[test]
    fn snapshot_outlives_a_deleted_track() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();

        let rows = tracks(&conn, pl).unwrap();
        assert_eq!(rows[0].title, "One", "the snapshot keeps the row readable");
        assert_eq!(
            rows[0].path, "/m/1.mp3",
            "and the snapshot path keeps the cover column resolvable"
        );
        assert!(
            ids(&conn, pl).unwrap().is_empty(),
            "but a deleted track has no file to play"
        );
    }
}
