//! Moving an Unknown row's listens ([`rox_library::unknown`]) onto the track
//! that turns out to be the song: a local copy a scan found ([`relink`]), or
//! the row a plugin search played ([`adopt`]).

use gpui::{App, Entity, Task};

use rox_library::cue::TrackKey;
use rox_library::rusqlite::{self, Connection};
use rox_library::{listens, store};

use crate::catalog::Library;
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

/// After a plugin search found the song: resolves `key` to its row and
/// hands the Unknown row's listens to it on a background connection, then
/// has the Library re-read plays so open history views refresh.
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

        rox_library::unknown::adopt(&mut conn, unknown_id, track_id).map_err(|e| e.to_string())
    });

    cx.spawn(async move |cx| {
        let moved = moved.await?;
        cx.update(|cx| library.update(cx, |library, cx| library.reload_plays(cx)))
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
