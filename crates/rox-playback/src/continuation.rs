//! Queue continuation (ADR 17): what plays when the queue runs dry.
//!
//! A provider is a selection strategy: it returns an ordered batch of library
//! ids, and the player appends it through the ordinary queue commands (ADR
//! 16). Nothing here opens a file or knows the engine exists.
//!
//! One provider at a time. An empty batch ends playback and never means "ask
//! another one"; unlike the online providers (ADR 14), tastes don't fall back
//! to each other. Blocking store queries, run on the background executor.

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use rox_library::rusqlite::Connection;
use rox_library::{embeddings, listens, song, store};

use crate::engine::{reservoir, shuffle_head, shuffle_slice};

/// Big enough to ask once an album, small enough to stay a readable stretch
/// of the queue.
pub const BATCH: usize = 20;

/// ADR 17's floor: one track of slack for the query, one for the gapless
/// boundary already opened.
pub const FLOOR: usize = 2;

/// Draws come from this many times the count, shuffled, so two sessions off
/// one seed differ while the ranking still decides what's in the running.
const BAND: usize = 4;

/// Which strategy fills the queue when it runs dry. Read leniently below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    /// The view play started in, then the rest of the library. The default (ADR 17).
    #[default]
    Continue,
    /// Never-played first, recent listens last (ADR 11).
    Weighted,
}

/// Unknown words read as the default: a settings shard that won't parse is
/// reset whole, taking the volume and saved queue with it. "radio" lands here
/// too; radio is what Similar shuffle does (see [`provider`]). By hand because
/// `serde(other)` only covers tagged enums.
impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Mode, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            Some("off") => Mode::Off,
            Some("weighted") => Mode::Weighted,
            _ => Mode::Continue,
        })
    }
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Continue => "Continue",
            Mode::Weighted => "Weighted",
        }
    }

    pub const ALL: [Mode; 3] = [Mode::Off, Mode::Continue, Mode::Weighted];
}

/// Where the playing context came from. Set when playback starts.
#[derive(Clone, Default)]
pub enum Scope {
    /// No list: a file open, a drop, a restored queue, the random button.
    #[default]
    Library,
    /// A browse view's ids in order. The library panel windows big views, so this
    /// is how continuation finds the rows below the window.
    View(Arc<Vec<i64>>),
}

pub struct Seed {
    /// None when the file isn't in the library.
    pub track: Option<i64>,
    pub scope: Scope,
    /// Every track this session has held, oldest first, queued by hand
    /// included: queue metal over a country context and the pool should follow
    /// the metal.
    pub recent: Vec<i64>,
    pub count: usize,
    /// The model an acoustic draw scores against, fixed when the batch was asked
    /// for so it matches the model the queue was ordered by.
    pub model: String,
}

impl Seed {
    /// Nothing in here comes back while the pool holds anything else.
    fn seen(&self) -> HashSet<i64> {
        self.recent.iter().copied().collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub id: i64,
    /// None lets the player fill it from the library at insert time.
    pub group: Option<u64>,
}

impl Pick {
    fn ungrouped(id: i64) -> Pick {
        Pick { id, group: None }
    }
}

/// One implementation active at a time, per [`Mode`].
pub trait Provider: Send {
    /// Blocking; called on the background executor.
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick>;
}

/// How the queue is ordered when a refill is asked for. Checked again on
/// landing: a batch drawn for one order is wrong for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Browse,
    Random,
    Similar,
}

/// The strategy for a mode, None while off. Built at the point of use so a
/// mode switched mid-query can't leave a live provider behind.
///
/// `order` takes the draw over the mode: Similar shuffle refills with radio,
/// and Random shuffle under Continue refills with a shuffle of the rest.
/// Weighted is already a shuffle and stays.
pub fn provider(mode: Mode, order: Order) -> Option<Box<dyn Provider>> {
    match (mode, order) {
        (Mode::Off, _) => None,
        (_, Order::Similar) => Some(Box::new(Radio)),
        (Mode::Continue, Order::Random) => Some(Box::new(Shuffled)),
        (Mode::Continue, Order::Browse) => Some(Box::new(Browse)),
        (Mode::Weighted, _) => Some(Box::new(Weighted)),
    }
}

/// Resume `order` after the *last* seen position, skipping anything seen. The
/// last, so a window that started mid-view carries on below it rather than
/// back into rows the listener skipped.
fn resume(order: &[i64], seen: &HashSet<i64>, count: usize) -> Vec<i64> {
    let from = order
        .iter()
        .rposition(|id| seen.contains(id))
        .map(|at| at + 1)
        .unwrap_or(0);
    order[from.min(order.len())..]
        .iter()
        .copied()
        .filter(|id| !seen.contains(id))
        .take(count)
        .collect()
}

/// Resume the browse order (#37): the view, then the rest of the library.
/// Widening past the view is the same taste asked twice, not a fallback.
struct Browse;

impl Provider for Browse {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        let seen = seed.seen();
        let mut out = Vec::new();
        if let Scope::View(order) = &seed.scope {
            out.extend(resume(order, &seen, seed.count));
        }
        if out.len() >= seed.count {
            return out.into_iter().map(Pick::ungrouped).collect();
        }
        let Ok(all) = store::all_ids(conn) else {
            return out.into_iter().map(Pick::ungrouped).collect();
        };
        // The view's picks count as seen, or a track in both lists comes back twice.
        let mut seen = seen;
        seen.extend(out.iter().copied());
        out.extend(resume(&all, &seen, seed.count - out.len()));
        // The whole library played: go round again rather than fall silent.
        if out.is_empty() {
            out.extend(all.into_iter().take(seed.count));
        }
        out.into_iter().map(Pick::ungrouped).collect()
    }
}

/// The browse order's shuffled twin, for Random shuffle: uniform draws from
/// the view's unplayed rows, then the library, nothing heard until both are
/// exhausted.
struct Shuffled;

impl Provider for Shuffled {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        let mut seen = seed.seen();
        let mut out = Vec::new();
        if let Scope::View(order) = &seed.scope {
            out = reservoir(
                order.iter().copied().filter(|id| !seen.contains(id)),
                seed.count,
            );
        }
        if out.len() >= seed.count {
            return out.into_iter().map(Pick::ungrouped).collect();
        }
        let Ok(all) = store::all_ids(conn) else {
            return out.into_iter().map(Pick::ungrouped).collect();
        };
        seen.extend(out.iter().copied());
        out.extend(reservoir(
            all.iter().copied().filter(|id| !seen.contains(id)),
            seed.count - out.len(),
        ));
        // Everything heard: go round again, shuffled.
        if out.is_empty() {
            out = reservoir(all, seed.count);
        }
        out.into_iter().map(Pick::ungrouped).collect()
    }
}

/// History-weighted draws (#38): never-played first, then longest unplayed.
/// With no history it's plain uniform random, not a special case.
struct Weighted;

impl Provider for Weighted {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        weighted_ids(conn, &seed.seen(), seed.count)
            .into_iter()
            .map(Pick::ungrouped)
            .collect()
    }
}

/// Bare ids, so the radio can top up its own batch.
fn weighted_ids(conn: &Connection, seen: &HashSet<i64>, count: usize) -> Vec<i64> {
    let Ok(all) = store::all_ids(conn) else {
        return Vec::new();
    };
    let counts = listens::counts(conn).unwrap_or_default();
    let last = listens::last_played(conn).unwrap_or_default();
    let mut pool: Vec<i64> = all
        .iter()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    // Everything heard this session: play on rather than stop.
    if pool.is_empty() {
        pool = all;
    }
    let mut fresh: Vec<i64> = Vec::new();
    let mut played: Vec<i64> = Vec::new();
    for id in pool {
        if last.contains_key(&id) {
            played.push(id);
        } else {
            fresh.push(id);
        }
    }
    // Shuffled whole: the ids arrive in browse order, and the head of an
    // unshuffled tier would start every session on the same artist.
    shuffle_slice(&mut fresh);
    // Longest ago first, ties to the one played least.
    played.sort_by_key(|id| {
        (
            last.get(id).copied().unwrap_or(0),
            counts.get(id).copied().unwrap_or(0),
        )
    });
    fresh.truncate(count.max(1) * BAND);
    let mut out = fresh;
    if out.len() < count {
        // The unplayed tier ran short; fill from the played ranking, banded so a
        // thin library doesn't replay the same order every wrap.
        let want = (count - out.len()) * BAND;
        let mut rest: Vec<i64> = played.into_iter().take(want.max(1)).collect();
        shuffle_slice(&mut rest);
        out.extend(rest);
    }
    out.truncate(count);
    out
}

/// Radio (#39): keep drawing what sounds like the seed, library-wide, off
/// `embeddings::ranked`, which also marks down tracks at another tempo.
///
/// Selection only; Similar shuffle is the ordering. [`provider`] hands the
/// draw to radio whenever the queue is ordered by sound.
struct Radio;

impl Provider for Radio {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        let seen = seed.seen();
        let mut out = radio_ids(conn, seed, &seen);
        if out.len() < seed.count {
            // A thin pool widens into the weighted draw instead of ending playback.
            let mut seen = seen;
            seen.extend(out.iter().copied());
            out.extend(weighted_ids(conn, &seen, seed.count - out.len()));
        }
        out.into_iter().map(Pick::ungrouped).collect()
    }
}

/// Bounds the song guard's walk down the ranking; that far down nothing
/// sounds like the seed anyway.
const LOOKAHEAD: usize = 8;

fn radio_ids(conn: &Connection, seed: &Seed, seen: &HashSet<i64>) -> Vec<i64> {
    let Some(track) = seed.track else {
        return Vec::new();
    };
    let Ok(mut scored) = embeddings::ranked(conn, track, &seed.model) else {
        return Vec::new();
    };
    // Ties by id so the ranking is stable; the variety comes from the band.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut ids: Vec<i64> = scored
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| !seen.contains(id))
        .collect();
    let band = seed.count * BAND;
    let mut band_ids = one_per_song(conn, seed, &ids[..ids.len().min(band * LOOKAHEAD)], band);
    // The whole neighbourhood is one song: play it rather than end the session.
    if band_ids.is_empty() {
        ids.truncate(band);
        band_ids = ids;
    }
    shuffle_head(&mut band_ids, band);
    band_ids.truncate(seed.count);
    band_ids
}

/// One track per song (see [`rox_library::song`]), minus songs already
/// played. Without it seventeen rips of one song fill the band, since by sound
/// they really are the same track.
fn one_per_song(conn: &Connection, seed: &Seed, candidates: &[i64], want: usize) -> Vec<i64> {
    let mut lookup: Vec<i64> = candidates.to_vec();
    lookup.extend(seed.recent.iter().copied());
    let Ok(keys) = song::keys_for(conn, &lookup) else {
        return candidates.to_vec();
    };
    let blocked = song::keys_of(&keys, seed.recent.iter().copied());
    song::distinct(candidates, &keys, &blocked, want)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not the built-in model: the draw must follow the model the seed names.
    const MODEL: &str = "test-vectors-1";

    /// The same library under another model, which a draw must not wander into.
    const OTHER_MODEL: &str = "test-vectors-2";

    /// One track per album, so the ids come back in a predictable order.
    fn library(n: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        listens::init_schema(&conn).unwrap();
        embeddings::init_schema(&conn).unwrap();
        for i in 0..n {
            conn.execute(
                "INSERT INTO tracks (path, title, artist, album, album_artist, genre, year,
                    track_no, disc_no, duration_ms, size, mtime)
                 VALUES (?1, ?2, 'A', ?3, 'A', 'g', 0, 1, 1, 200000, 0, 0)",
                rox_library::rusqlite::params![
                    format!("/m/{i:03}.flac"),
                    format!("t{i:03}"),
                    format!("al{i:03}"),
                ],
            )
            .unwrap();
        }
        conn
    }

    fn ids(conn: &Connection) -> Vec<i64> {
        store::all_ids(conn).unwrap()
    }

    fn seed(scope: Scope, recent: Vec<i64>, count: usize) -> Seed {
        Seed {
            track: recent.last().copied(),
            scope,
            recent,
            count,
            model: MODEL.to_string(),
        }
    }

    fn picked(picks: Vec<Pick>) -> Vec<i64> {
        picks.into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn browse_resumes_the_view_below_the_window_that_played() {
        let conn = library(10);
        let all = ids(&conn);
        let view = Arc::new(all.clone());
        let played = all[2..6].to_vec();
        let batch = Browse.next(&conn, &seed(Scope::View(view), played, 3));
        assert_eq!(picked(batch), all[6..9].to_vec());
    }

    #[test]
    fn browse_widens_past_a_view_it_has_finished() {
        let conn = library(10);
        let all = ids(&conn);
        let view = Arc::new(all[..4].to_vec());
        let batch = Browse.next(&conn, &seed(Scope::View(view), all[..4].to_vec(), 3));
        assert_eq!(picked(batch), all[4..7].to_vec());
    }

    #[test]
    fn browse_with_no_view_walks_the_library() {
        let conn = library(6);
        let all = ids(&conn);
        let batch = Browse.next(&conn, &seed(Scope::Library, vec![all[1]], 2));
        assert_eq!(picked(batch), all[2..4].to_vec());
    }

    #[test]
    fn browse_goes_round_again_once_the_library_is_exhausted() {
        let conn = library(3);
        let all = ids(&conn);
        let batch = Browse.next(&conn, &seed(Scope::Library, all.clone(), 2));
        assert_eq!(picked(batch), all[..2].to_vec());
    }

    #[test]
    fn browse_on_an_empty_library_returns_nothing() {
        let conn = library(0);
        assert!(
            Browse
                .next(&conn, &seed(Scope::Library, Vec::new(), 5))
                .is_empty()
        );
    }

    #[test]
    fn weighted_draws_the_unheard_before_the_heard() {
        let conn = library(8);
        let all = ids(&conn);
        for id in &all[..6] {
            conn.execute(
                "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path)
                 VALUES (?1, 1000, 't', 'A', 'al', 'g', '/m/x.flac')",
                rox_library::rusqlite::params![id],
            )
            .unwrap();
        }
        let batch = picked(Weighted.next(&conn, &seed(Scope::Library, Vec::new(), 2)));
        assert_eq!(batch.len(), 2);
        for id in batch {
            assert!(all[6..].contains(&id), "a played track jumped the queue");
        }
    }

    #[test]
    fn weighted_with_no_history_still_fills_a_batch() {
        let conn = library(12);
        let batch = picked(Weighted.next(&conn, &seed(Scope::Library, Vec::new(), 5)));
        assert_eq!(batch.len(), 5);
        let unique: HashSet<i64> = batch.iter().copied().collect();
        assert_eq!(unique.len(), 5, "a batch never repeats a track");
    }

    /// Excluded even when the history table has never heard of them.
    #[test]
    fn weighted_skips_what_the_session_already_holds() {
        let conn = library(6);
        let all = ids(&conn);
        let held = all[..4].to_vec();
        let batch = picked(Weighted.next(&conn, &seed(Scope::Library, held.clone(), 2)));
        for id in batch {
            assert!(!held.contains(&id), "the session's own tracks came back");
        }
    }

    #[test]
    fn radio_falls_through_to_the_weighted_draw_unanalyzed() {
        let conn = library(8);
        let all = ids(&conn);
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], 3)));
        assert_eq!(batch.len(), 3);
        assert!(!batch.contains(&all[0]));
    }

    /// Picks come from the band nearest the seed, and from the model the seed
    /// names: the library is described twice here.
    #[test]
    fn radio_picks_out_of_the_band_nearest_the_seed() {
        const N: usize = 40;
        let conn = library(N);
        let all = ids(&conn);
        // Vectors around a circle: after standardization the dot product reads as
        // the angle. Track 0 is the seed.
        for (step, id) in all.iter().enumerate() {
            let angle = step as f32 / N as f32 * std::f32::consts::TAU;
            embeddings::upsert(&conn, *id, MODEL, &[angle.cos(), angle.sin()]).unwrap();
            // The other model steps seven at a time, so every neighbour is elsewhere.
            let angle = (step * 7 % N) as f32 / N as f32 * std::f32::consts::TAU;
            embeddings::upsert(&conn, *id, OTHER_MODEL, &[angle.cos(), angle.sin()]).unwrap();
        }
        let count = 2;
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], count)));
        assert_eq!(batch.len(), count);
        assert!(
            !batch.contains(&all[0]),
            "the seed came back as its own neighbour"
        );
        let band = count * BAND;
        let near: HashSet<i64> = all[1..=band / 2]
            .iter()
            .chain(&all[N - band / 2..])
            .copied()
            .collect();
        for id in batch {
            assert!(near.contains(&id), "a pick came from outside the band");
        }
    }

    /// The nearest tracks by vector sit half an octave off the seed's tempo, so
    /// a band of one passes over them to the nearest pair at the seed's tempo.
    #[test]
    fn radio_passes_over_the_nearest_track_at_the_wrong_tempo() {
        const N: usize = 40;
        let conn = library(N);
        let all = ids(&conn);
        for (step, id) in all.iter().enumerate() {
            let angle = step as f32 / N as f32 * std::f32::consts::TAU;
            embeddings::upsert(&conn, *id, MODEL, &[angle.cos(), angle.sin()]).unwrap();
        }
        let bpm = |id: i64, bpm: f32| {
            conn.execute(
                "UPDATE tracks SET bpm = ?2 WHERE id = ?1",
                rox_library::rusqlite::params![id, bpm],
            )
            .unwrap();
        };
        bpm(all[0], 140.0);
        bpm(all[1], 198.0);
        bpm(all[N - 1], 198.0);
        // Everything further out is unmeasured, itself a mismatch.
        for id in [all[2], all[3], all[N - 2], all[N - 3]] {
            bpm(id, 140.0);
        }

        let nearest = [all[1], all[N - 1]];
        let compatible: HashSet<i64> = [all[2], all[3], all[N - 2], all[N - 3]]
            .into_iter()
            .collect();
        for _ in 0..8 {
            let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], 1)));
            assert_eq!(batch.len(), 1);
            assert!(
                !nearest.contains(&batch[0]),
                "the draw took the nearest track despite its tempo"
            );
            assert!(
                compatible.contains(&batch[0]),
                "the draw wandered past the tracks that share the tempo"
            );
        }
    }

    /// The version pile: every copy is every other copy's nearest neighbour. The
    /// draw takes none of them past the seed.
    #[test]
    fn radio_takes_one_copy_of_a_song_the_library_holds_many_times() {
        const N: usize = 40;
        const COPIES: usize = 12;
        let conn = library(N);
        let all = ids(&conn);
        let versions = [
            "Barracuda",
            "Barracuda (Live)",
            "Barracuda (Live In Japan)",
            "Barracuda - Live",
            "Barracuda [2010 Remaster]",
        ];
        for (step, id) in all.iter().enumerate().take(COPIES) {
            conn.execute(
                "UPDATE tracks SET artist = 'Heart', title = ?2 WHERE id = ?1",
                rox_library::rusqlite::params![id, versions[step % versions.len()]],
            )
            .unwrap();
        }
        for (step, id) in all.iter().enumerate() {
            let vec = match step < COPIES {
                true => [1.0, step as f32 / 1000.0],
                false => {
                    let angle = step as f32 / N as f32 * std::f32::consts::TAU;
                    [angle.cos(), angle.sin()]
                }
            };
            embeddings::upsert(&conn, *id, MODEL, &vec).unwrap();
        }
        let count = 6;
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], count)));
        assert_eq!(batch.len(), count);
        let copies = batch.iter().filter(|id| all[..COPIES].contains(id)).count();
        assert_eq!(copies, 0, "the draw came back with the seed's own song");
    }

    /// A library of nothing but versions of one song still plays.
    #[test]
    fn radio_plays_a_library_of_one_song_rather_than_falling_silent() {
        let conn = library(6);
        let all = ids(&conn);
        for id in &all {
            conn.execute(
                "UPDATE tracks SET artist = 'Heart', title = 'Barracuda' WHERE id = ?1",
                rox_library::rusqlite::params![id],
            )
            .unwrap();
            embeddings::upsert(&conn, *id, MODEL, &[1.0, 0.0]).unwrap();
        }
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], 3)));
        assert_eq!(batch.len(), 3);
        assert!(!batch.contains(&all[0]));
    }

    /// Unknown modes degrade to the default instead of taking the shard down.
    #[test]
    fn a_mode_the_build_doesnt_know_reads_as_the_default() {
        for mode in Mode::ALL {
            let wire = serde_json::to_value(mode).unwrap();
            assert_eq!(serde_json::from_value::<Mode>(wire).unwrap(), mode);
        }
        for junk in [r#""shoutcast""#, "17", "null", "{}"] {
            assert_eq!(
                serde_json::from_str::<Mode>(junk).unwrap(),
                Mode::default(),
                "{junk} should have degraded rather than failed"
            );
        }
    }

    /// "radio" degrades to the default, Similar takes the draw whatever the mode,
    /// and Off still means off even with shuffle on.
    #[test]
    fn similar_order_takes_the_draw_rather_than_a_mode_of_its_own() {
        assert_eq!(
            serde_json::from_str::<Mode>(r#""radio""#).unwrap(),
            Mode::default()
        );
        assert!(provider(Mode::Off, Order::Similar).is_none());

        // Same mode and seed, two orders: browse carries on down the view, radio
        // (here the weighted fallback) doesn't.
        let conn = library(10);
        let all = ids(&conn);
        let view = Arc::new(all.clone());
        let played = all[2..6].to_vec();
        let ordered = provider(Mode::Continue, Order::Browse)
            .expect("continuation is on")
            .next(&conn, &seed(Scope::View(view.clone()), played.clone(), 3));
        assert_eq!(picked(ordered), all[6..9].to_vec(), "the browse resume");
        let by_sound = provider(Mode::Continue, Order::Similar)
            .expect("continuation is on")
            .next(&conn, &seed(Scope::View(view), played.clone(), 3));
        for id in picked(by_sound) {
            assert!(!played.contains(&id), "radio replayed the session");
        }
    }

    #[test]
    fn random_shuffle_draws_from_everything_unheard() {
        let conn = library(12);
        let all = ids(&conn);
        let view = Arc::new(all[..6].to_vec());
        let played = all[..4].to_vec();
        let shuffled = provider(Mode::Continue, Order::Random).expect("continuation is on");
        let batch =
            picked(shuffled.next(&conn, &seed(Scope::View(view.clone()), played.clone(), 5)));
        assert_eq!(batch.len(), 5);
        assert!(
            batch.contains(&all[4]) && batch.contains(&all[5]),
            "the view's remainder"
        );
        for id in &batch {
            assert!(!played.contains(id), "replayed the session");
        }
        let distinct: HashSet<i64> = batch.iter().copied().collect();
        assert_eq!(distinct.len(), batch.len(), "a pick came back twice");
        // Asked often enough, the draw covers the view's remainder, not just the
        // next rows.
        let mut landed: HashSet<i64> = HashSet::new();
        for _ in 0..64 {
            landed.extend(picked(
                shuffled.next(&conn, &seed(Scope::View(view.clone()), played.clone(), 1)),
            ));
        }
        assert_eq!(landed, [all[4], all[5]].into_iter().collect());
        let again = picked(shuffled.next(&conn, &seed(Scope::Library, all.clone(), 3)));
        assert_eq!(again.len(), 3);
    }

    #[test]
    fn resume_walks_from_the_last_seen_position() {
        let order = vec![1, 2, 3, 4, 5];
        let none = HashSet::new();
        assert_eq!(resume(&order, &none, 2), vec![1, 2]);
        let mid: HashSet<i64> = [2, 3].into_iter().collect();
        assert_eq!(resume(&order, &mid, 9), vec![4, 5]);
        let last: HashSet<i64> = [5].into_iter().collect();
        assert!(resume(&order, &last, 3).is_empty());
        // A gap in the middle is skipped, not replayed.
        let gappy: HashSet<i64> = [1, 4].into_iter().collect();
        assert_eq!(resume(&order, &gappy, 9), vec![5]);
    }
}
