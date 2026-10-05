//! Unknown rows: songs the play history knows by name that the library
//! doesn't hold. A Last.fm scrobble matching no track lands on one, so its
//! listen keeps a row to point at. One row per song in `tracks` under
//! [`SOURCE`], keyed by the folded artist and title, so case and accent
//! variants share it (ADR 11, amended 2026-10-04).
//!
//! A love the loved-tracks import can't match lands on one too, as a
//! favourite.
//!
//! An Unknown row never plays and never browses or searches. Only the
//! history and the favourites read it. When a copy turns up, [`adopt`] hands
//! its listens and its heart over and drops it.

use rusqlite::{Connection, OptionalExtension, params};

pub const SOURCE: &str = "unknown";

/// The row's path: folded artist and title around a unit separator, which no
/// tag value carries.
pub fn key(artist: &str, title: &str) -> String {
    format!(
        "{}\u{1f}{}",
        crate::fold::fold(artist.trim()),
        crate::fold::fold(title.trim())
    )
}

/// Upserts the Unknown row for this song and answers its track id. Idempotent
/// per song: the first call's names stay the display names.
pub fn row(conn: &Connection, artist: &str, title: &str, album: &str) -> rusqlite::Result<i64> {
    let path = key(artist, title);

    conn.prepare_cached(
        "INSERT INTO tracks
            (source, path, sub, title, artist, album_artist, album, genre, year,
             track_no, duration_ms, size, mtime, added)
         VALUES (?1, ?2, 0, ?3, ?4, ?4, ?5, '', 0, 0, 0, 0, 0, ?6)
         ON CONFLICT DO NOTHING",
    )?
    .execute(params![
        SOURCE,
        path,
        title.trim(),
        artist.trim(),
        album.trim(),
        now_secs()
    ])?;

    conn.prepare_cached("SELECT id FROM tracks WHERE source = ?1 AND path = ?2 AND sub = 0")?
        .query_row(params![SOURCE, path], |row| row.get(0))
}

/// Whether the library holds any Unknown row. Rides the (source, path, sub)
/// index, so it's the cheap gate in front of a relink.
pub fn any(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM tracks WHERE source = ?1)",
        [SOURCE],
        |row| row.get(0),
    )
}

/// Every Unknown row as (id, artist, title).
pub fn all(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare("SELECT id, artist, title FROM tracks WHERE source = ?1")?;
    let rows = stmt.query_map([SOURCE], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

/// Every hearted Unknown row as (id, artist, title). Driven from the
/// favourites' members, which are far fewer than the Unknown rows.
pub fn hearted(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.id, t.artist, t.title FROM playlist_tracks m
           JOIN playlists p ON p.id = m.playlist_id AND p.favourite = 1
           JOIN tracks t ON t.id = m.track_id AND t.source = ?1",
    )?;
    let rows = stmt.query_map([SOURCE], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

/// Deletes the Unknown rows that no listen names and no favourite holds.
/// Answers how many went.
pub fn prune(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM tracks WHERE source = ?1
           AND NOT EXISTS (SELECT 1 FROM listens l WHERE l.track_id = tracks.id)
           AND NOT EXISTS (SELECT 1 FROM playlist_tracks m WHERE m.track_id = tracks.id)",
        [SOURCE],
    )
}

/// Moves everything that references the Unknown row onto `track_id`, then
/// deletes the row, in one transaction. Answers the listens moved.
///
/// Listens take the target's source and path snapshot the way
/// [`crate::listens::reattach`] refreshes it; their tag snapshot stays, since
/// that's what was heard. A listen the target already holds at the same
/// second is the same play arriving twice (rox recorded it and Last.fm
/// handed it back unmatched), so it goes. A heart follows, deduped against
/// one the target already has. Bookmarks follow too, though nothing should
/// have filed one against an Unknown row.
///
/// Zero, touching nothing, when `unknown_id` isn't an Unknown row (a second
/// adopt of the same song) or `track_id` isn't a real one.
pub fn adopt(conn: &mut Connection, unknown_id: i64, track_id: i64) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;

    let source_of = |id: i64| {
        tx.query_row("SELECT source FROM tracks WHERE id = ?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .optional()
    };
    let from_unknown = source_of(unknown_id)?.is_some_and(|source| source == SOURCE);
    let onto_real = source_of(track_id)?.is_some_and(|source| source != SOURCE);
    if !from_unknown || !onto_real {
        return Ok(0);
    }

    tx.execute(
        "DELETE FROM listens WHERE track_id = ?1
           AND EXISTS (SELECT 1 FROM listens h
                        WHERE h.track_id = ?2 AND h.played_at = listens.played_at)",
        params![unknown_id, track_id],
    )?;
    let moved = tx.execute(
        "UPDATE listens SET track_id = t.id, source = t.source, path =
             CASE WHEN t.sub = 0 THEN t.path ELSE t.path || '#' || t.sub END
         FROM tracks t
         WHERE t.id = ?2 AND listens.track_id = ?1",
        params![unknown_id, track_id],
    )?;

    let members = tx.execute(
        "UPDATE playlist_tracks SET track_id = t.id, source = t.source, path = t.path
         FROM tracks t
         WHERE t.id = ?2 AND playlist_tracks.track_id = ?1",
        params![unknown_id, track_id],
    )?;
    if members > 0 {
        crate::playlists::dedupe_favourites(&tx, now_secs())?;
    }
    let marks = tx.execute(
        "UPDATE bookmarks SET track_id = ?2 WHERE track_id = ?1",
        params![unknown_id, track_id],
    )?;
    if marks > 0 {
        crate::bookmarks::reattach(&tx)?;
    }

    crate::track_meta::clear(&tx, unknown_id)?;
    tx.execute(
        "DELETE FROM tracks WHERE id = ?1 AND source = ?2",
        params![unknown_id, SOURCE],
    )?;
    tx.commit()?;

    Ok(moved)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listens::{self, Listen};
    use crate::projection::Projection;
    use crate::{TrackRow, store};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        conn
    }

    fn file(path: &str, artist: &str, title: &str) -> TrackRow {
        TrackRow {
            path: path.into(),
            sub: 0,
            cue: None,
            remote_url: String::new(),
            remote_live: false,
            title: title.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: "Album".into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 1,
            duration_ms: 1000,
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

    fn local_id(conn: &Connection, path: &str) -> i64 {
        store::id_for_path(conn, crate::cue::LOCAL, path)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn one_row_per_song_whatever_the_case_or_accents() {
        let conn = db();
        let id = row(&conn, "Beyoncé", "Halo", "I Am... Sasha Fierce").unwrap();

        assert_eq!(row(&conn, "beyonce", " HALO ", "").unwrap(), id);
        assert_eq!(row(&conn, "Beyoncé", "Halo", "Other").unwrap(), id);
        assert_ne!(row(&conn, "Beyoncé", "Hello", "").unwrap(), id);

        let (title, artist, album, source): (String, String, String, String) = conn
            .query_row(
                "SELECT title, artist, album, source FROM tracks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (title.as_str(), artist.as_str(), album.as_str()),
            ("Halo", "Beyoncé", "I Am... Sasha Fierce"),
            "the first scrobble's names stay"
        );
        assert_eq!(source, SOURCE);
        assert_eq!(crate::cue::Origin::of(&source), crate::cue::Origin::Unknown);
    }

    #[test]
    fn clearing_the_imported_listens_takes_their_rows_along() {
        let mut conn = db();
        let id = row(&conn, "Air", "Sexy Boy", "Moon Safari").unwrap();
        listens::import_scrobbles(&mut conn, &[(id, 100)], |_, _| true).unwrap();
        assert!(any(&conn).unwrap());

        listens::clear(&conn, listens::Clear::Imported).unwrap();
        assert!(!any(&conn).unwrap(), "no row is left holding nothing");
    }

    #[test]
    fn imported_scrobbles_land_on_the_row_and_rerun_idempotently() {
        let mut conn = db();
        let id = row(&conn, "Aphex Twin", "Xtal", "Selected Ambient Works").unwrap();
        let plays = [(id, 100), (id, 200)];

        assert_eq!(
            listens::import_scrobbles(&mut conn, &plays, |_, _| true).unwrap(),
            2
        );
        assert_eq!(
            listens::import_scrobbles(&mut conn, &plays, |_, _| true).unwrap(),
            0
        );

        let recent = listens::recent(&conn, 0, i64::MAX, 10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].title, "Xtal");
        assert_eq!(recent[0].artist, "Aphex Twin");
        assert_eq!(recent[0].album, "Selected Ambient Works");
        assert_eq!(recent[0].source, SOURCE);
        assert_eq!(recent[0].path, key("Aphex Twin", "Xtal"));

        let snapshot: (String, String, String) = conn
            .query_row(
                "SELECT source, path, origin FROM listens LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            snapshot,
            (
                SOURCE.to_string(),
                key("Aphex Twin", "Xtal"),
                listens::ORIGIN_SCROBBLE.to_string()
            )
        );
    }

    #[test]
    fn adopt_moves_the_listens_and_drops_the_row() {
        let mut conn = db();
        let id = row(&conn, "Air", "Sexy Boy", "Moon Safari").unwrap();
        listens::import_scrobbles(&mut conn, &[(id, 100), (id, 200), (id, 300)], |_, _| true)
            .unwrap();

        store::insert_batch(&mut conn, &[file("/m/air.flac", "Air", "Sexy Boy")]).unwrap();
        let local = local_id(&conn, "/m/air.flac");
        // rox heard the second play itself; Last.fm handed it back unmatched.
        listens::append(
            &conn,
            &Listen {
                track_id: local,
                played_at: 200,
                title: "Sexy Boy".into(),
                artist: "Air".into(),
                album: "Album".into(),
                genre: String::new(),
                path: "/m/air.flac".into(),
            },
        )
        .unwrap();
        crate::bookmarks::add(&conn, id, &key("Air", "Sexy Boy"), 5_000, "", None).unwrap();

        assert_eq!(
            adopt(&mut conn, id, local).unwrap(),
            2,
            "the duplicate second goes"
        );

        let gone: Option<i64> = conn
            .query_row("SELECT id FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .optional()
            .unwrap();
        assert_eq!(gone, None);

        let rows: Vec<(i64, i64, String, String, String)> = conn
            .prepare(
                "SELECT track_id, played_at, source, path, album FROM listens ORDER BY played_at",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows.iter().map(|r| (r.0, r.1)).collect::<Vec<_>>(),
            [(local, 100), (local, 200), (local, 300)]
        );
        for (_, _, source, path, _) in &rows {
            assert_eq!((source.as_str(), path.as_str()), ("local", "/m/air.flac"));
        }
        assert_eq!(
            rows[0].4, "Moon Safari",
            "the tag snapshot is what was heard"
        );

        let marks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bookmarks WHERE track_id = ?1 AND path = '/m/air.flac'",
                [local],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marks, 1);
        assert_eq!(listens::reattach(&conn).unwrap(), None, "nothing dangles");
    }

    #[test]
    fn adopt_onto_a_plugin_row_snapshots_its_key() {
        let mut conn = db();
        let id = row(&conn, "Air", "Sexy Boy", "").unwrap();
        listens::import_scrobbles(&mut conn, &[(id, 100)], |_, _| true).unwrap();

        let mut picked = file("track-42", "Air", "Sexy Boy");
        picked.sub = 0;
        store::upsert_source_rows(&mut conn, "plugin:demo", &[picked]).unwrap();
        let plugin = store::id_for_path(&conn, "plugin:demo", "track-42")
            .unwrap()
            .unwrap();

        assert_eq!(adopt(&mut conn, id, plugin).unwrap(), 1);
        let (track_id, source, path): (i64, String, String) = conn
            .query_row("SELECT track_id, source, path FROM listens", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(
            (track_id, source.as_str(), path.as_str()),
            (plugin, "plugin:demo", "track-42")
        );
    }

    #[test]
    fn adopt_refuses_anything_but_an_unknown_row_onto_a_real_one() {
        let mut conn = db();
        store::insert_batch(
            &mut conn,
            &[file("/m/a.flac", "A", "One"), file("/m/b.flac", "B", "Two")],
        )
        .unwrap();
        let (a, b) = (local_id(&conn, "/m/a.flac"), local_id(&conn, "/m/b.flac"));
        let id = row(&conn, "C", "Three", "").unwrap();
        listens::import_scrobbles(&mut conn, &[(a, 100), (id, 200)], |_, _| true).unwrap();

        assert_eq!(
            adopt(&mut conn, a, b).unwrap(),
            0,
            "two real tracks never merge"
        );
        assert_eq!(adopt(&mut conn, id, 9_999).unwrap(), 0, "no such target");
        assert_eq!(adopt(&mut conn, id, id).unwrap(), 0);
        assert_eq!(store::count(&conn).unwrap(), 3, "and nothing was deleted");

        assert_eq!(adopt(&mut conn, id, b).unwrap(), 1);
        assert_eq!(
            adopt(&mut conn, id, b).unwrap(),
            0,
            "a second adopt is a no-op"
        );
    }

    #[test]
    fn only_favourites_take_an_unknown_row() {
        let mut conn = db();
        let id = row(&conn, "A", "One", "").unwrap();
        let list = crate::playlists::create(&conn, "Mix", 0).unwrap();

        assert!(
            crate::playlists::add(&mut conn, list, &[id], 0)
                .unwrap()
                .is_empty()
        );
        crate::playlists::set_favourite(&mut conn, id, true, 0).unwrap();
        assert!(crate::playlists::is_favourite(&conn, id).unwrap());
    }

    #[test]
    fn adopt_carries_the_heart_and_keeps_one() {
        let mut conn = db();
        let id = row(&conn, "Air", "Sexy Boy", "").unwrap();
        crate::playlists::set_favourite(&mut conn, id, true, 0).unwrap();
        store::insert_batch(&mut conn, &[file("/m/air.flac", "Air", "Sexy Boy")]).unwrap();
        let local = local_id(&conn, "/m/air.flac");

        assert_eq!(
            adopt(&mut conn, id, local).unwrap(),
            0,
            "no listens to move"
        );
        assert_eq!(
            crate::playlists::favourite_track_ids(&conn).unwrap(),
            [local]
        );

        // A copy hearted on its own already: the two hearts become one.
        let again = row(&conn, "Air", "Sexy Boy", "").unwrap();
        crate::playlists::set_favourite(&mut conn, again, true, 0).unwrap();
        adopt(&mut conn, again, local).unwrap();
        assert_eq!(
            crate::playlists::favourite_track_ids(&conn).unwrap(),
            [local]
        );
    }

    #[test]
    fn a_hearted_unknown_row_survives_the_prune() {
        let mut conn = db();
        let hearted = row(&conn, "A", "One", "").unwrap();
        let bare = row(&conn, "B", "Two", "").unwrap();
        crate::playlists::set_favourite(&mut conn, hearted, true, 0).unwrap();

        assert_eq!(prune(&conn).unwrap(), 1);
        assert!(store::key_for_id(&conn, hearted).unwrap().is_some());
        assert!(store::key_for_id(&conn, bare).unwrap().is_none());
    }

    #[test]
    fn no_prune_reaches_an_unknown_row() {
        let mut conn = db();
        let id = row(&conn, "A", "One", "").unwrap();
        listens::import_scrobbles(&mut conn, &[(id, 100)], |_, _| true).unwrap();

        // An empty walk of the root the folded key would sort under.
        store::prune_missing(&mut conn, std::path::Path::new("a"), &[]).unwrap();
        store::remove_under(&conn, std::path::Path::new("a")).unwrap();
        listens::reattach(&conn).unwrap();
        assert!(crate::members::remove_source(&mut conn, SOURCE).is_err());
        assert!(crate::members::remove_track(&mut conn, SOURCE, &key("A", "One")).is_err());

        assert!(any(&conn).unwrap());
        assert_eq!(listens::counts(&conn).unwrap().get(&id), Some(&1));
    }

    #[test]
    fn an_unknown_row_never_reaches_the_projection() {
        let mut conn = db();
        store::insert_batch(&mut conn, &[file("/m/a.flac", "Air", "Sexy Boy")]).unwrap();
        let id = row(&conn, "Air", "Kelly Watch the Stars", "Moon Safari").unwrap();

        let p = Projection::load_serial(&conn, false).unwrap();
        assert_eq!(p.len(), 1, "only the file loads");
        assert!(p.search_all("kelly").is_empty());
        assert_eq!(p.search_all("air").len(), 1);
        assert!(p.browse_sources().all(|source| source != SOURCE));

        let shard = crate::projection::shard_for_ids(&conn, &[id], false).unwrap();
        assert!(shard.ids().is_empty(), "nor does a patch bring one in");
    }

    #[test]
    fn the_library_rollup_leaves_unknown_rows_out() {
        let mut conn = db();
        store::insert_batch(&mut conn, &[file("/m/a.flac", "A", "One")]).unwrap();
        row(&conn, "B", "Two", "Elsewhere").unwrap();

        assert_eq!(store::stats(&conn).unwrap().tracks, 1);
        assert_eq!(store::stats(&conn).unwrap().albums, 1);
    }
}
