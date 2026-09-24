//! What genre an untagged track probably is, voted on by the library the
//! user already tagged: the rest of the album, the rest of the artist, and
//! the acoustic neighbours, with the evidence kept beside each result. Pure
//! over a projection snapshot; only [`suggest`] touches SQLite.
//!
//! The weights are lopsided on purpose. `examples/genreprobe.rs` hides known
//! genres on a 53k-track library: album siblings alone score 98.6% top-1,
//! artist siblings 87.7%, acoustic neighbours 58.4%. A seed can have
//! hundreds of artist siblings, so at equal weights the artist outvotes the
//! album; weighting a whole discography to roughly tie one album lifts the
//! combination to 98.8% at full coverage. The lookup weight is unmeasured.
//!
//! The tally is per split, alias-resolved, case-folded value, and the
//! winning spelling is the one most rows use.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::projection::Projection;

pub const NEIGHBOURS: usize = 24;

/// The unit the other weights are priced against.
pub const ALBUM_WEIGHT: f32 = 8.0;

/// Per artist row outside the album: eighty tie one album.
pub const ARTIST_WEIGHT: f32 = 0.1;

/// Times the cosine: the whole neighbour set at full agreement is worth
/// about half an album sibling.
pub const ACOUSTIC_WEIGHT: f32 = 0.15;

/// One and a half album siblings: beats one, loses to two.
pub const LOOKUP_WEIGHT: f32 = ALBUM_WEIGHT * 1.5;

/// A zero weight takes its source off the ballot entirely, so it can't
/// sway tie-breaks.
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

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// One value, never a "; " list, alias-resolved.
    pub genre: String,
    /// This candidate's share of the total weight.
    pub score: f32,
    /// Same album symbol and same folder.
    pub album: usize,
    /// Artist or album artist match, not already counted under album.
    pub artist: usize,
    pub acoustic: usize,
    pub lookup: bool,
}

#[derive(Default)]
struct Tally {
    weight: f32,
    album: usize,
    artist: usize,
    acoustic: usize,
    lookup: bool,
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

    /// Ties go to the lexicographically smaller, so runs agree.
    fn display(&self) -> String {
        self.spellings
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
    }
}

/// Live rows with an empty genre, in projection order.
pub fn untagged(projection: &Projection) -> Vec<u32> {
    // Per distinct value, not per row.
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

/// `neighbours` is (db id, score), nearest first, as `embeddings::ranked`
/// returns them. `lookup` may hold "; " lists. The seed never votes.
pub fn vote(
    projection: &Projection,
    row: u32,
    neighbours: &[(i64, f32)],
    lookup: &[String],
    cap: usize,
) -> Vec<Suggestion> {
    vote_weighted(projection, row, neighbours, lookup, cap, Weights::default())
}

/// One pass over the columns, matching neighbours through a small id map:
/// the projection has no db-id index, and building one costs a hash entry
/// per track.
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

    // An empty artist would make every artistless row a sibling. The album is
    // bounded by the folder already.
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
    // The two tables intern separately, so match by name, not symbol.
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
        // A row can be both sibling and neighbour and counts as both, so the
        // evidence adds up to the score.
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
            // All-negative neighbours still name their candidates, at zero.
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

fn symbol_hits(lower: &[String], wanted: &HashSet<&str>) -> Vec<bool> {
    if wanted.is_empty() {
        return vec![false; lower.len()];
    }
    lower
        .iter()
        .map(|name| !name.is_empty() && wanted.contains(name.as_str()))
        .collect()
}

/// Memoized per genre symbol: resolving takes the alias lock and allocates.
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
            // "Rock; rock" is one vote.
            if !values.iter().any(|(_, seen)| *seen == key) {
                values.push((display, key));
            }
        }
        cache[sym] = Some(values);
    }
    cache[sym].as_deref().unwrap_or(&[])
}

/// [`vote`] with the top NEIGHBOURS from `embeddings::ranked`. Errors and
/// missing vectors degrade to no neighbours.
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

/// The best `k`, nearest first, ties by id. Selects before sorting so the
/// unread tail costs nothing.
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

    fn row_of(p: &Projection, title: &str) -> u32 {
        (0..p.len()).find(|&i| p.title.get(i) == title).unwrap() as u32
    }

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

    #[test]
    fn album_siblings_outweigh_artist_siblings() {
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
        let album = 2. * ALBUM_WEIGHT;
        let artist = 3. * ARTIST_WEIGHT;
        assert!((out[0].score - album / (album + artist)).abs() < 1e-6);

        let p = projection(&library("/m/reissue"));
        let seed = row_of(&p, "Seed");
        let out = vote(&p, seed, &[], &[], 5);
        assert_eq!(out[0].genre, "Dream Pop", "only artist siblings are left");
        assert_eq!(out[0].album, 0);
    }

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

    /// The map is process-global, so the test clears it after.
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

        let p = projection(&[seed(), track("/m/a/2.mp3"), track("/m/a/3.mp3")]);
        let seed_row = row_of(&p, "Seed");
        let out = vote(&p, seed_row, &[], &["Post-Rock".into()], 5);
        assert_eq!(out[0].genre, "Shoegaze");
        assert_eq!(out[0].album, 2);
    }

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

    #[test]
    fn nearest_takes_the_head() {
        let scored = vec![(3, 0.1), (1, 0.9), (2, 0.9), (4, -0.2)];
        assert_eq!(nearest(scored.clone(), 2), [(1, 0.9), (2, 0.9)]);
        assert_eq!(nearest(scored, 10).len(), 4);
        assert!(nearest(Vec::new(), 4).is_empty());
    }
}
