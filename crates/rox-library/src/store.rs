//! The durable side of ADR 5: bundled SQLite in WAL mode. The write path is
//! batched upsert transactions from the scanner; the read path is the
//! projection load, either one reader or one reader per core over disjoint
//! rowid ranges (WAL gives concurrent readers for free).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::TrackRow;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    // Source-qualified identity per the components contract: local files are
    // the first source, streaming extensions add rows instead of migrations.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks (
            id           INTEGER PRIMARY KEY,
            source       TEXT NOT NULL DEFAULT 'local',
            path         TEXT NOT NULL,
            title        TEXT NOT NULL,
            artist       TEXT NOT NULL,
            album_artist TEXT NOT NULL DEFAULT '',
            album        TEXT NOT NULL,
            genre        TEXT NOT NULL,
            year         INTEGER NOT NULL,
            disc_no      INTEGER NOT NULL DEFAULT 0,
            track_no     INTEGER NOT NULL,
            duration_ms  INTEGER NOT NULL,
            codec        TEXT NOT NULL DEFAULT '',
            bitrate      INTEGER NOT NULL DEFAULT 0,
            rating       INTEGER NOT NULL DEFAULT 0,
            added        INTEGER NOT NULL DEFAULT 0,
            size         INTEGER NOT NULL,
            mtime        INTEGER NOT NULL,
            UNIQUE (source, path)
        );",
    )?;
    // A library from before the album artist column: add it, and reset
    // every mtime so the next scan re-reads tags instead of skipping the
    // files as unchanged, which would leave the column empty forever.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'album_artist'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN album_artist TEXT NOT NULL DEFAULT '';
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // Same move for a library from before codec and bitrate.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'codec'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN codec TEXT NOT NULL DEFAULT '';
             ALTER TABLE tracks ADD COLUMN bitrate INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // And for a library from before the disc number.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'disc_no'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN disc_no INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET mtime = 0;",
        )?;
    }
    // And for a library from before ratings. No mtime reset here: the
    // rating is the app's own, never read from tags, so no rescan is owed.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'rating'")?;
    if !stmt.exists([])? {
        conn.execute_batch("ALTER TABLE tracks ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;")?;
    }
    // And for a library from before the added timestamp: add it and
    // backfill every row to now, so tracks scanned in after the upgrade
    // sort newer while the existing catalog clusters at the upgrade time.
    // No mtime reset: the timestamp is the app's own, never read from tags.
    let mut stmt =
        conn.prepare("SELECT 1 FROM pragma_table_info('tracks') WHERE name = 'added'")?;
    if !stmt.exists([])? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN added INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET added = CAST(strftime('%s', 'now') AS INTEGER);",
        )?;
    }
    // The listen events ride the same database and schema setup (ADR 11).
    crate::listens::init_schema(conn)?;
    // Playlists share the database too (ADR 16).
    crate::playlists::init_schema(conn)?;
    Ok(())
}

pub fn count(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)
}

/// Insert or refresh one batch of local rows inside a single transaction. An
/// existing (source, path) row keeps its id, so projection db_ids stay valid
/// across a rescan. A re-read file's rating imports like any tag, except a
/// zero keeps the stored one: a rating the writer could not land in the
/// file (wav, read-only media) must not vanish because the file changed.
pub fn insert_batch(conn: &mut Connection, rows: &[TrackRow]) -> rusqlite::Result<()> {
    // The scan time stamps first-seen rows only: the conflict update below
    // leaves `added` alone, so a rescan of an unchanged or edited file keeps
    // the moment it entered the library.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO tracks
             (path, title, artist, album_artist, album, genre, year, disc_no, track_no,
              duration_ms, codec, bitrate, rating, added, size, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT (source, path) DO UPDATE SET
                title = excluded.title, artist = excluded.artist,
                album_artist = excluded.album_artist,
                album = excluded.album, genre = excluded.genre,
                year = excluded.year, disc_no = excluded.disc_no,
                track_no = excluded.track_no,
                duration_ms = excluded.duration_ms, codec = excluded.codec,
                bitrate = excluded.bitrate,
                rating = CASE excluded.rating WHEN 0 THEN rating ELSE excluded.rating END,
                size = excluded.size,
                mtime = excluded.mtime",
        )?;
        for r in rows {
            stmt.execute(rusqlite::params![
                r.path,
                r.title,
                r.artist,
                r.album_artist,
                r.album,
                r.genre,
                r.year,
                r.disc_no,
                r.track_no,
                r.duration_ms,
                r.codec,
                r.bitrate_kbps,
                r.rating,
                now,
                r.size as i64,
                r.mtime,
            ])?;
        }
    }
    tx.commit()
}

/// The half-open path range holding exactly the files under `root`: from
/// the root plus a trailing separator up to the separator's successor
/// byte. SQLite compares TEXT bytewise, so the (source, path) index
/// serves range queries directly where a LIKE would not.
fn path_range(root: &Path) -> (String, String) {
    let mut lo = root.to_string_lossy().into_owned();
    if !lo.ends_with(std::path::MAIN_SEPARATOR) {
        lo.push(std::path::MAIN_SEPARATOR);
    }
    let mut hi = lo.clone();
    hi.pop();
    hi.push((std::path::MAIN_SEPARATOR as u8 + 1) as char);
    (lo, hi)
}

/// One scope's rollup: how many tracks and distinct albums it holds and
/// what its files weigh on disk.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub tracks: u64,
    pub albums: u64,
    pub bytes: u64,
}

/// The rollup columns behind [`Stats`]. Albums are distinct
/// (album_artist, album) pairs joined on the unit separator so the pair
/// never collides across the boundary; untagged tracks (empty album)
/// count no album, and the CASE's NULL keeps them out of the DISTINCT.
const STATS_COLUMNS: &str = "COUNT(*),
     COUNT(DISTINCT CASE WHEN album <> '' THEN album_artist || char(31) || album END),
     COALESCE(SUM(size), 0)";

fn stats_row(r: &rusqlite::Row) -> rusqlite::Result<Stats> {
    Ok(Stats {
        tracks: r.get::<_, i64>(0)? as u64,
        albums: r.get::<_, i64>(1)? as u64,
        bytes: r.get::<_, i64>(2)? as u64,
    })
}

/// The whole library's rollup.
pub fn stats(conn: &Connection) -> rusqlite::Result<Stats> {
    conn.query_row(&format!("SELECT {STATS_COLUMNS} FROM tracks"), [], stats_row)
}

/// The rollup for the local tracks under one folder.
pub fn stats_under(conn: &Connection, root: &Path) -> rusqlite::Result<Stats> {
    let (lo, hi) = path_range(root);
    conn.query_row(
        &format!(
            "SELECT {STATS_COLUMNS} FROM tracks
             WHERE source = 'local' AND path >= ?1 AND path < ?2"
        ),
        rusqlite::params![lo, hi],
        stats_row,
    )
}

/// Drop every local track under one folder, for when it leaves the
/// library. The files themselves are untouched.
pub fn remove_under(conn: &Connection, root: &Path) -> rusqlite::Result<usize> {
    let (lo, hi) = path_range(root);
    conn.execute(
        "DELETE FROM tracks WHERE source = 'local' AND path >= ?1 AND path < ?2",
        rusqlite::params![lo, hi],
    )
}

/// Every local path with its (mtime, size), so a rescan can skip files that
/// have not changed without reading their tags.
pub fn local_files(conn: &Connection) -> rusqlite::Result<HashMap<String, (i64, u64)>> {
    let mut stmt = conn.prepare("SELECT path, mtime, size FROM tracks WHERE source = 'local'")?;
    let mut rows = stmt.query([])?;
    let mut out = HashMap::new();
    while let Some(row) = rows.next()? {
        out.insert(row.get(0)?, (row.get(1)?, row.get::<_, i64>(2)? as u64));
    }
    Ok(out)
}

/// The deepest directory holding every local track, for recovering the scan
/// root from a library indexed before anything recorded it. None on an
/// empty library.
pub fn common_root(conn: &Connection) -> rusqlite::Result<Option<PathBuf>> {
    let mut stmt = conn.prepare("SELECT path FROM tracks WHERE source = 'local'")?;
    let mut rows = stmt.query([])?;
    let mut root: Option<PathBuf> = None;
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let Some(dir) = Path::new(&path).parent() else {
            continue;
        };
        root = Some(match root {
            None => dir.to_path_buf(),
            Some(root) => root
                .components()
                .zip(dir.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    Ok(root.filter(|root| root.parent().is_some()))
}

/// Apply one file's committed tag changes to its row, so the projection
/// can reload the edit without a rescan. Only the columns the library
/// projects move; comment, composer, and custom fields have no column
/// and fall through. The stored mtime stays put on purpose: the write
/// bumped the file's, so the next rescan re-reads it and squares the
/// row with the tag wholesale.
pub fn apply_changes(
    conn: &Connection,
    id: i64,
    changes: &[crate::writer::Change],
) -> rusqlite::Result<()> {
    use crate::writer::Field;
    // The leading digits of a tag value: a "2020-05-01" date and a
    // "5/12" track fraction both reduce to the number the column holds,
    // the scanner's read of the same tags.
    fn leading(value: &str) -> i64 {
        let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().unwrap_or(0)
    }
    for change in changes {
        let value = change.value.as_deref().unwrap_or("");
        // The rating speaks the writer's 0-10 display number, not the
        // column's 0-100; a cleared or unparseable one lands as unrated.
        if change.field == Field::Rating {
            let rating = crate::rating::parse_display(value).unwrap_or(0);
            conn.execute(
                "UPDATE tracks SET rating = ?2 WHERE id = ?1",
                rusqlite::params![id, rating],
            )?;
            continue;
        }
        let (column, number) = match &change.field {
            Field::Title => ("title", false),
            Field::Artist => ("artist", false),
            Field::Album => ("album", false),
            Field::AlbumArtist => ("album_artist", false),
            Field::Genre => ("genre", false),
            Field::Year => ("year", true),
            Field::TrackNo => ("track_no", true),
            Field::DiscNo => ("disc_no", true),
            _ => continue,
        };
        if number {
            conn.execute(
                &format!("UPDATE tracks SET {column} = ?2 WHERE id = ?1"),
                rusqlite::params![id, leading(value)],
            )?;
        } else if column == "album_artist" && value.is_empty() {
            // A cleared album artist falls back to the track artist, the
            // scanner's grouping rule.
            conn.execute(
                "UPDATE tracks SET album_artist = artist WHERE id = ?1",
                rusqlite::params![id],
            )?;
        } else {
            conn.execute(
                &format!("UPDATE tracks SET {column} = ?2 WHERE id = ?1"),
                rusqlite::params![id, value],
            )?;
        }
    }
    Ok(())
}

/// One track's rating onto its row: the app's 0-100 scale, 0 unrated.
/// Ratings live in the library alone, never in the file's tags, so this
/// touches no mtime and owes no rescan.
pub fn set_rating(conn: &Connection, id: i64, rating: u8) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET rating = ?2 WHERE id = ?1",
        rusqlite::params![id, rating],
    )?;
    Ok(())
}

/// Resolve projection db_ids back to playable paths, in the order given.
pub fn paths_for(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT path FROM tracks WHERE id = ?1")?;
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if let Ok(path) = stmt.query_row([id], |r| r.get::<_, String>(0)) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Resolve a playable path to its track id, for marking the playing row.
/// Ok(None) when the path is not in the library.
pub fn id_for_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<i64>> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM tracks WHERE source = 'local' AND path = ?1")?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// The display tags for one track, what a path-keyed lookup returns.
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: u16,
}

/// Resolve a playable path back to its tags, for showing what is playing.
/// Ok(None) when the path is not in the library.
pub fn meta_for_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<TrackMeta>> {
    let mut stmt = conn.prepare_cached(
        "SELECT title, artist, album, track_no FROM tracks
         WHERE source = 'local' AND path = ?1",
    )?;
    let mut rows = stmt.query([path])?;
    match rows.next()? {
        Some(row) => Ok(Some(TrackMeta {
            title: row.get(0)?,
            artist: row.get(1)?,
            album: row.get(2)?,
            track_no: row.get::<_, i64>(3)? as u16,
        })),
        None => Ok(None),
    }
}

pub fn max_rowid(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tracks", [], |r| r.get(0))
}

/// Stream the projection columns for one rowid range, in id order. The
/// sink's string order mirrors the SELECT: title, artist, album artist,
/// album, genre, then codec after the numbers, the rating last.
#[allow(clippy::type_complexity)]
pub fn scan_range(
    conn: &Connection,
    lo: i64,
    hi: i64,
    mut sink: impl FnMut(i64, &str, &str, &str, &str, &str, u16, u16, u16, u32, &str, u16, u8, i64),
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, album_artist, album, genre, year, disc_no, track_no,
                duration_ms, codec, bitrate, rating, added
         FROM tracks WHERE id > ?1 AND id <= ?2 ORDER BY id",
    )?;
    let mut rows = stmt.query(rusqlite::params![lo, hi])?;
    while let Some(row) = rows.next()? {
        sink(
            row.get(0)?,
            row.get_ref(1)?.as_str().unwrap_or(""),
            row.get_ref(2)?.as_str().unwrap_or(""),
            row.get_ref(3)?.as_str().unwrap_or(""),
            row.get_ref(4)?.as_str().unwrap_or(""),
            row.get_ref(5)?.as_str().unwrap_or(""),
            row.get::<_, i64>(6)? as u16,
            row.get::<_, i64>(7)? as u16,
            row.get::<_, i64>(8)? as u16,
            row.get::<_, i64>(9)? as u32,
            row.get_ref(10)?.as_str().unwrap_or(""),
            row.get::<_, i64>(11)? as u16,
            row.get::<_, i64>(12)? as u8,
            row.get::<_, i64>(13)?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, album_artist: &str, album: &str, size: u64) -> TrackRow {
        TrackRow {
            path: path.into(),
            title: String::new(),
            artist: String::new(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms: 0,
            codec: String::new(),
            bitrate_kbps: 0,
            rating: 0,
            size,
            mtime: 0,
        }
    }

    #[test]
    fn stats_roll_up_tracks_albums_and_bytes() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        insert_batch(
            &mut conn,
            &[
                // One album twice, the same title under another artist,
                // an untagged track, and one outside the folder.
                row("/m/a/1.mp3", "X", "Album", 100),
                row("/m/a/2.mp3", "X", "Album", 200),
                row("/m/b/1.mp3", "Y", "Album", 300),
                row("/m/c/1.mp3", "Z", "", 50),
                row("/n/d/1.mp3", "W", "Other", 400),
            ],
        )
        .unwrap();

        let whole = stats(&conn).unwrap();
        assert_eq!(
            (whole.tracks, whole.albums, whole.bytes),
            (5, 3, 1050),
            "an empty album tag counts no album"
        );

        let under = stats_under(&conn, Path::new("/m")).unwrap();
        assert_eq!((under.tracks, under.albums, under.bytes), (4, 2, 650));
    }

    /// A rating lands on the row and a rescan's upsert leaves it alone,
    /// since ratings are the app's own and never come back from tags.
    #[test]
    fn ratings_survive_a_rescan() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let track = || row("/m/a/1.mp3", "X", "Album", 100);
        insert_batch(&mut conn, &[track()]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        set_rating(&conn, id, 75).unwrap();
        insert_batch(&mut conn, &[track()]).unwrap();

        let p = crate::projection::Projection::load_serial(&conn).unwrap();
        assert_eq!(p.resolve(0).rating, 75);
    }

    /// The scan timestamp stamps a row when it first lands and a rescan's
    /// upsert leaves it alone, so a re-read file keeps the moment it
    /// entered the library.
    #[test]
    fn added_stamps_once_and_survives_a_rescan() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let track = || row("/m/a/1.mp3", "X", "Album", 100);
        insert_batch(&mut conn, &[track()]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        let added: i64 = conn
            .query_row("SELECT added FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert!(added > 0, "a first insert stamps the scan time");

        // Pin it to a known past value, then rescan: the upsert must not
        // move it.
        conn.execute("UPDATE tracks SET added = 123 WHERE id = ?1", [id])
            .unwrap();
        insert_batch(&mut conn, &[track()]).unwrap();
        let after: i64 = conn
            .query_row("SELECT added FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 123, "a rescan keeps the first-seen scan time");

        let p = crate::projection::Projection::load_serial(&conn).unwrap();
        assert_eq!(p.resolve(0).added, 123, "the projection carries it through");
    }

    /// The edit path's landing half: committed changes move exactly their
    /// columns, the reloaded projection shows them, and everything else
    /// holds still.
    #[test]
    fn apply_changes_moves_only_named_columns() {
        use crate::writer::{Change, Field};
        let mut conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut before = row("/m/a/1.mp3", "X", "Album", 100);
        before.title = "Before".into();
        before.artist = "Someone".into();
        before.year = 1999;
        insert_batch(&mut conn, &[before]).unwrap();
        let id = id_for_path(&conn, "/m/a/1.mp3").unwrap().unwrap();

        apply_changes(
            &conn,
            id,
            &[
                Change {
                    field: Field::Title,
                    value: Some("After".into()),
                },
                Change {
                    field: Field::Year,
                    value: Some("2020-05-01".into()),
                },
                Change {
                    field: Field::TrackNo,
                    value: Some("5/12".into()),
                },
                Change {
                    field: Field::AlbumArtist,
                    value: None,
                },
                Change {
                    field: Field::Comment,
                    value: Some("no column".into()),
                },
            ],
        )
        .unwrap();

        let p = crate::projection::Projection::load_serial(&conn).unwrap();
        let v = p.resolve(0);
        assert_eq!(v.title, "After");
        assert_eq!(v.year, 2020, "the date's leading digits land as the year");
        assert_eq!(v.track_no, 5, "a track fraction lands as its number");
        assert_eq!(
            v.album_artist, "Someone",
            "a cleared album artist falls back to the artist"
        );
        assert_eq!((v.artist, v.album), ("Someone", "Album"), "untouched columns hold");
    }
}
