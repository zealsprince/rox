//! Scrobbles that come back from Last.fm as plays rox recorded itself. Rox
//! sends the second it filed the listen at as the scrobble's timestamp, so
//! an echo sits on exactly that second and names the same song. For a radio
//! listen that's the song the station had on, which is what the listen's
//! snapshot holds.
//!
//! The song check matters: different songs share a second often enough
//! across two imported accounts that the time alone would eat real plays.

use rox_library::listens::{ORIGIN_LOCAL, ORIGIN_SCROBBLE};
use rox_library::rusqlite::{self, Connection, params};

use crate::names;

/// Whether rox already holds this scrobble as a listen of its own.
pub fn is_echo(
    conn: &Connection,
    artist: &str,
    title: &str,
    played_at: i64,
) -> rusqlite::Result<bool> {
    let mut stmt = conn
        .prepare_cached("SELECT artist, title FROM listens WHERE played_at = ?1 AND origin = ?2")?;
    let mut rows = stmt.query(params![played_at, ORIGIN_LOCAL])?;

    while let Some(row) = rows.next()? {
        let (own_artist, own_title): (String, String) = (row.get(0)?, row.get(1)?);
        if names::same_song(artist, title, &own_artist, &own_title) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Deletes the imported listens that echo one of rox's own, then the
/// Unknown rows left with none. Answers the listens deleted.
pub fn prune(conn: &mut Connection) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;

    let echoes: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT s.id, s.artist, s.title, o.artist, o.title
               FROM listens s JOIN listens o
                 ON o.played_at = s.played_at AND o.origin = ?1
              WHERE s.origin = ?2",
        )?;
        let pairs = stmt.query_map(params![ORIGIN_LOCAL, ORIGIN_SCROBBLE], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        let mut echoes = Vec::new();
        for pair in pairs {
            let (id, artist, title, own_artist, own_title) = pair?;
            if names::same_song(&artist, &title, &own_artist, &own_title) {
                echoes.push(id);
            }
        }
        echoes.sort_unstable();
        echoes.dedup();
        echoes
    };

    {
        let mut delete = tx.prepare("DELETE FROM listens WHERE id = ?1")?;
        for id in &echoes {
            delete.execute([id])?;
        }
    }
    rox_library::unknown::prune(&tx)?;
    tx.commit()?;

    Ok(echoes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::store;

    fn listen(conn: &Connection, track_id: i64, at: i64, artist: &str, title: &str, origin: &str) {
        conn.execute(
            "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path, origin)
             VALUES (?1, ?2, ?3, ?4, '', '', '', ?5)",
            params![track_id, at, title, artist, origin],
        )
        .unwrap();
    }

    #[test]
    fn an_echo_goes_and_a_song_sharing_its_second_stays() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();

        // Rox's own play of a Tidal track, and Last.fm handing it back with
        // its own casing, unmatched.
        listen(&conn, 7, 100, "KATSEYE", "Hootie Frutti", ORIGIN_LOCAL);
        let echo = rox_library::unknown::row(&conn, "KATSEYE", "HOOTIE FRUTTI", "").unwrap();
        listen(
            &conn,
            echo,
            100,
            "KATSEYE",
            "HOOTIE FRUTTI",
            ORIGIN_SCROBBLE,
        );

        // Another account's scrobble of a different song on the same second.
        listen(&conn, 8, 200, "Muse", "Mercy", ORIGIN_LOCAL);
        let other = rox_library::unknown::row(&conn, "Incubus", "Absolution Calling", "").unwrap();
        listen(
            &conn,
            other,
            200,
            "Incubus",
            "Absolution Calling",
            ORIGIN_SCROBBLE,
        );

        assert!(is_echo(&conn, "Katseye", "hootie frutti", 100).unwrap());
        assert!(!is_echo(&conn, "Incubus", "Absolution Calling", 200).unwrap());
        assert!(!is_echo(&conn, "KATSEYE", "Hootie Frutti", 101).unwrap());

        assert_eq!(prune(&mut conn).unwrap(), 1);
        assert_eq!(prune(&mut conn).unwrap(), 0, "a second pass finds nothing");

        let unknown = rox_library::unknown::all(&conn).unwrap();
        assert_eq!(
            unknown.iter().map(|(id, ..)| *id).collect::<Vec<_>>(),
            [other],
            "the emptied Unknown row goes with its echo"
        );
    }
}
