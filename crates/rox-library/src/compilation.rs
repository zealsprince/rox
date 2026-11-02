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
//! keeps the group as tagged.

use std::path::Path;

use rusqlite::{Connection, params};

/// The album artist a settled album takes. MusicBrainz's name for a
/// compilation, so an album tagged that way and one settled here group alike.
pub const VARIOUS_ARTISTS: &str = "Various Artists";

/// A row's folder with its trailing separator: rtrim strips every character
/// that isn't a separator back to the last one. The stats rollup uses the
/// same expression.
const FOLDER: &str = "rtrim(path, replace(replace(path, '/', ''), '\\', ''))";

/// The settle over rows matching `scope`, an extra `AND` clause on both the
/// group and the update. ?1 is always [`VARIOUS_ARTISTS`].
///
/// A row counts as borrowed when its album artist is its own artist or
/// already the settled value; anything else came from a tag and blocks the
/// group. Artists compare without ASCII case, so "Abba" and "ABBA" stay one
/// artist.
fn settle_sql(scope: &str) -> String {
    format!(
        "UPDATE tracks SET album_artist = ?1, album_artist_sort = ''
          WHERE source = 'local' AND album_artist <> ?1 {scope}
            AND ({FOLDER}, album) IN (
                SELECT {FOLDER}, album FROM tracks
                 WHERE source = 'local' AND album <> '' {scope}
                 GROUP BY 1, 2
                HAVING COUNT(DISTINCT artist COLLATE NOCASE) > 1
                   AND SUM(album_artist <> artist AND album_artist <> ?1) = 0)"
    )
}

/// Settle every local album in the library. The migration's pass over rows
/// scanned before the rule existed.
pub(crate) fn settle_all(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(&settle_sql(""), params![VARIOUS_ARTISTS])
}

/// Settle the albums directly in these folders. Reads each folder whole from
/// the store, so rows the scan didn't re-read still count toward the group.
pub fn settle_folders<'a>(
    conn: &mut Connection,
    dirs: impl IntoIterator<Item = &'a Path>,
) -> rusqlite::Result<usize> {
    // The path range lets the (source, path) index find the folder; the
    // FOLDER match drops its subfolders.
    let sql = settle_sql(&format!("AND path >= ?2 AND path < ?3 AND {FOLDER} = ?2"));
    let tx = conn.transaction()?;
    let mut settled = 0;
    {
        let mut stmt = tx.prepare_cached(&sql)?;
        for dir in dirs {
            let (lo, hi) = crate::store::path_range(dir);
            settled += stmt.execute(params![VARIOUS_ARTISTS, lo, hi])?;
        }
    }
    tx.commit()?;
    Ok(settled)
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

    #[test]
    fn the_migration_settles_a_library_scanned_before_the_rule() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::run_ladder_before(&conn, "folder-compilations").unwrap();
        store::insert_batch(&mut conn, &soundtrack()).unwrap();
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
