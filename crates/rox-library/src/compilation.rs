//! Albums with no album artist tag whose tracks credit several artists: a
//! soundtrack or a compilation tagged without ALBUMARTIST. The scanner borrows
//! each track's artist as its album artist, so such an album would split into
//! one album per artist. This pass gives the whole album
//! [`VARIOUS_ARTISTS`], the value a tagged compilation carries, so every
//! (album artist, album) reader agrees it's one album: the projection, queue
//! groups (ADR 17), ReplayGain albums and the stats rollup.
//!
//! The group is one folder and one album title, the way a player grouping by
//! directory sees it. Two self-titled records by different artists in
//! different folders stay apart. Any row whose album artist came from a tag
//! keeps the group as tagged, and so does a record with guests: one artist
//! credited on every track ("A" and "A feat. B") isn't a compilation.
//!
//! A settled row carries `album_artist_settled`, so a folder that stops
//! qualifying (a guest's credit fixed, tracks deleted, an album artist tagged
//! on one file) hands every row its own artist back instead of leaving the
//! rest under Various Artists.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};

/// The album artist a settled album takes. MusicBrainz's name for a
/// compilation, so an album tagged that way and one settled here group alike.
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// One local row as the rule reads it.
struct Member {
    id: i64,
    artist: String,
    album_artist: String,
    settled: bool,
}

impl Member {
    /// The album artist is the track's own artist, or an earlier settle's.
    /// Anything else came from a tag.
    fn borrowed(&self) -> bool {
        self.settled || self.album_artist == self.artist
    }
}

/// Whether one folder's album is a compilation: every album artist borrowed
/// and at least two artists credited, none of them on every track. Artists
/// compare without case, so "Abba" and "ABBA" stay one artist, and an
/// untagged track's empty artist doesn't count as a second one.
fn is_compilation(album: &str, members: &[Member]) -> bool {
    if album.is_empty() || !members.iter().all(Member::borrowed) {
        return false;
    }

    let mut artists: Vec<String> = members
        .iter()
        .map(|m| m.artist.to_lowercase())
        .filter(|a| !a.is_empty())
        .collect();
    artists.sort();
    artists.dedup();
    if artists.len() < 2 {
        return false;
    }

    // A name inside every credit is the album's own artist with guests.
    !artists
        .iter()
        .any(|name| artists.iter().all(|credit| credits(credit, name)))
}

/// Whether `credit` names `name` as a whole word: "artist a feat. b" credits
/// "artist a", but "fair weather" doesn't credit "air".
fn credits(credit: &str, name: &str) -> bool {
    credit.match_indices(name).any(|(at, _)| {
        let before = credit[..at].chars().next_back();
        let after = credit[at + name.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// A stored path's folder with its trailing separator.
fn folder(path: &str) -> &str {
    &path[..path.rfind(['/', '\\']).map_or(0, |at| at + 1)]
}

/// Settle or release every (folder, album) group in `rows`, which must hold
/// each group whole. Returns how many rows changed.
fn settle(conn: &Connection, rows: Vec<(String, String, Member)>) -> rusqlite::Result<usize> {
    let mut groups: HashMap<(String, String), Vec<Member>> = HashMap::new();
    for (folder, album, member) in rows {
        groups.entry((folder, album)).or_default().push(member);
    }

    let mut join = conn.prepare_cached(
        "UPDATE tracks SET album_artist = ?2, album_artist_sort = '', album_artist_settled = 1
          WHERE id = ?1",
    )?;
    // Back to what the scanner stores for a file with no album artist tag.
    let mut release = conn.prepare_cached(
        "UPDATE tracks SET album_artist = artist, album_artist_sort = artist_sort,
                album_artist_settled = 0
          WHERE id = ?1",
    )?;

    let mut changed = 0;
    for ((_, album), members) in &groups {
        if is_compilation(album, members) {
            for member in members.iter().filter(|m| !m.settled) {
                changed += join.execute(params![member.id, VARIOUS_ARTISTS])?;
            }
        } else {
            for member in members.iter().filter(|m| m.settled) {
                changed += release.execute(params![member.id])?;
            }
        }
    }
    Ok(changed)
}

const SELECT: &str = "SELECT id, path, album, artist, album_artist, album_artist_settled
                        FROM tracks WHERE source = 'local'";

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<(String, String, Member)> {
    let path: String = row.get(1)?;
    Ok((
        folder(&path).to_owned(),
        row.get(2)?,
        Member {
            id: row.get(0)?,
            artist: row.get(3)?,
            album_artist: row.get(4)?,
            settled: row.get(5)?,
        },
    ))
}

/// Settle every local album in the library. The migration's pass over rows
/// scanned before the rule existed.
pub(crate) fn settle_all(conn: &Connection) -> rusqlite::Result<usize> {
    let rows = conn
        .prepare(SELECT)?
        .query_map([], read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    settle(conn, rows)
}

/// Settle or release the albums directly in these folders. Reads each folder
/// whole from the store, so rows the scan didn't re-read still count toward
/// the group.
pub fn settle_folders<'a>(
    conn: &mut Connection,
    dirs: impl IntoIterator<Item = &'a Path>,
) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let mut changed = 0;
    {
        // The path range lets the (source, path) index find the folder; the
        // folder match drops its subfolders.
        let mut stmt = tx.prepare_cached(&format!("{SELECT} AND path >= ?1 AND path < ?2"))?;
        for dir in dirs {
            let (lo, hi) = crate::store::path_range(dir);
            let rows = stmt
                .query_map(params![lo, hi], read_row)?
                .filter(|row| row.as_ref().map_or(true, |(folder, _, _)| *folder == lo))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            changed += settle(&tx, rows)?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackRow, store};

    fn track(path: &str, artist: &str, album_artist: &str, album: &str) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            path: path.into(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: artist.into(),
            album_sort: String::new(),
            genre: String::new(),
            comment: String::new(),
            month: 0,
            day: 0,
            year: 2009,
            disc_no: 1,
            track_no: 1,
            duration_ms: 200_000,
            codec: "flac".into(),
            bitrate_kbps: 0,
            sample_rate_hz: 44100,
            bit_depth: 16,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    /// A track with no album artist tag, as the scanner stores it.
    fn borrowed(path: &str, artist: &str, album: &str) -> TrackRow {
        track(path, artist, artist, album)
    }

    fn library(rows: &[TrackRow]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        conn
    }

    /// (path, album artist, album artist sort), by path.
    fn album_artists(conn: &Connection) -> Vec<(String, String, String)> {
        let mut stmt = conn
            .prepare("SELECT path, album_artist, album_artist_sort FROM tracks ORDER BY path")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn distinct_albums(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(DISTINCT album_artist || char(31) || album) FROM tracks",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// The shape of the issue #152 report: one folder, one album title, no
    /// album artist tag, and the artist varying per track.
    fn soundtrack() -> Vec<TrackRow> {
        let dir = "/m/Miley Cyrus - Hannah Montana The Movie (2009)";
        let artists = [
            "Hannah Montana",
            "Steve Rushton",
            "Miley Cyrus",
            "Hannah Montana",
            "Rascal Flatts",
            "Miley and Billy Ray Cyrus",
            "Billy Ray Cyrus",
            "Taylor Swift",
            "Hannah Montana",
            "Miley Cyrus",
        ];
        artists
            .iter()
            .enumerate()
            .map(|(i, artist)| {
                borrowed(
                    &format!("{dir}/{:02}.flac", i + 1),
                    artist,
                    "Hannah Montana The Movie",
                )
            })
            .collect()
    }

    #[test]
    fn a_folder_of_credited_artists_settles_into_one_album() {
        let rows = soundtrack();
        let mut conn = library(&rows);
        assert_eq!(distinct_albums(&conn), 7);

        let dir = Path::new(&rows[0].path).parent().unwrap();
        assert_eq!(settle_folders(&mut conn, [dir]).unwrap(), rows.len());

        assert_eq!(distinct_albums(&conn), 1);
        for (_, album_artist, sort) in album_artists(&conn) {
            assert_eq!(album_artist, VARIOUS_ARTISTS);
            assert_eq!(sort, "");
        }
    }

    /// A library as the build before the marked rung left it. The rows go in
    /// first because today's upsert names the marker column.
    fn before_marked_rung(rows: &[TrackRow]) -> Connection {
        let probe = Connection::open_in_memory().unwrap();
        store::run_ladder_before(&probe, "folder-compilations-marked").unwrap();
        let version: i64 = probe
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();

        let conn = library(rows);
        conn.execute_batch("ALTER TABLE tracks DROP COLUMN album_artist_settled;")
            .unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        conn
    }

    #[test]
    fn the_migration_settles_a_library_scanned_before_the_rule() {
        let conn = before_marked_rung(&soundtrack());
        assert_eq!(distinct_albums(&conn), 7);

        store::init_schema(&conn).unwrap();
        assert_eq!(distinct_albums(&conn), 1);
    }

    #[test]
    fn self_titled_records_in_their_own_folders_stay_apart() {
        let mut conn = library(&[
            borrowed("/m/Weezer/Weezer/01.flac", "Weezer", "Weezer"),
            borrowed("/m/Weezer/Weezer/02.flac", "Weezer", "Weezer"),
            borrowed(
                "/m/Peter Gabriel/Peter Gabriel/01.flac",
                "Peter Gabriel",
                "Peter Gabriel",
            ),
            borrowed(
                "/m/Peter Gabriel/Peter Gabriel/02.flac",
                "Peter Gabriel",
                "Peter Gabriel",
            ),
            borrowed("/m/Greatest Hits/A/01.flac", "Queen", "Greatest Hits"),
            borrowed("/m/Greatest Hits/B/01.flac", "ABBA", "Greatest Hits"),
        ]);

        assert_eq!(settle_all(&conn).unwrap(), 0);
        let dirs = [
            Path::new("/m/Weezer/Weezer"),
            Path::new("/m/Peter Gabriel/Peter Gabriel"),
            Path::new("/m/Greatest Hits/A"),
            Path::new("/m/Greatest Hits/B"),
        ];
        assert_eq!(settle_folders(&mut conn, dirs).unwrap(), 0);
        assert_eq!(distinct_albums(&conn), 4);
    }

    #[test]
    fn a_tagged_album_artist_keeps_the_album_as_tagged() {
        let mut conn = library(&[
            track("/m/ost/01.flac", "Hannah Montana", "Disney", "The Movie"),
            borrowed("/m/ost/02.flac", "Miley Cyrus", "The Movie"),
        ]);

        assert_eq!(settle_folders(&mut conn, [Path::new("/m/ost")]).unwrap(), 0);
        let stored: Vec<String> = album_artists(&conn).into_iter().map(|r| r.1).collect();
        assert_eq!(stored, ["Disney", "Miley Cyrus"]);
    }

    #[test]
    fn one_artist_in_any_case_is_still_one_artist() {
        let mut conn = library(&[
            borrowed("/m/abba/01.flac", "ABBA", "Gold"),
            borrowed("/m/abba/02.flac", "Abba", "Gold"),
        ]);

        assert_eq!(
            settle_folders(&mut conn, [Path::new("/m/abba")]).unwrap(),
            0
        );
    }

    #[test]
    fn a_reread_track_rejoins_its_settled_album() {
        let rows = soundtrack();
        let mut conn = library(&rows);
        let dir = Path::new(&rows[0].path).parent().unwrap();
        settle_folders(&mut conn, [dir]).unwrap();

        // A tag edit's reindex stores the borrowed artist again for the one file.
        store::insert_batch(&mut conn, &rows[2..3]).unwrap();
        assert_eq!(distinct_albums(&conn), 2);

        assert_eq!(settle_folders(&mut conn, [dir]).unwrap(), 1);
        assert_eq!(distinct_albums(&conn), 1);
    }

    #[test]
    fn a_record_with_guests_is_not_a_compilation() {
        let mut conn = library(&[
            borrowed("/m/a/01.flac", "Artist A", "Record"),
            borrowed("/m/a/02.flac", "Artist A feat. B", "Record"),
            borrowed("/m/a/03.flac", "artist a & C", "Record"),
            borrowed("/m/a/04.flac", "", "Record"),
        ]);

        assert_eq!(settle_folders(&mut conn, [Path::new("/m/a")]).unwrap(), 0);
    }

    #[test]
    fn a_name_inside_another_word_is_not_a_shared_artist() {
        assert!(credits("artist a feat. b", "artist a"));
        assert!(credits("b & air", "air"));
        assert!(!credits("fair weather", "air"));
        assert!(!credits("airborne", "air"));

        let mut conn = library(&[
            borrowed("/m/mix/01.flac", "Air", "Mix"),
            borrowed("/m/mix/02.flac", "Fair Weather", "Mix"),
        ]);
        assert_eq!(settle_folders(&mut conn, [Path::new("/m/mix")]).unwrap(), 2);
    }

    #[test]
    fn a_folder_that_stops_qualifying_hands_its_rows_their_artists_back() {
        let rows = vec![
            borrowed("/m/a/01.flac", "Artist A", "Record"),
            borrowed("/m/a/02.flac", "Other B", "Record"),
            borrowed("/m/a/03.flac", "Artist A", "Record"),
        ];
        let mut conn = library(&rows);
        let dir = Path::new("/m/a");
        assert_eq!(settle_folders(&mut conn, [dir]).unwrap(), 3);

        // The second track's credit fixed: the re-read row borrows its artist
        // again, and the settle releases the two it didn't re-read.
        store::insert_batch(&mut conn, &[borrowed("/m/a/02.flac", "Artist A", "Record")]).unwrap();
        assert_eq!(settle_folders(&mut conn, [dir]).unwrap(), 2);

        let stored = album_artists(&conn);
        assert!(
            stored
                .iter()
                .all(|(_, album_artist, _)| album_artist == "Artist A")
        );

        // A released row's sort name is its artist's own, as the scanner
        // borrows it. These rows carry none.
        assert_eq!(stored[0].2, "");
        assert_eq!(stored[2].2, "");
    }

    #[test]
    fn an_album_artist_tagged_on_one_file_releases_the_rest() {
        let rows = soundtrack();
        let mut conn = library(&rows);
        let dir = Path::new(&rows[0].path).parent().unwrap();
        settle_folders(&mut conn, [dir]).unwrap();

        let mut tagged = soundtrack().swap_remove(0);
        tagged.album_artist = "Disney".into();
        store::insert_batch(&mut conn, &[tagged]).unwrap();
        settle_folders(&mut conn, [dir]).unwrap();

        let stored: Vec<String> = album_artists(&conn).into_iter().map(|r| r.1).collect();
        assert_eq!(stored[0], "Disney");
        for (row, album_artist) in rows.iter().zip(&stored).skip(1) {
            assert_eq!(*album_artist, row.artist);
        }
    }

    #[test]
    fn a_tag_edit_to_the_album_artist_is_never_released() {
        let rows = soundtrack();
        let mut conn = library(&rows);
        let dir = Path::new(&rows[0].path).parent().unwrap();
        settle_folders(&mut conn, [dir]).unwrap();

        // The editor types the value the rule already gave every row.
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for id in &ids {
            store::apply_changes(
                &conn,
                *id,
                &[crate::writer::Change {
                    field: crate::writer::Field::AlbumArtist,
                    value: Some(VARIOUS_ARTISTS.into()),
                }],
            )
            .unwrap();
        }

        // Down to one artist, which would release settled rows.
        conn.execute("UPDATE tracks SET artist = 'Miley Cyrus'", [])
            .unwrap();
        assert_eq!(settle_folders(&mut conn, [dir]).unwrap(), 0);
        assert_eq!(distinct_albums(&conn), 1);
    }

    #[test]
    fn the_marked_rung_queues_unmarked_settles_for_a_reread() {
        let mut settled = borrowed("/m/ost/01.flac", "Hannah Montana", "The Movie");
        settled.album_artist = VARIOUS_ARTISTS.into();
        settled.mtime = 5;
        let mut plain = borrowed("/m/a/01.flac", "Artist A", "Record");
        plain.mtime = 5;
        let conn = before_marked_rung(&[settled, plain]);

        store::init_schema(&conn).unwrap();
        let mtimes: Vec<i64> = conn
            .prepare("SELECT mtime FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(mtimes, [5, 0]);
    }

    #[test]
    fn a_folder_settle_leaves_its_subfolders_alone() {
        let mut conn = library(&[
            borrowed("/m/mix/01.flac", "Queen", "Mix"),
            borrowed("/m/mix/cd2/01.flac", "ABBA", "Mix"),
            borrowed("/m/mix/cd2/02.flac", "Blur", "Mix"),
        ]);

        assert_eq!(settle_folders(&mut conn, [Path::new("/m/mix")]).unwrap(), 0);
        assert_eq!(settle_all(&conn).unwrap(), 2);
    }
}
