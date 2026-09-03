//! What genre an untagged track probably is, argued from the library the
//! user already tagged.
//!
//! Nothing here talks to a service or guesses from a title. A library that
//! has been curated for years is its own best reference: the rest of the
//! album, the rest of the artist, and the tracks that sound like this one
//! already carry the answer often enough that filling a blank genre is a
//! confirmation rather than a decision. So this is a vote. Every live row
//! that has something to say about the seed puts weight behind the values it
//! carries, the weights are summed per value, and the caller gets the ranked
//! result with the evidence still attached, because a suggestion a user can't
//! see the reason for is a suggestion they have to check by hand anyway.
//!
//! Written in the register of [`crate::health`]: a pure function over a
//! projection snapshot, no settings, no i18n, no entities. [`suggest`] is the
//! only thing here that touches SQLite, and only to ask the acoustic table
//! for neighbours; the vote itself never does.
//!
//! The weights are constants rather than a tuned model, and they're lopsided
//! on purpose. `examples/genreprobe.rs` hides the genre on tagged tracks and
//! scores what the vote would have said; on a 53k-track library it puts album
//! siblings at 98.6% top-1 on their own, artist siblings at 87.7%, and
//! acoustic neighbours at 58.4%. So the album is the unit and everything else
//! is priced well under it.
//!
//! The gap between those and the per-row weights is cardinality, which is the
//! part that isn't obvious: a seed has a handful of album siblings, a couple
//! of dozen neighbours, and sometimes hundreds of artist siblings. At equal
//! weights a prolific artist's other lane simply outvotes the album the track
//! is on, and the combined vote scored worse than the album alone. Dropping
//! the per-row artist weight far enough that a whole discography roughly
//! ties one album is what turns the combination into an improvement (98.8%
//! against the album's 98.6%, at full coverage instead of 99.6%). Read the
//! artist and acoustic numbers as tie-breaks and gap-fillers, not as
//! statements about how trustworthy those sources are on their own.
//!
//! An acoustic neighbour weighs its own cosine, which already says how much
//! of a neighbour it is, and a negative one weighs nothing rather than voting
//! against. A lookup value is priced at one and a half album siblings: a
//! service that names a genre has usually been told it by a person, so it
//! beats one sibling, but an album that agrees with itself still beats the
//! service. That one is unmeasured, since the probe votes with no lookup.
//!
//! Genre values are "; " lists (see [`crate::genre`]), so the tally is per
//! split value, resolved through the alias map and grouped case-folded. The
//! spelling that comes back is the one the most rows use, so a suggestion
//! written into a file matches its neighbours character for character.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::projection::Projection;

/// How many acoustic neighbours a suggestion reads.
pub const NEIGHBOURS: usize = 24;

/// What one live row on the same album is worth. The unit the rest is
/// priced against: a track whose album siblings agree is not a guess.
pub const ALBUM_WEIGHT: f32 = 8.0;

/// What one live row by the same artist is worth, when it isn't already
/// counted as an album sibling. Small because there are so many of them:
/// eighty of them tie one album, which is about where a prolific artist's
/// other lane stops overruling the record in front of it.
pub const ARTIST_WEIGHT: f32 = 0.1;

/// What one acoustic neighbour's cosine is multiplied by. The score does the
/// real weighting; this is the dial that says how much the model gets to
/// argue against the tags, and it's set low: the whole neighbour set at full
/// agreement is worth about half an album sibling.
pub const ACOUSTIC_WEIGHT: f32 = 0.15;

/// What one value an external service offered is worth: one and a half album
/// siblings, expressed against the album so a retune of that one carries.
pub const LOOKUP_WEIGHT: f32 = ALBUM_WEIGHT * 1.5;

/// The four sources' weights, so the probe can turn them one at a time and a
/// later tuning pass has one place to change.
///
/// A weight of zero turns its source off rather than counting it at zero:
/// a source that contributes nothing shouldn't put its values on the ballot
/// where they'd take the ranking's tie-breaks with them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    pub album: f32,
    pub artist: f32,
    pub acoustic: f32,
    pub lookup: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            album: ALBUM_WEIGHT,
            artist: ARTIST_WEIGHT,
            acoustic: ACOUSTIC_WEIGHT,
            lookup: LOOKUP_WEIGHT,
        }
    }
}

/// One candidate genre and where its support came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The genre value as it would be written, one value (never a "; " list),
    /// passed through `crate::genre::resolve`.
    pub genre: String,
    /// Normalized weight in 0..=1: the best candidate's share of the total
    /// weight of all candidates. Sorted descending, ties by name.
    pub score: f32,
    /// Live rows on the same album (same album symbol and same folder) that
    /// carry this value.
    pub album: usize,
    /// Live rows by the same artist (artist or album_artist symbol match),
    /// not already counted under album, that carry this value.
    pub artist: usize,
    /// Acoustic neighbours that carry this value.
    pub acoustic: usize,
    /// Whether an external lookup offered this value.
    pub lookup: bool,
}

/// One value's running total while the vote is being counted.
#[derive(Default)]
struct Tally {
    weight: f32,
    album: usize,
    artist: usize,
    acoustic: usize,
    lookup: bool,
    /// How many times each spelling of this value was seen, so the winner
    /// can display the one the library actually writes.
    spellings: HashMap<String, usize>,
}

impl Tally {
    fn saw(&mut self, display: &str) {
        match self.spellings.get_mut(display) {
            Some(count) => *count += 1,
            None => {
                self.spellings.insert(display.to_string(), 1);
            }
        }
    }

    /// The most common spelling, ties to the lexicographically smaller so
    /// two runs over the same library agree.
    fn display(&self) -> String {
        self.spellings
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
    }
}

/// Every live row with an empty genre, as projection row indices, in
/// projection order.
pub fn untagged(projection: &Projection) -> Vec<u32> {
    // Asked once per distinct value rather than once per row, the same move
    // [`crate::health::completeness`] makes: a library holds far fewer genre
    // strings than tracks, and a whitespace-only tag is as empty as a blank
    // one.
    let blank: Vec<bool> = projection
        .genres
        .strings
        .iter()
        .map(|value| crate::genre::split(value).next().is_none())
        .collect();
    (0..projection.len() as u32)
        .filter(|&row| !projection.is_dead(row) && blank[projection.genre[row as usize] as usize])
        .collect()
}

/// The pure vote for one row. `neighbours` is (db track id, score) as
/// `embeddings::ranked` returns them, nearest first (empty when there's no
/// vector). `lookup` is genre strings an external service offered (may be
/// "; " lists; split them). The seed row itself never contributes. At most
/// `cap` suggestions.
pub fn vote(
    projection: &Projection,
    row: u32,
    neighbours: &[(i64, f32)],
    lookup: &[String],
    cap: usize,
) -> Vec<Suggestion> {
    vote_weighted(projection, row, neighbours, lookup, cap, Weights::default())
}

/// [`vote`] with the weights named, for the probe that measures what each
/// source is worth on its own.
///
/// One pass over the columns, which is what makes this affordable to call
/// per row rather than per library: the album siblings, the artist siblings
/// and the acoustic neighbours are all recognized in the same loop, against
/// a small map of the neighbour ids. There's no db-id index on the
/// projection to reach for, and building one here would cost a hash entry
/// per track to answer two dozen questions.
pub fn vote_weighted(
    projection: &Projection,
    row: u32,
    neighbours: &[(i64, f32)],
    lookup: &[String],
    cap: usize,
    weights: Weights,
) -> Vec<Suggestion> {
    let seed = row as usize;
    if seed >= projection.len() || cap == 0 {
        return Vec::new();
    }

    let album_on = weights.album > 0.;
    let artist_on = weights.artist > 0.;
    let acoustic_on = weights.acoustic > 0. && !neighbours.is_empty();

    // The seed's own names, folded, empties dropped: an untagged artist is
    // the empty symbol, and letting it match would make every other
    // artistless row in the library a sibling. The album has the folder
    // beside it to bound it, so it needs no such guard.
    let mut wanted: HashSet<&str> = HashSet::new();
    if artist_on {
        for name in [
            projection.artists.lower[projection.artist[seed] as usize].as_str(),
            projection.album_artists.lower[projection.album_artist[seed] as usize].as_str(),
        ] {
            if !name.is_empty() {
                wanted.insert(name);
            }
        }
    }
    // Per symbol rather than per row again, and the two tables intern
    // separately so the match is by name and not by symbol id.
    let artist_hit: Vec<bool> = symbol_hits(&projection.artists.lower, &wanted);
    let album_artist_hit: Vec<bool> = symbol_hits(&projection.album_artists.lower, &wanted);

    let nearby: HashMap<i64, f32> = if acoustic_on {
        neighbours
            .iter()
            .take(NEIGHBOURS)
            .map(|&(id, score)| (id, score))
            .collect()
    } else {
        HashMap::new()
    };

    let seed_album = projection.album[seed];
    let seed_folder = projection.folder[seed];
    let mut values: Vec<Option<Vec<(String, String)>>> =
        vec![None; projection.genres.strings.len()];
    let mut tallies: HashMap<String, Tally> = HashMap::new();

    for i in 0..projection.len() {
        if i == seed || projection.is_dead(i as u32) {
            continue;
        }
        let album =
            album_on && projection.album[i] == seed_album && projection.folder[i] == seed_folder;
        let artist = !album
            && artist_on
            && (artist_hit[projection.artist[i] as usize]
                || album_artist_hit[projection.album_artist[i] as usize]);
        let acoustic = nearby.get(&projection.db_id[i]).copied();
        if !album && !artist && acoustic.is_none() {
            continue;
        }
        // A row can be both a sibling and a neighbour, and it counts as
        // both: the two are separate claims about the same value, and
        // hiding one of them from the caller's evidence would make the
        // counts stop adding up to the score.
        let mut weight = 0.;
        if album {
            weight += weights.album;
        }
        if artist {
            weight += weights.artist;
        }
        if let Some(score) = acoustic {
            weight += score.max(0.) * weights.acoustic;
        }
        for (display, key) in row_values(projection, i, &mut values) {
            let tally = tallies.entry(key.clone()).or_default();
            tally.weight += weight;
            tally.album += usize::from(album);
            tally.artist += usize::from(artist);
            tally.acoustic += usize::from(acoustic.is_some());
            tally.saw(display);
        }
    }

    if weights.lookup > 0. {
        for offered in lookup {
            let mut seen: HashSet<String> = HashSet::new();
            for part in crate::genre::split(offered) {
                let display = crate::genre::resolve(part);
                let key = display.to_lowercase();
                if !seen.insert(key.clone()) {
                    continue;
                }
                let tally = tallies.entry(key).or_default();
                tally.weight += weights.lookup;
                tally.lookup = true;
                tally.saw(&display);
            }
        }
    }

    let total: f32 = tallies.values().map(|t| t.weight).sum();
    let mut out: Vec<Suggestion> = tallies
        .values()
        .map(|tally| Suggestion {
            genre: tally.display(),
            // A ballot where every voter weighed nothing (a neighbour set
            // that is all negative cosines) still names its candidates,
            // scored honestly at zero.
            score: if total > 0. { tally.weight / total } else { 0. },
            album: tally.album,
            artist: tally.artist,
            acoustic: tally.acoustic,
            lookup: tally.lookup,
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.genre.cmp(&b.genre))
    });
    out.truncate(cap);
    out
}

/// Which symbols in a folded table are one of the names wanted. All false
/// when nothing is, which is the artistless seed and costs one pass over the
/// table rather than a branch per row.
fn symbol_hits(lower: &[String], wanted: &HashSet<&str>) -> Vec<bool> {
    if wanted.is_empty() {
        return vec![false; lower.len()];
    }
    lower
        .iter()
        .map(|name| !name.is_empty() && wanted.contains(name.as_str()))
        .collect()
}

/// One row's genre values as (display, folded key) pairs, resolved through
/// the alias map, memoized per genre symbol.
///
/// Memoized because the resolution takes the alias lock and allocates, and a
/// vote's matching rows share a handful of genre strings between them; the
/// cache is one slot per distinct value in the library, filled only for the
/// values the vote actually meets.
fn row_values<'a>(
    projection: &Projection,
    row: usize,
    cache: &'a mut [Option<Vec<(String, String)>>],
) -> &'a [(String, String)] {
    let sym = projection.genre[row] as usize;
    if cache[sym].is_none() {
        let mut values: Vec<(String, String)> = Vec::new();
        for part in crate::genre::split(&projection.genres.strings[sym]) {
            let display = crate::genre::resolve(part);
            let key = display.to_lowercase();
            // A tag spelling one value twice ("Rock; rock") is one vote.
            if !values.iter().any(|(_, seen)| *seen == key) {
                values.push((display, key));
            }
        }
        cache[sym] = Some(values);
    }
    cache[sym].as_deref().unwrap_or(&[])
}

/// [`vote`] fed by the acoustic table: runs `embeddings::ranked(conn, id, model)`
/// for the row's db id, keeps the top NEIGHBOURS, and votes. Any error or a
/// row with no vector degrades to a vote with no neighbours; never panics.
pub fn suggest(
    conn: &Connection,
    model: &str,
    projection: &Projection,
    row: u32,
    lookup: &[String],
    cap: usize,
) -> Vec<Suggestion> {
    let neighbours = projection
        .db_id
        .get(row as usize)
        .and_then(|&id| crate::embeddings::ranked(conn, id, model).ok())
        .map(|scored| nearest(scored, NEIGHBOURS))
        .unwrap_or_default();
    vote(projection, row, &neighbours, lookup, cap)
}

/// The best `k` of a score map, nearest first, ties by id so two calls
/// agree. `ranked` hands back the whole library scored, and only the head of
/// it is a neighbour; selecting before sorting keeps the cost off the tail
/// nobody reads.
pub fn nearest(mut scored: Vec<(i64, f32)>, k: usize) -> Vec<(i64, f32)> {
    let cmp = |a: &(i64, f32), b: &(i64, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
    if scored.len() > k && k > 0 {
        scored.select_nth_unstable_by(k - 1, cmp);
        scored.truncate(k);
    }
    scored.sort_by(cmp);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rusqlite::Connection;
    use crate::{store, TrackRow};

    /// A plain row; a test sets the fields its case is about.
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

    fn row_of(p: &Projection, title: &str) -> u32 {
        (0..p.len()).find(|&i| p.title.get(i) == title).unwrap() as u32
    }

    /// The blank list is live rows only, and a whitespace-only tag is as
    /// blank as an empty one.
    #[test]
    fn untagged_lists_only_live_empty_rows() {
        let tagged = track("/m/a/1.mp3");
        let mut blank = track("/m/a/2.mp3");
        blank.genre = String::new();
        blank.title = "Blank".into();
        let mut spaced = track("/m/a/3.mp3");
        spaced.genre = " ; ".into();
        spaced.title = "Spaced".into();
        let mut doomed = track("/m/a/4.mp3");
        doomed.genre = String::new();
        doomed.title = "Doomed".into();
        let mut p = projection(&[tagged, blank, spaced, doomed]);

        let index: HashMap<i64, u32> = p
            .db_id
            .iter()
            .enumerate()
            .map(|(row, id)| (*id, row as u32))
            .collect();
        let gone = p.db_id[3];
        p.remove_ids(&[gone], &index);

        let blanks = untagged(&p);
        assert_eq!(blanks, [1, 2], "the tagged row and the tombstone are out");
    }

    /// Two album siblings against three artist siblings: the album wins on
    /// weight even outnumbered, and the counts say which rows were which.
    #[test]
    fn album_siblings_outweigh_artist_siblings() {
        // `folder` is the sibling rows' directory: the same album name in
        // two places is two albums.
        let library = |folder: &str| {
            let mut rows = Vec::new();
            let mut seed = track("/m/album/1.mp3");
            seed.title = "Seed".into();
            seed.genre = String::new();
            rows.push(seed);
            for i in 0..2 {
                let mut row = track(&format!("{folder}/{}.mp3", i + 2));
                row.genre = "Shoegaze".into();
                rows.push(row);
            }
            for i in 0..3 {
                let mut row = track(&format!("/m/other/{i}.mp3"));
                row.album = "Other".into();
                row.genre = "Dream Pop".into();
                rows.push(row);
            }
            rows
        };
        let p = projection(&library("/m/album"));
        let seed = row_of(&p, "Seed");

        let out = vote(&p, seed, &[], &[], 5);
        assert_eq!(out[0].genre, "Shoegaze");
        assert_eq!((out[0].album, out[0].artist), (2, 0));
        assert_eq!(out[1].genre, "Dream Pop");
        assert_eq!((out[1].album, out[1].artist), (0, 3));
        assert!(out[0].score > out[1].score);
        // Two albums against three artists, priced.
        let album = 2. * ALBUM_WEIGHT;
        let artist = 3. * ARTIST_WEIGHT;
        assert!((out[0].score - album / (album + artist)).abs() < 1e-6);

        // The same album name in another folder is not the same album.
        let p = projection(&library("/m/reissue"));
        let seed = row_of(&p, "Seed");
        let out = vote(&p, seed, &[], &[], 5);
        assert_eq!(out[0].genre, "Dream Pop", "only artist siblings are left");
        assert_eq!(out[0].album, 0);
    }

    /// Neighbours vote their cosine, a negative one weighs nothing, and the
    /// seed's own row is never a voter even when the caller hands it in.
    #[test]
    fn neighbours_count_and_the_seed_never_votes() {
        let mut seed = track("/m/a/1.mp3");
        seed.title = "Seed".into();
        seed.genre = "Ambient".into();
        seed.artist = "Seed Artist".into();
        seed.album_artist = "Seed Artist".into();
        seed.album = "Seed Album".into();
        let mut near = track("/m/b/1.mp3");
        near.title = "Near".into();
        near.genre = "Drone".into();
        near.artist = "Other".into();
        near.album_artist = "Other".into();
        near.album = "Other".into();
        let mut far = track("/m/c/1.mp3");
        far.title = "Far".into();
        far.genre = "Techno".into();
        far.artist = "Third".into();
        far.album_artist = "Third".into();
        far.album = "Third".into();
        let p = projection(&[seed, near, far]);
        let seed_row = row_of(&p, "Seed");

        let ids = |title: &str| p.db_id[row_of(&p, title) as usize];
        let out = vote(
            &p,
            seed_row,
            &[(ids("Seed"), 1.0), (ids("Near"), 0.8), (ids("Far"), -0.5)],
            &[],
            5,
        );
        assert_eq!(out.len(), 2, "both neighbours are candidates");
        assert_eq!(out[0].genre, "Drone");
        assert_eq!(out[0].acoustic, 1);
        assert_eq!(out[0].score, 1.0, "the negative neighbour weighs nothing");
        assert_eq!(out[1].genre, "Techno");
        assert_eq!(out[1].score, 0.);
        assert!(
            !out.iter().any(|s| s.genre == "Ambient"),
            "the seed's own value never appears"
        );
    }

    /// A list value is as many votes as it has parts, and the spelling the
    /// most rows use is the one that comes back.
    #[test]
    fn list_values_split_and_the_common_spelling_wins() {
        let mut seed = track("/m/a/1.mp3");
        seed.title = "Seed".into();
        seed.genre = String::new();
        let mut both = track("/m/a/2.mp3");
        both.genre = "Shoegaze; Dream Pop".into();
        let mut lower = track("/m/a/3.mp3");
        lower.genre = "shoegaze".into();
        let mut lower_too = track("/m/a/4.mp3");
        lower_too.genre = "shoegaze".into();
        let p = projection(&[seed, both, lower, lower_too]);
        let seed_row = row_of(&p, "Seed");

        let out = vote(&p, seed_row, &[], &[], 5);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].genre, "shoegaze", "two rows spell it lowercase");
        assert_eq!(out[0].album, 3, "the list row votes for both its values");
        assert_eq!(out[1].genre, "Dream Pop");
    }

    /// Aliases fold two spellings into one candidate under the canonical
    /// display. The map is process-global, so the test clears it after.
    #[test]
    fn aliases_fold_into_one_candidate() {
        let mut seed = track("/m/a/1.mp3");
        seed.title = "Seed".into();
        seed.genre = String::new();
        let mut one = track("/m/a/2.mp3");
        one.genre = "dnb-suggest".into();
        let mut two = track("/m/a/3.mp3");
        two.genre = "d&b-suggest".into();
        let p = projection(&[seed, one, two]);
        let seed_row = row_of(&p, "Seed");

        assert_eq!(vote(&p, seed_row, &[], &[], 5).len(), 2);
        crate::genre::set_aliases(HashMap::from([
            ("dnb-suggest".to_string(), "Drum & Bass Suggest".to_string()),
            ("d&b-suggest".to_string(), "Drum & Bass Suggest".to_string()),
        ]));
        let out = vote(&p, seed_row, &[], &[], 5);
        crate::genre::set_aliases(HashMap::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].genre, "Drum & Bass Suggest");
        assert_eq!(out[0].album, 2);
        assert_eq!(out[0].score, 1.0);
    }

    /// The cap bounds the list, and a cap of nothing asks for nothing.
    #[test]
    fn the_cap_holds() {
        let mut rows = Vec::new();
        let mut seed = track("/m/a/0.mp3");
        seed.title = "Seed".into();
        seed.genre = String::new();
        rows.push(seed);
        for i in 0..5 {
            let mut row = track(&format!("/m/a/{}.mp3", i + 1));
            row.genre = format!("Genre {i}");
            rows.push(row);
        }
        let p = projection(&rows);
        let seed_row = row_of(&p, "Seed");

        assert_eq!(vote(&p, seed_row, &[], &[], 5).len(), 5);
        assert_eq!(vote(&p, seed_row, &[], &[], 2).len(), 2);
        assert!(vote(&p, seed_row, &[], &[], 0).is_empty());
    }

    /// A lookup value is a voter like the rest: it outweighs one album
    /// sibling, loses to two, and is flagged so a panel can say where it
    /// came from.
    #[test]
    fn lookup_contributes() {
        let seed = || {
            let mut seed = track("/m/a/1.mp3");
            seed.title = "Seed".into();
            seed.genre = String::new();
            seed
        };
        let p = projection(&[seed(), track("/m/a/2.mp3")]);
        let seed_row = row_of(&p, "Seed");

        let out = vote(&p, seed_row, &[], &["Post-Rock; Slowcore".into()], 5);
        assert_eq!(out[0].genre, "Post-Rock");
        assert!(out[0].lookup);
        assert_eq!(out[0].album, 0);
        assert_eq!(out[2].genre, "Shoegaze", "the one sibling is outweighed");
        assert!(!out[2].lookup);

        // Two siblings put the tags back on top.
        let p = projection(&[seed(), track("/m/a/2.mp3"), track("/m/a/3.mp3")]);
        let seed_row = row_of(&p, "Seed");
        let out = vote(&p, seed_row, &[], &["Post-Rock".into()], 5);
        assert_eq!(out[0].genre, "Shoegaze");
        assert_eq!(out[0].album, 2);
    }

    /// Turning a source's weight off takes its candidates off the ballot
    /// rather than scoring them at zero, which is what lets the probe
    /// measure one source at a time.
    #[test]
    fn a_zero_weight_turns_its_source_off() {
        let mut seed = track("/m/album/1.mp3");
        seed.title = "Seed".into();
        seed.genre = String::new();
        let sibling = track("/m/album/2.mp3");
        let mut elsewhere = track("/m/other/1.mp3");
        elsewhere.album = "Other".into();
        elsewhere.genre = "Dream Pop".into();
        let p = projection(&[seed, sibling, elsewhere]);
        let seed_row = row_of(&p, "Seed");

        let album_only = Weights {
            artist: 0.,
            ..Weights::default()
        };
        let out = vote_weighted(&p, seed_row, &[], &[], 5, album_only);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].genre, "Shoegaze");

        let artist_only = Weights {
            album: 0.,
            ..Weights::default()
        };
        let out = vote_weighted(&p, seed_row, &[], &[], 5, artist_only);
        assert_eq!(out.len(), 2, "the album sibling votes as an artist one now");
        assert_eq!(out.iter().map(|s| s.album).sum::<usize>(), 0);
    }

    /// The acoustic path over a library with no vectors at all: an empty
    /// table is a vote with no neighbours, not a panic.
    #[test]
    fn suggest_degrades_when_there_are_no_vectors() {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        let mut seed = track("/m/a/1.mp3");
        seed.title = "Seed".into();
        seed.genre = String::new();
        store::insert_batch(&mut conn, &[seed, track("/m/a/2.mp3")]).unwrap();
        let p = Projection::load_serial(&conn, false).unwrap();
        let seed_row = row_of(&p, "Seed");

        let out = suggest(&conn, "nothing-here", &p, seed_row, &[], 5);
        assert_eq!(out[0].genre, "Shoegaze");
        assert!(suggest(&conn, "nothing-here", &p, 999, &[], 5).is_empty());
    }

    /// The head of a score map is the best of it, ties by id, and a map
    /// shorter than the cap comes back sorted whole.
    #[test]
    fn nearest_takes_the_head() {
        let scored = vec![(3, 0.1), (1, 0.9), (2, 0.9), (4, -0.2)];
        assert_eq!(nearest(scored.clone(), 2), [(1, 0.9), (2, 0.9)]);
        assert_eq!(nearest(scored, 10).len(), 4);
        assert!(nearest(Vec::new(), 4).is_empty());
    }
}
