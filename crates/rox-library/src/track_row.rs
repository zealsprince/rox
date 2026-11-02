//! One track's stored row: every tracks column a client can use, with its
//! listen count and cue span, read by key or by id. The control socket's
//! per-track answers come from here. The projection carries only what
//! browsing needs, and widening it for socket-only columns would cost every
//! browse the memory.
//!
//! The enrichment side tables (artist, album, and genre meta) read here too,
//! one name at a time, for the same answers.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, Row};

use crate::cue::{self, TrackKey};
use crate::replaygain::{self, ReplayGain};

/// Raw columns: the zeros and empty strings the table defaults to are left
/// for the caller to read as absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredTrack {
    pub id: i64,
    pub source: String,
    pub path: String,
    pub sub: u16,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    /// The date tag past the year, 0 where it stops there.
    pub month: u8,
    pub day: u8,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub sample_rate_hz: u32,
    pub bit_depth: u8,
    pub rating: u8,
    pub plays: u32,
    pub last_played: Option<i64>,
    pub added: i64,
    pub size: u64,
    pub comment: String,
    pub title_sort: String,
    pub artist_sort: String,
    pub album_artist_sort: String,
    pub album_sort: String,
    pub remote_url: String,
    pub remote_live: bool,
    pub replay_gain: ReplayGain,
    pub rg_source: replaygain::Source,
    pub bpm: Option<f32>,
    pub bpm_source: crate::tempo::Source,
    /// Start and end inside the image, for a cue subsong. An open end runs to
    /// the end of the file.
    pub span: Option<(u32, Option<u32>)>,
}

impl StoredTrack {
    pub fn key(&self) -> TrackKey {
        TrackKey {
            source: cue::source_id(&self.source),
            path: PathBuf::from(&self.path),
            sub: self.sub,
        }
    }
}

const COLUMNS: &str = "t.id, t.source, t.path, t.sub, t.title, t.artist, t.album_artist,
     t.album, t.genre, t.year, t.disc_no, t.track_no, t.duration_ms, t.codec, t.bitrate,
     t.sample_rate, t.bit_depth, t.rating,
     (SELECT COUNT(*) FROM listens l WHERE l.track_id = t.id),
     (SELECT MAX(played_at) FROM listens l WHERE l.track_id = t.id),
     t.added, t.size, t.comment, t.title_sort, t.artist_sort, t.album_artist_sort,
     t.album_sort, t.remote_url, t.remote_live,
     t.rg_track_gain, t.rg_track_peak, t.rg_album_gain, t.rg_album_peak, t.rg_source,
     t.bpm, t.bpm_source, c.start_ms, c.end_ms, t.month, t.day";

fn stored(row: &Row) -> rusqlite::Result<StoredTrack> {
    let start_ms: Option<i64> = row.get(36)?;
    let end_ms: Option<i64> = row.get(37)?;
    Ok(StoredTrack {
        id: row.get(0)?,
        source: row.get(1)?,
        path: row.get(2)?,
        sub: row.get::<_, i64>(3)? as u16,
        title: row.get(4)?,
        artist: row.get(5)?,
        album_artist: row.get(6)?,
        album: row.get(7)?,
        genre: row.get(8)?,
        year: row.get::<_, i64>(9)? as u16,
        month: row.get::<_, i64>(38)? as u8,
        day: row.get::<_, i64>(39)? as u8,
        disc_no: row.get::<_, i64>(10)? as u16,
        track_no: row.get::<_, i64>(11)? as u16,
        duration_ms: row.get::<_, i64>(12)? as u32,
        codec: row.get(13)?,
        bitrate_kbps: row.get::<_, i64>(14)? as u16,
        sample_rate_hz: row.get::<_, i64>(15)? as u32,
        bit_depth: row.get::<_, i64>(16)? as u8,
        rating: row.get::<_, i64>(17)? as u8,
        plays: row.get::<_, i64>(18)? as u32,
        last_played: row.get(19)?,
        added: row.get(20)?,
        size: row.get::<_, i64>(21)?.max(0) as u64,
        comment: row.get(22)?,
        title_sort: row.get(23)?,
        artist_sort: row.get(24)?,
        album_artist_sort: row.get(25)?,
        album_sort: row.get(26)?,
        remote_url: row.get(27)?,
        remote_live: row.get(28)?,
        replay_gain: ReplayGain {
            track_db: row.get(29)?,
            track_peak: row.get(30)?,
            album_db: row.get(31)?,
            album_peak: row.get(32)?,
        },
        rg_source: replaygain::Source::from_code(row.get(33)?),
        bpm: row.get::<_, Option<f64>>(34)?.map(|bpm| bpm as f32),
        bpm_source: crate::tempo::Source::from_code(row.get(35)?),
        span: start_ms.map(|start| (start as u32, end_ms.map(|end| end as u32))),
    })
}

/// None when no row has that key.
pub fn by_key(conn: &Connection, key: &TrackKey) -> rusqlite::Result<Option<StoredTrack>> {
    let Some(path) = key.path.to_str() else {
        return Ok(None);
    };

    conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM tracks t LEFT JOIN cue_tracks c ON c.track_id = t.id
         WHERE t.source = ?1 AND t.path = ?2 AND t.sub = ?3"
    ))?
    .query_row(
        rusqlite::params![key.source.as_ref(), path, key.sub],
        stored,
    )
    .optional()
}

/// The rows that still exist, by id. A gone id is just missing from the map.
pub fn by_ids(conn: &Connection, ids: &[i64]) -> rusqlite::Result<HashMap<i64, StoredTrack>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM tracks t LEFT JOIN cue_tracks c ON c.track_id = t.id
         WHERE t.id = ?1"
    ))?;
    let mut out = HashMap::with_capacity(ids.len());
    for &id in ids {
        if out.contains_key(&id) {
            continue;
        }

        if let Some(row) = stmt.query_row([id], stored).optional()? {
            out.insert(id, row);
        }
    }
    Ok(out)
}

/// An artist or album meta row: the sort name a lookup or the user gave the
/// name, who gave it, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameMeta {
    pub sort_name: String,
    pub source: String,
    pub fetched: i64,
}

pub fn artist_meta(conn: &Connection, name: &str) -> rusqlite::Result<Option<NameMeta>> {
    name_meta(conn, "artist_meta", name)
}

pub fn album_meta(conn: &Connection, name: &str) -> rusqlite::Result<Option<NameMeta>> {
    name_meta(conn, "album_meta", name)
}

/// `table` is one of the two literals above, never caller text.
fn name_meta(conn: &Connection, table: &str, name: &str) -> rusqlite::Result<Option<NameMeta>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }

    conn.prepare_cached(&format!(
        "SELECT sort_name, source, fetched FROM {table} WHERE name = ?1"
    ))?
    .query_row([name], |row| {
        Ok(NameMeta {
            sort_name: row.get(0)?,
            source: row.get(1)?,
            fetched: row.get(2)?,
        })
    })
    .optional()
}

/// A genre meta row. Empty strings are columns nothing has filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenreMeta {
    pub fold_into: String,
    pub display: String,
    pub art: String,
}

/// Keyed by the lowercased name, the way [`crate::genre_meta`] writes it.
pub fn genre_meta(conn: &Connection, name: &str) -> rusqlite::Result<Option<GenreMeta>> {
    let key = name.trim().to_lowercase();
    if key.is_empty() {
        return Ok(None);
    }

    conn.prepare_cached("SELECT fold_into, display, art FROM genre_meta WHERE name = ?1")?
        .query_row([key], |row| {
            Ok(GenreMeta {
                fold_into: row.get(0)?,
                display: row.get(1)?,
                art: row.get(2)?,
            })
        })
        .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::init_schema(&conn).unwrap();
        conn
    }

    fn insert(conn: &Connection, source: &str, path: &str, sub: u16) -> i64 {
        conn.execute(
            "INSERT INTO tracks (source, path, sub, title, artist, album, genre, year, track_no,
                duration_ms, size, mtime)
             VALUES (?1, ?2, ?3, 'T', 'A', 'Al', 'g', 1999, 4, 200000, 1234, 0)",
            rusqlite::params![source, path, sub as i64],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn a_row_reads_back_by_key_and_id_with_its_plays_and_span() {
        let conn = library();
        let file = insert(&conn, "local", "/m/a.flac", 0);
        let cue = insert(&conn, "local", "/m/disc.flac", 2);
        conn.execute(
            "INSERT INTO cue_tracks (track_id, cue_path, start_ms, end_ms)
             VALUES (?1, '/m/disc.cue', 61000, 125000)",
            [cue],
        )
        .unwrap();
        for played_at in [100, 300, 200] {
            conn.execute(
                "INSERT INTO listens (track_id, played_at, title, artist, album, genre)
                 VALUES (?1, ?2, 'T', 'A', 'Al', 'g')",
                [file, played_at],
            )
            .unwrap();
        }

        conn.execute(
            "UPDATE tracks SET month = 6, day = 12 WHERE id = ?1",
            [file],
        )
        .unwrap();

        let row = by_key(&conn, &TrackKey::from(PathBuf::from("/m/a.flac")))
            .unwrap()
            .unwrap();
        assert_eq!(row.id, file);
        assert_eq!((row.year, row.track_no, row.size), (1999, 4, 1234));
        assert_eq!((row.month, row.day), (6, 12));
        assert_eq!((row.plays, row.last_played), (3, Some(300)));
        assert_eq!(row.span, None);
        assert_eq!(row.key(), TrackKey::from(PathBuf::from("/m/a.flac")));

        let rows = by_ids(&conn, &[cue, 9999]).unwrap();
        assert_eq!(rows.len(), 1, "a gone id is left out");
        assert_eq!(rows[&cue].span, Some((61000, Some(125000))));
        assert_eq!((rows[&cue].plays, rows[&cue].last_played), (0, None));

        let missing = TrackKey::from(PathBuf::from("/m/nowhere.flac"));
        assert_eq!(by_key(&conn, &missing).unwrap(), None);
    }

    #[test]
    fn the_meta_tables_read_one_name_at_a_time() {
        let conn = library();
        crate::artist_meta::set(&conn, "Björk", "Bjork", "musicbrainz").unwrap();
        crate::genre_meta::set_alias(&conn, "DnB", "Drum & Bass").unwrap();

        let artist = artist_meta(&conn, "Björk").unwrap().unwrap();
        assert_eq!(artist.sort_name, "Bjork");
        assert_eq!(artist.source, "musicbrainz");
        assert_eq!(album_meta(&conn, "Homogenic").unwrap(), None);
        assert_eq!(
            genre_meta(&conn, "DNB").unwrap().map(|g| g.fold_into),
            Some("Drum & Bass".into())
        );
        assert_eq!(artist_meta(&conn, "  ").unwrap(), None);
    }
}
