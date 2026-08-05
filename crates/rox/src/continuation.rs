//! Queue continuation (ADR 17): what plays when the queue runs dry.
//!
//! A provider is a selection strategy, not a source of audio. It answers one
//! question, "what plays next", with an ordered batch of library ids, and the
//! player appends that batch into the running engine through the ordinary
//! queue commands (ADR 16). Nothing here opens a file, and nothing here knows
//! the engine exists.
//!
//! Exactly one provider is active at a time. An empty batch means there is
//! nothing left to continue with and playback ends; it never means "ask the
//! next one". That's the deliberate difference from the online enrichment
//! chain in `providers/` (ADR 14), which races several services for the best
//! answer to the same question. Continuation is a taste, and tastes don't
//! fall back to each other.
//!
//! The calls are blocking store queries. The player runs them on the
//! background executor, which is why every provider takes a plain
//! `&Connection` and nothing in this module touches gpui.

use std::collections::HashSet;
use std::sync::Arc;

use rox_library::rusqlite::Connection;
use rox_library::{embeddings, listens, store};
use rox_playback::engine::shuffle_slice;
use serde::{Deserialize, Serialize};

use crate::player::shuffle_head;

/// How many tracks a batch asks for. Big enough that a slow provider gets
/// asked once an album rather than once a track, small enough that what
/// continuation added stays a readable stretch of the queue rather than a
/// wall the user has to clear.
pub const BATCH: usize = 20;

/// How close to the end of the upcoming portion the pump fires, in tracks.
/// Two is the ADR's floor: one track of slack for the query, one for the
/// gapless boundary the decode cursor has already opened.
pub const FLOOR: usize = 2;

/// How much wider than the batch a strategy looks before it picks. The
/// draw comes from this many times the requested count, shuffled among
/// themselves, so two sessions off the same seed don't play the same list
/// in the same order while the ranking behind the band still decides which
/// tracks are in the running at all.
const BAND: usize = 4;

/// Where the playing context came from, so a provider can carry on from it
/// rather than guess. Set when playback starts and held for the session.
#[derive(Clone, Default)]
pub enum Scope {
    /// Nothing named a list: an OS file open, a drop, a restored queue, the
    /// random button. The library at large is the pool.
    #[default]
    Library,
    /// A browse view's track ids in the order it was showing them. The
    /// library panel windows a big view rather than queueing all of it, so
    /// this is how continuation finds the rows below the window.
    View(Arc<Vec<i64>>),
}

/// What the player hands a provider (ADR 17).
pub struct Seed {
    /// The track playing when the queue ran low. None when the file isn't in
    /// the library, which is the case a radio has no answer for.
    pub track: Option<i64>,
    /// The view play started in.
    pub scope: Scope,
    /// Every track this session has held, oldest first: what already played,
    /// what's still upcoming, and what was queued by hand. In the contract on
    /// purpose. Queue metal over a country context and the pool should follow
    /// the metal, and nothing else in the seed would say so.
    pub recent: Vec<i64>,
    /// How many tracks to return. A provider may return fewer.
    pub count: usize,
}

impl Seed {
    /// The recent plays as a set, which is how every provider reads them:
    /// nothing in here comes back in a batch while the pool still holds
    /// anything else.
    fn seen(&self) -> HashSet<i64> {
        self.recent.iter().copied().collect()
    }
}

/// One track a provider picked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub id: i64,
    /// The album group the entry carries, or None to let the player fill it
    /// in from the library at insert time. A strategy that wants its picks
    /// to stand alone rather than splice as an album says so here; the ones
    /// below all leave it to the library, which is where album membership is
    /// actually known.
    pub group: Option<u64>,
}

impl Pick {
    /// A pick that takes the library's own grouping.
    fn ungrouped(id: i64) -> Pick {
        Pick { id, group: None }
    }
}

/// The continuation seam. One implementation is active at a time; which one
/// is [`Mode`], the user's pick.
pub trait Provider: Send {
    /// The next batch, in play order. Blocking store queries: the player
    /// calls this on the background executor.
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick>;
}

/// Which strategy feeds the queue when it runs dry.
///
/// A real enum rather than the wire string the loop mode carries, because
/// the modes are a closed set the menus enumerate; the read below is what
/// keeps that from making the settings file brittle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The queue ends when it ends, which is how rox behaved before any of
    /// this existed.
    Off,
    /// Carry on down the list play started in, then the rest of the library
    /// behind it. The default: continuation is on out of the box (ADR 17), a
    /// local player that goes silent mid-flow feels broken, and resuming the
    /// browse order is the least surprising thing it can do.
    #[default]
    Continue,
    /// Draw from the whole library, never-played first and recent listens
    /// last. What the play history (ADR 11) is for.
    Weighted,
}

/// Anything this doesn't recognize reads as the default rather than failing.
/// A derived read would refuse a mode a newer build wrote, and a settings
/// shard that won't parse is reset whole, so one unknown word here would cost
/// the volume, the loop mode, and the saved queue with it. Written by hand
/// rather than with `serde(other)`, which only covers tagged enums.
///
/// "radio" is deliberately not listed. It was a mode of its own until the
/// radio draw became what Similar shuffle does when it runs out (see
/// [`Mode::provider`]), so a settings file carrying it lands on the default
/// and the listener's radio comes back from the shuffle order instead.
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
    /// The label the mode menu shows.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Continue => "Continue",
            Mode::Weighted => "Weighted",
        }
    }

    /// Every mode in menu order.
    pub const ALL: [Mode; 3] = [Mode::Off, Mode::Continue, Mode::Weighted];

    /// The strategy behind the pick, None while continuation is off.
    /// Built on the background executor, at the point of use, so a mode
    /// switched during a query can't leave a live provider behind.
    ///
    /// `similar` is whether the queue is currently ordered by sound (shuffle
    /// on, in [`crate::settings::ShuffleMode::Similar`]), and it takes the
    /// draw over whatever mode is picked. Radio isn't a strategy the listener
    /// chooses any more: a queue ordered by what sounds alike and then
    /// refilled from browse order would answer two different questions in one
    /// session, so the refill follows the order. Turning on Similar shuffle
    /// is turning on radio, which is what it looked like it did anyway.
    pub fn provider(self, similar: bool) -> Option<Box<dyn Provider>> {
        match self {
            Mode::Off => None,
            _ if similar => Some(Box::new(Radio)),
            Mode::Continue => Some(Box::new(Browse)),
            Mode::Weighted => Some(Box::new(Weighted)),
        }
    }
}

/// Resume `order` past wherever the session got to: the last position any
/// seen track occupies, and everything unseen after it. Nothing already seen
/// comes back.
///
/// The resume point is the *last* seen position rather than the first,
/// because a window that started mid-view and played to its end must carry on
/// below it. Taking the first would walk back up into the rows above the
/// click, which the listener already skipped past on purpose.
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

/// Resume the browse order (#37): the view play started in, then the rest of
/// the library behind it.
///
/// Widening past the view is this provider's own pool growing, not a fallback
/// to another strategy. The ADR's "an empty batch ends playback" rule is
/// about not racing tastes against each other; a list that runs out and a
/// library that runs out are the same taste asked twice.
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
        // The view's picks count as seen for the library pass, or a track
        // sitting in both lists would come back twice in one batch.
        let mut seen = seen;
        seen.extend(out.iter().copied());
        out.extend(resume(&all, &seen, seed.count - out.len()));
        // The library ran out too. Everything the session could play, it has
        // played, so go round again from the top rather than fall silent:
        // "no repeats until the library is exhausted" is the promise, and
        // this is what exhausted looks like on a twelve-track library.
        if out.is_empty() {
            out.extend(all.into_iter().take(seed.count));
        }
        out.into_iter().map(Pick::ungrouped).collect()
    }
}

/// History-weighted draws (#38): never-played tracks tier first, then the
/// longest unplayed, with recent listens sinking to the back.
///
/// The exact falloff is implementation detail and this is the crude version
/// on purpose: two tiers, ordered inside the second by how long ago and how
/// often. With no history at all every track lands in the first tier and the
/// shuffle over it is plain uniform random, which is the degradation the
/// issue asks for and not a special case in the code.
struct Weighted;

impl Provider for Weighted {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        weighted_ids(conn, &seed.seen(), seed.count)
            .into_iter()
            .map(Pick::ungrouped)
            .collect()
    }
}

/// The weighted draw as bare ids, so the radio can top its own batch up with
/// it without going through a second provider.
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
    // Everything's been heard this session. Same call the browse provider
    // makes at the end of the library: play on rather than stop.
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
    // The tier is shuffled whole rather than windowed, because the ids
    // arrive in browse order: taking the head of an unshuffled tier would
    // mean every session starts at the same artist.
    shuffle_slice(&mut fresh);
    // Longest ago first, ties to the one played least. Both ascending, so
    // the record from last year outranks the one from this morning.
    played.sort_by_key(|id| {
        (
            last.get(id).copied().unwrap_or(0),
            counts.get(id).copied().unwrap_or(0),
        )
    });
    fresh.truncate(count.max(1) * BAND);
    let mut out = fresh;
    if out.len() < count {
        // The unplayed tier ran short, so the rest of the batch comes off
        // the front of the played ranking. Banded like the tier above it,
        // so a thin library doesn't replay the same five tracks in the same
        // order every time it wraps.
        let want = (count - out.len()) * BAND;
        let mut rest: Vec<i64> = played.into_iter().take(want.max(1)).collect();
        shuffle_slice(&mut rest);
        out.extend(rest);
    }
    out.truncate(count);
    out
}

/// Radio (#39): keep drawing what sounds like the seed, library-wide.
///
/// Off the acoustic vectors rather than the genre and artist strings the
/// issue first proposed, because #40 landed: `embeddings::scores` answers the
/// same question without trusting the least consistent field in any real
/// library.
///
/// This is selection, and the Similar shuffle mode in the player is ordering:
/// radio decides which tracks join the queue, Similar decides what order the
/// upcoming portion plays them in. Neither does the other's job, which is why
/// the two used to be separate picks in two separate menus. Nobody could tell
/// them apart, and they were only ever wanted together, so this one lost its
/// menu entry: [`Mode::provider`] hands the draw to radio whenever the queue
/// is being ordered by sound.
struct Radio;

impl Provider for Radio {
    fn next(&self, conn: &Connection, seed: &Seed) -> Vec<Pick> {
        let seen = seed.seen();
        let mut out = radio_ids(conn, seed, &seen);
        if out.len() < seed.count {
            // A thin pool widens instead of ending playback: an unanalyzed
            // library, a seed with no vector, or a neighbourhood the session
            // has already played through. The weighted draw is the floor
            // under every strategy, so the music keeps going while the
            // analysis pass catches up.
            let mut seen = seen;
            seen.extend(out.iter().copied());
            out.extend(weighted_ids(conn, &seen, seed.count - out.len()));
        }
        out.into_iter().map(Pick::ungrouped).collect()
    }
}

/// The acoustic half of the radio draw, empty when there's nothing to score
/// against.
fn radio_ids(conn: &Connection, seed: &Seed, seen: &HashSet<i64>) -> Vec<i64> {
    let Some(track) = seed.track else {
        return Vec::new();
    };
    let Ok(mut scored) = embeddings::scores(conn, track, crate::embeddings::MODEL) else {
        return Vec::new();
    };
    // Nearest first, ties by id so the ranking is the same between calls;
    // the variety comes from the band below, not from an unstable sort.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut ids: Vec<i64> = scored
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| !seen.contains(id))
        .collect();
    shuffle_head(&mut ids, seed.count * BAND);
    ids.truncate(seed.count);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library of `n` tracks, one per album so the ids come back in a
    /// predictable order, plus the tables the providers read.
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
        }
    }

    fn picked(picks: Vec<Pick>) -> Vec<i64> {
        picks.into_iter().map(|p| p.id).collect()
    }

    /// The layer-one provider carries on down the view from where the
    /// session got to, not from the top of it and not from the library.
    #[test]
    fn browse_resumes_the_view_below_the_window_that_played() {
        let conn = library(10);
        let all = ids(&conn);
        let view = Arc::new(all.clone());
        // Play started at the third row and ran to the sixth.
        let played = all[2..6].to_vec();
        let batch = Browse.next(&conn, &seed(Scope::View(view), played, 3));
        assert_eq!(picked(batch), all[6..9].to_vec());
    }

    /// A view that's been played through hands over to the library rather
    /// than ending the session, and the handover skips what's already been
    /// heard.
    #[test]
    fn browse_widens_past_a_view_it_has_finished() {
        let conn = library(10);
        let all = ids(&conn);
        // The view is only the first four tracks, and all four have played.
        let view = Arc::new(all[..4].to_vec());
        let batch = Browse.next(&conn, &seed(Scope::View(view), all[..4].to_vec(), 3));
        assert_eq!(picked(batch), all[4..7].to_vec());
    }

    /// With no view behind it (a drop, an OS file open, the random button)
    /// the library itself is the order.
    #[test]
    fn browse_with_no_view_walks_the_library() {
        let conn = library(6);
        let all = ids(&conn);
        let batch = Browse.next(&conn, &seed(Scope::Library, vec![all[1]], 2));
        assert_eq!(picked(batch), all[2..4].to_vec());
    }

    /// Every track played and the library still has to keep playing: it
    /// wraps rather than returning the empty batch that ends the session.
    #[test]
    fn browse_goes_round_again_once_the_library_is_exhausted() {
        let conn = library(3);
        let all = ids(&conn);
        let batch = Browse.next(&conn, &seed(Scope::Library, all.clone(), 2));
        assert_eq!(picked(batch), all[..2].to_vec());
    }

    /// Nothing to draw from is the one case that really does end playback.
    #[test]
    fn browse_on_an_empty_library_returns_nothing() {
        let conn = library(0);
        assert!(Browse
            .next(&conn, &seed(Scope::Library, Vec::new(), 5))
            .is_empty());
    }

    /// A listen sinks its track behind everything never played, whatever
    /// order the library holds them in.
    #[test]
    fn weighted_draws_the_unheard_before_the_heard() {
        let conn = library(8);
        let all = ids(&conn);
        // Mark the first six played, leaving two the session has never heard.
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

    /// With no listens at all every track is equally unheard, so the draw is
    /// plain uniform random over the library rather than a special case.
    #[test]
    fn weighted_with_no_history_still_fills_a_batch() {
        let conn = library(12);
        let batch = picked(Weighted.next(&conn, &seed(Scope::Library, Vec::new(), 5)));
        assert_eq!(batch.len(), 5);
        let unique: HashSet<i64> = batch.iter().copied().collect();
        assert_eq!(unique.len(), 5, "a batch never repeats a track");
    }

    /// The session's own plays are excluded even when the history table has
    /// never heard of them, which is what keeps a fresh library from
    /// stuttering on the track it just played.
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

    /// A library with no vectors is exactly the case radio can't answer, and
    /// the answer is to keep playing off the weighted draw rather than stop.
    #[test]
    fn radio_falls_through_to_the_weighted_draw_unanalyzed() {
        let conn = library(8);
        let all = ids(&conn);
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], 3)));
        assert_eq!(batch.len(), 3);
        assert!(!batch.contains(&all[0]));
    }

    /// With vectors in the table the picks come off the acoustic ranking,
    /// out of the band at the near end of it rather than from anywhere in
    /// the library.
    #[test]
    fn radio_picks_out_of_the_band_nearest_the_seed() {
        const N: usize = 40;
        let conn = library(N);
        let all = ids(&conn);
        // Vectors around a circle, so the scoring has something real to
        // rank: the standardization centres the corpus and normalizes it,
        // which leaves the dot product reading as the angle between two
        // tracks. Track 0 is the seed, and distance grows either way around
        // until the opposite side of the circle.
        for (step, id) in all.iter().enumerate() {
            let angle = step as f32 / N as f32 * std::f32::consts::TAU;
            embeddings::upsert(
                &conn,
                *id,
                crate::embeddings::MODEL,
                &[angle.cos(), angle.sin()],
            )
            .unwrap();
        }
        let count = 2;
        let batch = picked(Radio.next(&conn, &seed(Scope::Library, vec![all[0]], count)));
        assert_eq!(batch.len(), count);
        assert!(
            !batch.contains(&all[0]),
            "the seed came back as its own neighbour"
        );
        // The band is BAND times the batch, so a pick has to be one of that
        // many nearest: the four either side of the seed.
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

    /// Every mode round-trips through the settings file, and anything the
    /// file holds that this build doesn't know reads as the default instead
    /// of taking the whole session shard down with it.
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

    /// Radio isn't a strategy the listener picks any more: it's what the
    /// draw becomes when the queue is ordered by sound. So an old settings
    /// file that names it degrades to the default, the Similar order takes
    /// the draw off whatever mode is set, and Off still means off, since a
    /// queue that was told to end must not start growing because shuffle
    /// happens to be on.
    #[test]
    fn similar_order_takes_the_draw_rather_than_a_mode_of_its_own() {
        assert_eq!(
            serde_json::from_str::<Mode>(r#""radio""#).unwrap(),
            Mode::default()
        );
        assert!(Mode::Off.provider(true).is_none());

        // The same mode, the same seed, the two orders: browse carries on
        // down the view, radio leaves it. An unanalyzed library gives radio
        // nothing to rank by and it falls through to the weighted draw, which
        // is still not the view's next three.
        let conn = library(10);
        let all = ids(&conn);
        let view = Arc::new(all.clone());
        let played = all[2..6].to_vec();
        let ordered = Mode::Continue
            .provider(false)
            .expect("continuation is on")
            .next(&conn, &seed(Scope::View(view.clone()), played.clone(), 3));
        assert_eq!(picked(ordered), all[6..9].to_vec(), "the browse resume");
        let by_sound = Mode::Continue
            .provider(true)
            .expect("continuation is on")
            .next(&conn, &seed(Scope::View(view), played.clone(), 3));
        for id in picked(by_sound) {
            assert!(!played.contains(&id), "radio replayed the session");
        }
    }

    /// The resume walk is the whole of the browse provider's cleverness, so
    /// it gets its own pass over the edges: nothing seen, everything seen,
    /// and a seen track sitting at the very end.
    #[test]
    fn resume_walks_from_the_last_seen_position() {
        let order = vec![1, 2, 3, 4, 5];
        let none = HashSet::new();
        assert_eq!(resume(&order, &none, 2), vec![1, 2]);
        let mid: HashSet<i64> = [2, 3].into_iter().collect();
        assert_eq!(resume(&order, &mid, 9), vec![4, 5]);
        let last: HashSet<i64> = [5].into_iter().collect();
        assert!(resume(&order, &last, 3).is_empty());
        // A gap in the middle is skipped rather than replayed: the resume
        // point is the last seen, and the filter catches the rest.
        let gappy: HashSet<i64> = [1, 4].into_iter().collect();
        assert_eq!(resume(&order, &gappy, 9), vec![5]);
    }
}
