//! Custom playlists in the library database (ADR 16). A playlist is a named,
//! ordered list of member rows; a member carries the track id, its position,
//! and a snapshot of the identifying tags at add time - the same deletion
//! hedge the listen events use (ADR 11). While a track exists, reads resolve
//! through the live catalog, so a fixed tag shows on the playlist row too;
//! once the track is gone the snapshot keeps the row readable, though there is
//! no file left to play. Track identity survives a rescan on the rowid
//! (ADR 5), so a playlist follows its tracks across scans.
//!
//! Members are addressed by their own row id, not the track id: a playlist may
//! hold the same track more than once, so removing or moving a member acts on
//! one occurrence, not every copy of a track.

use rusqlite::{Connection, OptionalExtension};

/// The playlists and their member rows beside the tracks they key to. No
/// foreign key on purpose, matching the listens table: deleting a track keeps
/// its playlist rows, that is the snapshot's job. Duplicates are allowed, so
/// there is no uniqueness on (playlist, track).
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
        conn.execute_batch("ALTER TABLE playlists ADD COLUMN favourite INTEGER NOT NULL DEFAULT 0;")?;
    }
    Ok(())
}

/// A playlist in the sidebar list: its id, name, and how many tracks it holds.
/// `favourite` marks the one default playlist behind the heart column and the
/// Favourites menu; the panel pins it to the top and shields it from delete
/// and rename.
#[derive(Clone)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub tracks: u64,
    pub favourite: bool,
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
    /// The 0-5 star rating, 0 when unrated. Read live from the catalog for
    /// the panel's rating cell, like the album grouping fields.
    pub rating: u8,
    /// The file path, for the cover column's thumbnail; empty for a deleted
    /// track the snapshot keeps but the catalog no longer holds.
    pub path: String,
}

/// Create an empty playlist, returning its id. `now` is unix seconds.
pub fn create(conn: &Connection, name: &str, now: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO playlists (name, created, updated) VALUES (?1, ?2, ?2)",
        rusqlite::params![name, now],
    )?;
    Ok(conn.last_insert_rowid())
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
                p.favourite
         FROM playlists p
         ORDER BY p.favourite DESC, p.updated DESC, p.id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Playlist {
            id: row.get(0)?,
            name: row.get(1)?,
            tracks: row.get::<_, i64>(2)? as u64,
            favourite: row.get::<_, i64>(3)? != 0,
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
/// playlist gets a second member row. Stamps the playlist updated.
pub fn add(conn: &mut Connection, playlist_id: i64, track_ids: &[i64], now: i64) -> rusqlite::Result<()> {
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
                (playlist_id, track_id, position, title, artist, album)
             SELECT ?1, t.id, ?3, t.title, t.artist, t.album
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
/// self-drop before it reaches here.
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
        let mut stmt = tx
            .prepare("SELECT id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position, id")?;
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
    {
        let mut stamp = tx.prepare("UPDATE playlists SET updated = ?2 WHERE id = ?1")?;
        for id in touched {
            stamp.execute(rusqlite::params![id, now])?;
        }
    }
    tx.commit()
}

/// Drop several members at once by row id, across whatever playlists they sit
/// in. Each playlist they leave stamps updated. Positions keep their gaps, the
/// same as the single remove; the next reorder closes them.
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
                COALESCE(t.rating, 0),
                COALESCE(t.path, '')
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
            rating: row.get(11)?,
            path: row.get(12)?,
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
    use super::*;
    use crate::{store, TrackRow};

    fn track(path: &str, title: &str, artist: &str, album: &str) -> TrackRow {
        TrackRow {
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
            rating: 0,
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
        let order: Vec<i64> = vec![members[2].member_id, members[0].member_id, members[1].member_id];
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
        assert_eq!(list(&conn).unwrap().len(), 1, "just the one favourites playlist");
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
    fn favourites_pins_to_the_top_of_the_list() {
        let conn = seed();
        create(&conn, "Later", 200).unwrap();
        ensure_favourites(&conn, 100).unwrap();
        let names: Vec<String> = list(&conn).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Favourites", "Later"], "favourites leads even though it is older");
    }

    #[test]
    fn place_members_reorders_a_block_before_a_target() {
        let mut conn = seed();
        let pl = create(&conn, "A", 100).unwrap();
        add(&mut conn, pl, &[1, 2, 3], 100).unwrap();
        let m = tracks(&conn, pl).unwrap();
        // Move members 1 and 3 (the block) to just before member 2.
        place_members(&mut conn, pl, &[m[0].member_id, m[2].member_id], Some(m[1].member_id), 110)
            .unwrap();
        assert_eq!(ids(&conn, pl).unwrap(), [1, 3, 2], "the block lands before the target, in order");
    }

    #[test]
    fn place_members_appends_when_no_target() {
        let mut conn = seed();
        let pl = create(&conn, "A", 100).unwrap();
        add(&mut conn, pl, &[1, 2, 3], 100).unwrap();
        let m = tracks(&conn, pl).unwrap();
        place_members(&mut conn, pl, &[m[0].member_id], None, 110).unwrap();
        assert_eq!(ids(&conn, pl).unwrap(), [2, 3, 1], "no target sends the block to the end");
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
        assert_eq!(ids(&conn, b).unwrap(), [1, 3], "and lands before the target in B");
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

    #[test]
    fn snapshot_outlives_a_deleted_track() {
        let mut conn = seed();
        let pl = create(&conn, "Mix", 100).unwrap();
        add(&mut conn, pl, &[1], 100).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 1", []).unwrap();

        let rows = tracks(&conn, pl).unwrap();
        assert_eq!(rows[0].title, "One", "the snapshot keeps the row readable");
        assert!(
            ids(&conn, pl).unwrap().is_empty(),
            "but a deleted track has no file to play"
        );
    }
}
