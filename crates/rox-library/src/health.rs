//! One definition of "completely tagged", shared by the health window, the
//! overview ring and the widget so they can't drift apart.
//!
//! The five tags are title, artist, album, genre, year. Rating is out: unrated
//! isn't untagged. The headline is the plain share of live rows missing none
//! of the five, and tombstones never count.

use crate::projection::Projection;

/// Ordered the way a tag editor lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Title,
    Artist,
    Album,
    Genre,
    Year,
}

impl Check {
    pub const ALL: [Check; 5] = [
        Check::Title,
        Check::Artist,
        Check::Album,
        Check::Genre,
        Check::Year,
    ];

    pub fn bit(self) -> u8 {
        match self {
            Check::Title => 1,
            Check::Artist => 2,
            Check::Album => 4,
            Check::Genre => 8,
            Check::Year => 16,
        }
    }
}

/// 2^5 missing-mask buckets, so a caller can ask "complete over these three"
/// without a second walk.
const COMBOS: usize = 32;

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Missing {
    pub count: u64,
    /// The first `cap` of their ids, for a drill-down filter.
    pub ids: Vec<i64>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Completeness {
    pub tracks: u64,
    pub title: Missing,
    pub artist: Missing,
    pub album: Missing,
    pub genre: Missing,
    pub year: Missing,
    /// Live rows per missing-mask bucket. Needed for exact subset counts: a row
    /// missing two checks is in both per-check counts.
    combos: [u64; COMBOS],
}

impl Completeness {
    pub fn missing(&self, check: Check) -> &Missing {
        match check {
            Check::Title => &self.title,
            Check::Artist => &self.artist,
            Check::Album => &self.album,
            Check::Genre => &self.genre,
            Check::Year => &self.year,
        }
    }

    fn missing_mut(&mut self, check: Check) -> &mut Missing {
        match check {
            Check::Title => &mut self.title,
            Check::Artist => &mut self.artist,
            Check::Album => &mut self.album,
            Check::Genre => &mut self.genre,
            Check::Year => &mut self.year,
        }
    }

    /// Public so any other counter goes through the same arithmetic instead of
    /// touching the buckets.
    pub fn add_row(&mut self, missing: u8) {
        self.tracks += 1;
        self.combos[(missing & 0b1_1111) as usize] += 1;
        for check in Check::ALL {
            if missing & check.bit() != 0 {
                self.missing_mut(check).count += 1;
            }
        }
    }

    pub fn complete(&self) -> u64 {
        self.combos[0]
    }

    pub fn complete_within(&self, checks: &[Check]) -> u64 {
        let wanted = checks.iter().fold(0u8, |mask, check| mask | check.bit());
        self.combos
            .iter()
            .enumerate()
            .filter(|(mask, _)| *mask as u8 & wanted == 0)
            .map(|(_, count)| count)
            .sum()
    }

    /// An empty library reads as 1.0.
    pub fn share_within(&self, checks: &[Check]) -> f32 {
        if self.tracks == 0 {
            return 1.0;
        }
        self.complete_within(checks) as f32 / self.tracks as f32
    }

    pub fn share(&self) -> f32 {
        self.share_within(&Check::ALL)
    }

    pub fn coverage(&self, check: Check) -> f32 {
        if self.tracks == 0 {
            return 1.0;
        }
        let present = self.tracks - self.missing(check).count.min(self.tracks);
        present as f32 / self.tracks as f32
    }
}

/// `drill_cap` bounds the ids kept per check, since a filter pinning them
/// matches row by row. Counts stay exact.
pub fn completeness(projection: &Projection, drill_cap: usize) -> Completeness {
    // Per distinct value, not per row. Blank-looking tags count as missing, on
    // the same predicate as `genre_suggest::untagged`, so the two agree.
    let artist_missing: Vec<bool> = projection
        .artists
        .strings
        .iter()
        .map(|name| name.trim().is_empty())
        .collect();
    let album_missing: Vec<bool> = projection
        .albums
        .strings
        .iter()
        .map(|name| name.trim().is_empty())
        .collect();
    let genre_missing: Vec<bool> = projection
        .genres
        .strings
        .iter()
        .map(|value| crate::genre::split(value).next().is_none())
        .collect();

    let mut out = Completeness::default();
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }
        let mut mask = 0u8;
        // An untitled file gets its stem as title, so this mostly catches cue and
        // patched rows. Catching filename-only titles needs a filename column.
        if projection.title.get(row).trim().is_empty() {
            mask |= Check::Title.bit();
        }
        if artist_missing[projection.artist[row] as usize] {
            mask |= Check::Artist.bit();
        }
        if album_missing[projection.album[row] as usize] {
            mask |= Check::Album.bit();
        }
        if genre_missing[projection.genre[row] as usize] {
            mask |= Check::Genre.bit();
        }
        if projection.year[row] == 0 {
            mask |= Check::Year.bit();
        }
        out.add_row(mask);
        if mask == 0 || drill_cap == 0 {
            continue;
        }
        let id = projection.db_id[row];
        for check in Check::ALL {
            if mask & check.bit() != 0 {
                let missing = out.missing_mut(check);
                if missing.ids.len() < drill_cap {
                    missing.ids.push(id);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rusqlite::Connection;
    use crate::{TrackRow, store};

    fn track(path: &str) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            path: path.into(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: "Shoegaze".into(),
            year: 1993,
            disc_no: 1,
            track_no: 1,
            duration_ms: 200_000,
            codec: "mp3".into(),
            bitrate_kbps: 320,
            sample_rate_hz: 44100,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    #[test]
    fn a_dead_row_is_in_nothing_and_a_half_tagged_one_is_counted_once() {
        let complete = track("/m/a/1.mp3");
        let mut bare = track("/m/a/2.mp3");
        bare.genre = String::new();
        bare.year = 0;
        let doomed = track("/m/a/3.mp3");
        let mut p = projection(&[complete, bare, doomed]);
        let index: std::collections::HashMap<i64, u32> = p
            .db_id
            .iter()
            .enumerate()
            .map(|(row, id)| (*id, row as u32))
            .collect();
        let gone = p.db_id[2];
        p.remove_ids(&[gone], &index);

        let health = completeness(&p, 100);
        assert_eq!(health.tracks, 2, "the tombstone is not a track");
        assert_eq!(health.complete(), 1);
        assert_eq!(health.genre.count, 1);
        assert_eq!(health.year.count, 1);
        assert_eq!(health.title.count, 0);
        assert_eq!(health.artist.count, 0);
        assert_eq!(health.album.count, 0);
        assert_eq!(health.genre.ids, health.year.ids);
        assert_eq!(health.share(), 0.5);
    }

    #[test]
    fn a_subset_counts_only_the_checks_it_names() {
        let complete = track("/m/a/1.mp3");
        let mut no_genre = track("/m/a/2.mp3");
        no_genre.genre = String::new();
        let mut neither = track("/m/a/3.mp3");
        neither.genre = String::new();
        neither.year = 0;
        let p = projection(&[complete, no_genre, neither]);

        let health = completeness(&p, 100);
        assert_eq!(health.complete(), 1);
        assert_eq!(health.complete_within(&[Check::Genre]), 1);
        assert_eq!(health.complete_within(&[Check::Year]), 2);
        assert_eq!(health.complete_within(&[Check::Genre, Check::Year]), 1);
        assert_eq!(
            health.complete_within(&[Check::Title, Check::Artist, Check::Album]),
            3
        );
        assert_eq!(health.complete_within(&[]), 3, "nothing to fail");
    }

    #[test]
    fn the_cap_bounds_the_ids_and_not_the_count() {
        let rows: Vec<TrackRow> = (0..3)
            .map(|i| {
                let mut row = track(&format!("/m/a/{i}.mp3"));
                row.year = 0;
                row
            })
            .collect();
        let p = projection(&rows);

        let health = completeness(&p, 2);
        assert_eq!(health.year.count, 3);
        assert_eq!(health.year.ids.len(), 2);
    }

    #[test]
    fn blank_looking_tags_count_as_missing() {
        let mut spaced = track("/m/a/1.mp3");
        spaced.genre = " ; ".into();
        spaced.artist = "  ".into();
        spaced.album = "\t".into();
        let mut titled = track("/m/a/2.mp3");
        titled.title = "   ".into();
        let p = projection(&[spaced, titled]);

        let health = completeness(&p, 10);
        assert_eq!(health.genre.count, 1);
        assert_eq!(health.artist.count, 1);
        assert_eq!(health.album.count, 1);
        assert_eq!(health.title.count, 1);
        assert_eq!(health.complete(), 0);
        assert_eq!(crate::genre_suggest::untagged(&p), [0]);
    }

    #[test]
    fn an_empty_library_reads_as_complete() {
        let p = projection(&[]);
        let health = completeness(&p, 10);
        assert_eq!(health.tracks, 0);
        assert_eq!(health.share(), 1.0);
        assert_eq!(health.coverage(Check::Genre), 1.0);
    }
}
