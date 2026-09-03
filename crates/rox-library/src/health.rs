//! One definition of "completely tagged", for everything that wants to put a
//! number on a library.
//!
//! The health window grew its own walk over the projection first, and then the
//! overview ring and the widget wanted the same answer; three walks with three
//! slightly different ideas of what counts would drift within a release. So
//! the walk lives here, next to the projection it reads, as a pure function
//! over a snapshot: no settings, no i18n, no entities.
//!
//! What counts is the five tags a track needs before the library can file it
//! the way its owner would look for it: title, artist, album, genre, year.
//! Rating is deliberately out. An unrated track isn't an untagged one, and
//! folding a taste judgement into a coverage number makes the number mean two
//! things at once. There's no weighting either: the headline is the plain
//! share of live rows missing none of the five, so a user can check it by
//! hand.
//!
//! Dead rows are skipped throughout. A tombstone is a file the library has
//! already let go of, and counting it would let a rescan of deleted music move
//! a coverage number.

use crate::projection::Projection;

/// One of the five tags a complete track carries.
///
/// Ordered the way a tag editor lists them, which is also the order the
/// overview draws its rows in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Title,
    Artist,
    Album,
    Genre,
    Year,
}

impl Check {
    /// Every check, in listing order.
    pub const ALL: [Check; 5] = [
        Check::Title,
        Check::Artist,
        Check::Album,
        Check::Genre,
        Check::Year,
    ];

    /// This check's bit in the per-row missing mask [`Completeness::add_row`]
    /// takes.
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

/// Every combination of the five checks a row can be missing: 2^5 buckets,
/// which is what lets a caller ask "complete over these three" without a
/// second walk.
const COMBOS: usize = 32;

/// One check's result: how many live rows are missing it, and enough of their
/// database ids to open a drill-down with.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Missing {
    /// Every live row missing this tag, uncapped.
    pub count: u64,
    /// The first `cap` of their ids, for a caller that pins them into a
    /// filter. Empty when the caller asked for no cap.
    pub ids: Vec<i64>,
}

/// What one walk of the projection says about how well the library is tagged.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Completeness {
    /// Live rows: the denominator every share here reads against.
    pub tracks: u64,
    pub title: Missing,
    pub artist: Missing,
    pub album: Missing,
    pub genre: Missing,
    pub year: Missing,
    /// How many live rows fall in each missing-mask bucket, indexed by the
    /// OR of the missing checks' bits. Bucket zero is the rows missing
    /// nothing.
    ///
    /// This exists so a caller can count "complete over an arbitrary subset"
    /// exactly, which the per-check counts can't answer: a row missing both
    /// genre and year appears in two of them, and adding them double-counts
    /// it. Thirty-two counters is a rounding error next to the walk itself.
    combos: [u64; COMBOS],
}

impl Completeness {
    /// One check's missing rows.
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

    /// Fold one live row in by the mask of checks it's missing, the OR of
    /// their [`Check::bit`]s.
    ///
    /// The walk's own step, and public so anything counting rows from
    /// somewhere other than a projection walk goes through the same
    /// arithmetic rather than reaching into the buckets, where it would be
    /// one forgotten increment away from a number that disagrees with the
    /// per-check counts beside it.
    pub fn add_row(&mut self, missing: u8) {
        self.tracks += 1;
        self.combos[(missing & 0b1_1111) as usize] += 1;
        for check in Check::ALL {
            if missing & check.bit() != 0 {
                self.missing_mut(check).count += 1;
            }
        }
    }

    /// Live rows missing none of the five: the headline number.
    pub fn complete(&self) -> u64 {
        self.combos[0]
    }

    /// Live rows missing none of `checks`, ignoring the rest. An empty list
    /// counts every live row, which is the honest answer to "complete over
    /// nothing".
    pub fn complete_within(&self, checks: &[Check]) -> u64 {
        let wanted = checks.iter().fold(0u8, |mask, check| mask | check.bit());
        self.combos
            .iter()
            .enumerate()
            .filter(|(mask, _)| *mask as u8 & wanted == 0)
            .map(|(_, count)| count)
            .sum()
    }

    /// The share of live rows missing none of `checks`, 0.0 to 1.0. An empty
    /// library reads as 1.0: nothing is untagged when there's nothing.
    pub fn share_within(&self, checks: &[Check]) -> f32 {
        if self.tracks == 0 {
            return 1.0;
        }
        self.complete_within(checks) as f32 / self.tracks as f32
    }

    /// The share of live rows missing none of the five.
    pub fn share(&self) -> f32 {
        self.share_within(&Check::ALL)
    }

    /// The share of live rows that carry `check`, 0.0 to 1.0, which is what a
    /// coverage bar fills to.
    pub fn coverage(&self, check: Check) -> f32 {
        if self.tracks == 0 {
            return 1.0;
        }
        let present = self.tracks - self.missing(check).count.min(self.tracks);
        present as f32 / self.tracks as f32
    }
}

/// Walk one projection snapshot and count the five checks.
///
/// `drill_cap` bounds the ids kept per check: a caller that pins them into a
/// filter matches row by row, so an uncapped set on a large library would be
/// quadratic. The counts stay exact either way, so a capped list is a sample
/// and the caller can say so.
///
/// Sequential rather than split across cores: it's a handful of bytes a row
/// against a per-file probe, and it runs on a projection swap, not per frame.
pub fn completeness(projection: &Projection, drill_cap: usize) -> Completeness {
    // Asked once per distinct value rather than once per row: an untagged
    // artist, album or genre is the interned empty string, and a library
    // holds far fewer names than tracks.
    //
    // A tag holding nothing but spaces is as untagged as a blank one, so all
    // three test the trimmed value; the genre goes through the splitter the
    // rest of the library files genres with, which makes a lone "; " the
    // empty list it reads as everywhere else. Suggestions come off the same
    // predicate (see [`crate::genre_suggest::untagged`]), and a tile saying
    // a library is fully tagged while the suggester offers rows to tag would
    // be one of the two lying.
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
        // Scanned rows almost never fail this one: an untitled file gets the
        // filename stem as its title (see the scanner's `fallback_row`), so
        // the honest test is "the title is only the filename", which needs a
        // per-row filename the projection doesn't carry. Until it does, this
        // catches the empty titles a cue sheet and a patched row can still
        // produce and nothing else.
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
    use crate::{store, TrackRow};

    /// A row with everything the five checks read filled in; a test blanks
    /// whichever fields it wants missing.
    fn track(path: &str) -> TrackRow {
        TrackRow {
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

    /// Three rows: one complete, one missing genre and year, one the library
    /// has let go of. The dead row is in no count at all, the half-tagged one
    /// is in two, and it's counted once against the headline rather than
    /// twice.
    #[test]
    fn a_dead_row_is_in_nothing_and_a_half_tagged_one_is_counted_once() {
        let complete = track("/m/a/1.mp3");
        let mut bare = track("/m/a/2.mp3");
        bare.genre = String::new();
        bare.year = 0;
        let doomed = track("/m/a/3.mp3");
        let mut p = projection(&[complete, bare, doomed]);
        // The third row's file is gone: tombstone it the way a rescan does.
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
        // Genre and year are missing on the same row, so the two per-check
        // counts add up to more rows than there are incomplete ones.
        assert_eq!(health.genre.ids, health.year.ids);
        assert_eq!(health.share(), 0.5);
    }

    /// The subset knob: dropping the checks a row fails makes it complete,
    /// and the arithmetic never double-counts the row failing two of them.
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

    /// The cap bounds the drill-down list without touching the count, so a
    /// tile can say "showing 2 of 3".
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

    /// A tag holding only separators and spaces counts as missing, the same
    /// way the genre suggester counts it, so the coverage number and the
    /// list of rows to fix agree on which rows are untagged.
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
        // The same rows the suggester would offer, off the same predicate.
        assert_eq!(crate::genre_suggest::untagged(&p), [0]);
    }

    /// An empty library is fully tagged, not zero percent tagged: there's
    /// nothing to fix, and a ring reading 0% would send a user looking.
    #[test]
    fn an_empty_library_reads_as_complete() {
        let p = projection(&[]);
        let health = completeness(&p, 10);
        assert_eq!(health.tracks, 0);
        assert_eq!(health.share(), 1.0);
        assert_eq!(health.coverage(Check::Genre), 1.0);
    }
}
