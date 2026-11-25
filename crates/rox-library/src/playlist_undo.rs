//! Undo for playlist edits: what each edit changed in the member rows and
//! names, and how to take it back. A step holds only the rows an edit
//! touched, never a copy of the whole list.
//!
//! Rows come back whole, member ids included, so a step never holds an id
//! that a re-add renumbered. A step applies only to the rows it expects: an
//! edit made outside the stack (a heart toggle, a sync, a deleted playlist)
//! leaves it stale, and [`apply`] refuses it and changes nothing.

use rusqlite::{Connection, OptionalExtension, Transaction};

/// A member row whole, as an undo puts it back.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberRow {
    pub id: i64,
    pub playlist_id: i64,
    pub track_id: i64,
    pub position: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub path: String,
    pub source: String,
}

/// Where a member stands: its playlist and position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub member: i64,
    pub playlist_id: i64,
    pub position: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Put these rows back exactly. Refused when an id is taken.
    Insert(Vec<MemberRow>),
    /// Refused when a member is gone.
    Delete(Vec<i64>),
    /// Refused when a member is gone.
    Place(Vec<Placement>),
    Rename {
        playlist_id: i64,
        name: String,
    },
}

impl Change {
    fn playlists(&self) -> Vec<i64> {
        match self {
            Change::Insert(rows) => rows.iter().map(|row| row.playlist_id).collect(),
            Change::Delete(_) => Vec::new(),
            Change::Place(places) => places.iter().map(|place| place.playlist_id).collect(),
            Change::Rename { playlist_id, .. } => vec![*playlist_id],
        }
    }
}

/// One undo step: changes applied in order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Step {
    changes: Vec<Change>,
}

impl Step {
    pub fn new(changes: Vec<Change>) -> Step {
        Step { changes }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// One step that applies `self`, then `older`.
    pub fn then(mut self, older: Step) -> Step {
        self.changes.extend(older.changes);
        self
    }

    /// Whether the step names `playlist_id`, for dropping a deleted
    /// playlist's steps. A Delete names rows, not playlists, and goes stale
    /// on its own.
    pub fn touches(&self, playlist_id: i64) -> bool {
        self.changes
            .iter()
            .any(|change| change.playlists().contains(&playlist_id))
    }
}

/// The rows of `members` that exist, in the order asked.
pub fn rows(conn: &Connection, members: &[i64]) -> rusqlite::Result<Vec<MemberRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, playlist_id, track_id, position, title, artist, album, path, source
         FROM playlist_tracks WHERE id = ?1",
    )?;
    let mut found = Vec::with_capacity(members.len());
    for &member in members {
        let row = stmt
            .query_row([member], |row| {
                Ok(MemberRow {
                    id: row.get(0)?,
                    playlist_id: row.get(1)?,
                    track_id: row.get(2)?,
                    position: row.get(3)?,
                    title: row.get(4)?,
                    artist: row.get(5)?,
                    album: row.get(6)?,
                    path: row.get(7)?,
                    source: row.get(8)?,
                })
            })
            .optional()?;
        found.extend(row);
    }
    Ok(found)
}

/// Where every member of `playlist_id` stands.
pub fn placements(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Vec<Placement>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, position FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position, id",
    )?;
    let rows = stmt.query_map([playlist_id], |row| {
        Ok(Placement {
            member: row.get(0)?,
            playlist_id,
            position: row.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn name(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT name FROM playlists WHERE id = ?1",
        [playlist_id],
        |row| row.get(0),
    )
    .optional()
}

/// Applies `step` in one transaction and returns the step that takes it
/// back, or None with nothing changed when the rows aren't the ones it was
/// made against.
pub fn apply(conn: &mut Connection, step: &Step, now: i64) -> rusqlite::Result<Option<Step>> {
    let tx = conn.transaction()?;
    let mut undo = Vec::with_capacity(step.changes.len());
    let mut touched: Vec<i64> = Vec::new();

    for change in &step.changes {
        let Some(back) = apply_change(&tx, change)? else {
            // Dropping the transaction rolls back whatever already ran.
            return Ok(None);
        };

        touched.extend(change.playlists());
        touched.extend(back.playlists());
        undo.push(back);
    }

    touched.sort_unstable();
    touched.dedup();
    {
        let mut stamp = tx.prepare("UPDATE playlists SET updated = ?2 WHERE id = ?1")?;
        for id in touched {
            stamp.execute(rusqlite::params![id, now])?;
        }
    }
    tx.commit()?;

    undo.reverse();
    Ok(Some(Step::new(undo)))
}

fn apply_change(tx: &Transaction, change: &Change) -> rusqlite::Result<Option<Change>> {
    match change {
        Change::Insert(rows) => {
            let mut taken = tx.prepare_cached("SELECT 1 FROM playlist_tracks WHERE id = ?1")?;
            let mut list = tx.prepare_cached("SELECT 1 FROM playlists WHERE id = ?1")?;
            for row in rows {
                if taken.exists([row.id])? || !list.exists([row.playlist_id])? {
                    return Ok(None);
                }
            }

            let mut insert = tx.prepare_cached(
                "INSERT INTO playlist_tracks
                    (id, playlist_id, track_id, position, title, artist, album, path, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for row in rows {
                insert.execute(rusqlite::params![
                    row.id,
                    row.playlist_id,
                    row.track_id,
                    row.position,
                    row.title,
                    row.artist,
                    row.album,
                    row.path,
                    row.source,
                ])?;
            }
            Ok(Some(Change::Delete(
                rows.iter().map(|row| row.id).collect(),
            )))
        }

        Change::Delete(members) => {
            let found = rows(tx, members)?;
            if found.len() != members.len() {
                return Ok(None);
            }

            let mut delete = tx.prepare_cached("DELETE FROM playlist_tracks WHERE id = ?1")?;
            for &member in members {
                delete.execute([member])?;
            }
            Ok(Some(Change::Insert(found)))
        }

        Change::Place(places) => {
            let members: Vec<i64> = places.iter().map(|place| place.member).collect();
            let found = rows(tx, &members)?;
            let mut list = tx.prepare_cached("SELECT 1 FROM playlists WHERE id = ?1")?;
            if found.len() != members.len() {
                return Ok(None);
            }
            for place in places {
                if !list.exists([place.playlist_id])? {
                    return Ok(None);
                }
            }

            let mut update = tx.prepare_cached(
                "UPDATE playlist_tracks SET playlist_id = ?2, position = ?3 WHERE id = ?1",
            )?;
            for place in places {
                update.execute(rusqlite::params![
                    place.member,
                    place.playlist_id,
                    place.position
                ])?;
            }
            let back = found
                .iter()
                .map(|row| Placement {
                    member: row.id,
                    playlist_id: row.playlist_id,
                    position: row.position,
                })
                .collect();
            Ok(Some(Change::Place(back)))
        }

        Change::Rename { playlist_id, name } => {
            let Some(was) = self::name(tx, *playlist_id)? else {
                return Ok(None);
            };

            tx.execute(
                "UPDATE playlists SET name = ?2 WHERE id = ?1",
                rusqlite::params![playlist_id, name],
            )?;
            Ok(Some(Change::Rename {
                playlist_id: *playlist_id,
                name: was,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackRow, playlists, store};

    fn track(path: &str, title: &str) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            sub: 0,
            cue: None,
            path: path.into(),
            title: title.into(),
            artist: "A".into(),
            album_artist: "A".into(),
            album: "First".into(),
            genre: String::new(),
            comment: String::new(),
            month: 0,
            day: 0,
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

    fn seed() -> (Connection, Vec<i64>) {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(
            &mut conn,
            &[
                track("/m/1.mp3", "One"),
                track("/m/2.mp3", "Two"),
                track("/m/3.mp3", "Three"),
                track("/m/4.mp3", "Four"),
            ],
        )
        .unwrap();
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        (conn, ids)
    }

    /// Member ids and titles in play order: what the panel shows.
    fn listed(conn: &Connection, playlist: i64) -> Vec<(i64, String)> {
        playlists::tracks(conn, playlist)
            .unwrap()
            .into_iter()
            .map(|t| (t.member_id, t.title))
            .collect()
    }

    /// Undo, then redo, checking the list before and after each.
    fn round_trip(conn: &mut Connection, playlist: i64, undo: Step, before: &[(i64, String)]) {
        let after = listed(conn, playlist);

        let redo = apply(conn, &undo, 1).unwrap().expect("applies");
        assert_eq!(listed(conn, playlist), before);

        let again = apply(conn, &redo, 2).unwrap().expect("redoes");
        assert_eq!(listed(conn, playlist), after);

        apply(conn, &again, 3).unwrap().expect("undoes again");
        assert_eq!(listed(conn, playlist), before);
    }

    #[test]
    fn a_removal_comes_back_with_its_member_ids() {
        let (mut conn, tracks) = seed();
        let list = playlists::create(&conn, "Mix", 0).unwrap();
        let members = playlists::add(&mut conn, list, &tracks, 0).unwrap();
        let before = listed(&conn, list);

        let gone = vec![members[1], members[2]];
        let undo = Step::new(vec![Change::Insert(rows(&conn, &gone).unwrap())]);
        playlists::remove_members(&mut conn, &gone, 0).unwrap();

        round_trip(&mut conn, list, undo, &before);
    }

    #[test]
    fn an_add_and_a_reorder_round_trip() {
        let (mut conn, tracks) = seed();
        let list = playlists::create(&conn, "Mix", 0).unwrap();
        let members = playlists::add(&mut conn, list, &tracks[..2], 0).unwrap();

        // Dropped in before the second member.
        let before = listed(&conn, list);
        let placed = placements(&conn, list).unwrap();
        let added = playlists::add(&mut conn, list, &tracks[2..], 0).unwrap();
        playlists::place_members(&mut conn, list, &added, Some(members[1]), 0).unwrap();
        let undo = Step::new(vec![Change::Delete(added), Change::Place(placed)]);
        round_trip(&mut conn, list, undo, &before);

        // A drag of the first member to the end.
        let before = listed(&conn, list);
        let undo = Step::new(vec![Change::Place(placements(&conn, list).unwrap())]);
        playlists::place_members(&mut conn, list, &members[..1], None, 0).unwrap();
        round_trip(&mut conn, list, undo, &before);
    }

    #[test]
    fn a_rename_round_trips() {
        let (mut conn, _) = seed();
        let list = playlists::create(&conn, "Mix", 0).unwrap();

        let undo = Step::new(vec![Change::Rename {
            playlist_id: list,
            name: name(&conn, list).unwrap().unwrap(),
        }]);
        playlists::rename(&conn, list, "Road Trip", 0).unwrap();

        let redo = apply(&mut conn, &undo, 1).unwrap().expect("applies");
        assert_eq!(name(&conn, list).unwrap().as_deref(), Some("Mix"));
        apply(&mut conn, &redo, 2).unwrap().expect("redoes");
        assert_eq!(name(&conn, list).unwrap().as_deref(), Some("Road Trip"));
    }

    #[test]
    fn a_step_against_rows_changed_elsewhere_is_refused_whole() {
        let (mut conn, tracks) = seed();
        let list = playlists::create(&conn, "Mix", 0).unwrap();
        let members = playlists::add(&mut conn, list, &tracks, 0).unwrap();

        // The undo of an add, but something else already took one member.
        let undo = Step::new(vec![
            Change::Delete(vec![members[0]]),
            Change::Delete(vec![members[3]]),
        ]);
        playlists::remove_members(&mut conn, &[members[3]], 0).unwrap();
        let before = listed(&conn, list);

        assert_eq!(apply(&mut conn, &undo, 1).unwrap(), None);
        assert_eq!(listed(&conn, list), before, "the first delete rolled back");

        // A removal's undo whose member id was handed out again since.
        let gone = rows(&conn, &[members[2]]).unwrap();
        playlists::remove_members(&mut conn, &[members[2]], 0).unwrap();
        conn.execute(
            "INSERT INTO playlist_tracks (id, playlist_id, track_id, position, title, artist, album)
             VALUES (?1, ?2, ?3, 9, '', '', '')",
            rusqlite::params![members[2], list, tracks[0]],
        )
        .unwrap();
        let undo = Step::new(vec![Change::Insert(gone)]);
        assert_eq!(apply(&mut conn, &undo, 1).unwrap(), None);
    }

    #[test]
    fn a_deleted_playlists_steps_are_refused() {
        let (mut conn, tracks) = seed();
        let list = playlists::create(&conn, "Mix", 0).unwrap();
        let members = playlists::add(&mut conn, list, &tracks, 0).unwrap();

        let gone = rows(&conn, &members[..1]).unwrap();
        playlists::remove_members(&mut conn, &members[..1], 0).unwrap();
        let undo = Step::new(vec![Change::Insert(gone)]);
        assert!(undo.touches(list));

        playlists::delete(&mut conn, list).unwrap();
        assert_eq!(apply(&mut conn, &undo, 1).unwrap(), None);
    }
}
