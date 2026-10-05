//! Moving an Unknown row's listens and heart ([`rox_library::unknown`]) onto
//! the track that turns out to be the song: a local copy a scan found
//! ([`relink`]), a plugin track the library holds ([`relink_hearts`]), a
//! plugin track that just played ([`claim`]), or the row a plugin search
//! played ([`adopt`]).

use gpui::{App, Entity, Task};

use rox_library::cue::{PLUGIN_PREFIX, TrackKey};
use rox_library::rusqlite::{self, Connection};
use rox_library::{listens, members, playlists, store};

use crate::catalog::{Library, LibraryEvent};
use crate::names::{self, Index};

/// Moves the listens of every Unknown row whose song now has a local copy
/// onto that copy, ties going to the copy already played most. Answers
/// listens moved. Blocking.
pub fn relink(conn: &mut Connection) -> rusqlite::Result<usize> {
    // Runs after every scan and reindex, and most libraries hold no Unknown
    // row at all, so one indexed probe gates the matcher.
    if !rox_library::unknown::any(conn)? {
        return Ok(0);
    }

    let songs = rox_library::unknown::all(conn)?;
    let index = Index::build(store::name_index(conn)?);
    let plays = listens::counts(conn)?;

    let mut moved = 0;
    for (unknown_id, artist, title) in songs {
        if let Some(track_id) = names::pick_one(&index.resolve(&artist, &title), &plays) {
            moved += rox_library::unknown::adopt(conn, unknown_id, track_id)?;
        }
    }
    Ok(moved)
}

/// Hands every hearted Unknown row to a plugin track the library holds for
/// that song, synced, saved or picked, ties going to the one played most.
/// After a scan it runs behind [`relink`], so a local copy wins. Answers
/// listens moved. Blocking.
pub fn relink_hearts(conn: &mut Connection) -> rusqlite::Result<usize> {
    // Runs on every load. The hearted rows are a handful, so they gate
    // building the plugin index.
    let songs = rox_library::unknown::hearted(conn)?;
    if songs.is_empty() {
        return Ok(0);
    }

    let index = Index::build(members::plugin_name_index(conn)?);
    let plays = listens::counts(conn)?;

    let mut moved = 0;
    for (unknown_id, artist, title) in songs {
        if let Some(track_id) = names::pick_one(&index.resolve(&artist, &title), &plays) {
            moved += rox_library::unknown::adopt(conn, unknown_id, track_id)?;
        }
    }
    Ok(moved)
}

/// A listen just landed on `track_id`. When that's a plugin track, every
/// hearted Unknown row naming its song hands over to it: playing it is the
/// user choosing this copy. Answers whether any did. Blocking.
pub fn claim(conn: &mut Connection, track_id: i64) -> rusqlite::Result<bool> {
    let plugin =
        store::key_for_id(conn, track_id)?.is_some_and(|key| key.source.starts_with(PLUGIN_PREFIX));
    if !plugin {
        return Ok(false);
    }

    let songs = rox_library::unknown::hearted(conn)?;
    if songs.is_empty() {
        return Ok(false);
    }

    let (artist, album_artist, title): (String, String, String) = conn.query_row(
        "SELECT artist, album_artist, title FROM tracks WHERE id = ?1",
        [track_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // The album artist is a second name, as the library pass files it.
    let mut claimed = false;
    for (unknown_id, loved_artist, loved_title) in songs {
        let named = |by: &str| names::same_song(&loved_artist, &loved_title, by, &title);
        if named(&artist) || (!album_artist.is_empty() && named(&album_artist)) {
            rox_library::unknown::adopt(conn, unknown_id, track_id)?;
            claimed = true;
        }
    }
    Ok(claimed)
}

/// After a plugin search found the song: resolves `key` to its row and
/// hands the Unknown row's listens and heart to it on a background
/// connection, then has the Library re-read plays so open history views
/// refresh.
pub fn adopt(
    library: Entity<Library>,
    unknown_id: i64,
    key: TrackKey,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    let db_path = library.read(cx).db_path();
    let moved = cx.background_executor().spawn(async move {
        let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;
        let path = key
            .path
            .to_str()
            .ok_or_else(|| format!("unreadable track key {}", key.path.display()))?;
        let track_id = store::queue_meta_for_key(&conn, &key.source, path, key.sub)
            .map_err(|e| e.to_string())?
            .id
            .ok_or_else(|| format!("{}|{path} isn't in the library", key.source))?;

        let hearted = playlists::is_favourite(&conn, unknown_id).map_err(|e| e.to_string())?;
        let moved = rox_library::unknown::adopt(&mut conn, unknown_id, track_id)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((moved, hearted))
    });

    cx.spawn(async move |cx| {
        let (moved, hearted) = moved.await?;
        cx.update(|cx| {
            library.update(cx, |library, cx| {
                library.reload_plays(cx);
                // The heart changed rows behind the Library's back. The
                // favourites mirrors diff on this, so Sync Favourites sends
                // the plugin row's add now.
                if hearted {
                    cx.emit(LibraryEvent::PlaylistsChanged);
                }
            })
        })
        .map_err(|e| e.to_string())?;
        Ok(moved)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::TrackRow;

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

    fn track_ids(conn: &Connection) -> Vec<i64> {
        conn.prepare("SELECT track_id FROM listens ORDER BY played_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn relink_moves_listens_onto_a_local_copy_once_one_exists() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let roygbiv = rox_library::unknown::row(&conn, "Boards of Canada", "Roygbiv", "").unwrap();
        let xtal = rox_library::unknown::row(&conn, "Aphex Twin", "Xtal", "").unwrap();
        listens::import_scrobbles(
            &mut conn,
            &[(roygbiv, 100), (xtal, 200), (roygbiv, 300)],
            |_, _| true,
        )
        .unwrap();

        assert_eq!(relink(&mut conn).unwrap(), 0, "no copy yet");

        // Two copies of the song: the one already played takes the history.
        store::insert_batch(
            &mut conn,
            &[
                file("/m/a/roygbiv.flac", "Boards of Canada", "Roygbiv"),
                file("/m/b/roygbiv.flac", "boards of canada", "ROYGBIV"),
            ],
        )
        .unwrap();
        let played = store::id_for_path(&conn, "local", "/m/b/roygbiv.flac")
            .unwrap()
            .unwrap();
        let listen = listens::listen_for_path(&conn, "/m/b/roygbiv.flac", 50)
            .unwrap()
            .unwrap();
        listens::append(&conn, &listen).unwrap();

        assert_eq!(relink(&mut conn).unwrap(), 2);
        assert_eq!(track_ids(&conn), [played, played, xtal, played]);
        assert!(
            rox_library::unknown::any(&conn).unwrap(),
            "Xtal is still unknown"
        );
        assert_eq!(relink(&mut conn).unwrap(), 0, "a rerun finds nothing new");
    }

    fn song(key: &str, artist: &str, title: &str) -> members::PluginTrack {
        members::PluginTrack {
            key: key.into(),
            title: title.into(),
            artist: artist.into(),
            ..Default::default()
        }
    }

    fn hearted_unknown(conn: &mut Connection, artist: &str, title: &str) -> i64 {
        let id = rox_library::unknown::row(conn, artist, title, "").unwrap();
        playlists::set_favourite(conn, id, true, 0).unwrap();
        id
    }

    fn plugin_id(conn: &Connection, key: &str) -> i64 {
        store::id_for_path(conn, "plugin:demo", key)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn relink_hearts_takes_any_plugin_track_and_only_hearts() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        hearted_unknown(&mut conn, "Lemaitre", "Trip Sitter");
        hearted_unknown(&mut conn, "Boards of Canada", "Roygbiv");
        // Scrobbled, never loved: the plugin pass is only for hearts.
        let scrobbled = rox_library::unknown::row(&conn, "Aphex Twin", "Xtal", "").unwrap();

        members::pick(
            &mut conn,
            "plugin:demo",
            &[song("trip", "Lemaitre, Sofiloud", "Trip Sitter")],
        )
        .unwrap();
        members::save(
            &mut conn,
            "plugin:demo",
            &[song("xtal", "Aphex Twin", "Xtal")],
        )
        .unwrap();
        members::set_collection(
            &mut conn,
            "plugin:demo",
            "liked",
            &[song("boc", "boards of canada", "ROYGBIV")],
        )
        .unwrap();

        relink_hearts(&mut conn).unwrap();
        assert_eq!(
            playlists::favourite_track_ids(&conn).unwrap().len(),
            2,
            "no heart doubled or lost"
        );
        assert!(playlists::is_favourite(&conn, plugin_id(&conn, "boc")).unwrap());
        assert!(
            playlists::is_favourite(&conn, plugin_id(&conn, "trip")).unwrap(),
            "a pick under its whole credit list still takes the lead's heart"
        );
        assert!(store::key_for_id(&conn, scrobbled).unwrap().is_some());
        assert!(rox_library::unknown::hearted(&conn).unwrap().is_empty());
    }

    #[test]
    fn claim_hands_a_heart_to_the_plugin_track_that_played() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        hearted_unknown(&mut conn, "Air", "Sexy Boy");
        store::insert_batch(&mut conn, &[file("/m/other.flac", "Air", "Sexy Boy")]).unwrap();
        let local = store::id_for_path(&conn, "local", "/m/other.flac")
            .unwrap()
            .unwrap();
        members::pick(
            &mut conn,
            "plugin:demo",
            &[song("air", "Air", "Sexy Boy (Remastered)")],
        )
        .unwrap();
        let picked = plugin_id(&conn, "air");

        assert!(
            !claim(&mut conn, local).unwrap(),
            "only a plugin play claims"
        );
        assert!(claim(&mut conn, picked).unwrap());
        assert_eq!(playlists::favourite_track_ids(&conn).unwrap(), [picked]);
        assert!(!claim(&mut conn, picked).unwrap(), "nothing left to claim");
    }

    #[test]
    fn relink_finds_a_copy_filed_with_its_guest_in_the_artist_tag() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let id =
            rox_library::unknown::row(&conn, "Lord Huron", "I Lied (with Allison Ponthier)", "")
                .unwrap();
        listens::import_scrobbles(&mut conn, &[(id, 100)], |_, _| true).unwrap();

        let mut copy = file("/m/i-lied.flac", "Lord Huron, Allison Ponthier", "I Lied");
        copy.album_artist = "Lord Huron".into();
        store::insert_batch(&mut conn, &[copy]).unwrap();
        let local = store::id_for_path(&conn, "local", "/m/i-lied.flac")
            .unwrap()
            .unwrap();

        assert_eq!(relink(&mut conn).unwrap(), 1);
        assert_eq!(track_ids(&conn), [local]);
    }
}
