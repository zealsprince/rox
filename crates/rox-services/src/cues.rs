//! Session cue points: the marks dropped on a strip to find a spot again
//! while listening, held for the length of this run and nothing longer.
//! A bookmark is a considered thing, named and coloured and written to the
//! library; a cue is the opposite of considered. It goes down in one right
//! click because the drop is right there, it gets stepped through a few
//! times, and it's gone when rox closes.
//!
//! Nothing here touches SQLite, and deliberately so. Writing cues would
//! make them bookmarks with a worse editor, and the value of a mark you
//! can drop without thinking is that dropping ten of them costs nothing
//! and leaves nothing behind.
//!
//! Keyed by [`TrackKey`] like everything else that hangs off a track, so
//! two cue subsongs of one image keep their own sets.

use std::collections::HashMap;

use gpui::{Context, EventEmitter};
use rox_library::cue::TrackKey;

/// A track's cue set moved: one dropped, one removed, or the lot cleared.
/// Subscribers re-read [`Cues::for_key`]; the event carries the track so a
/// strip drawing one song can ignore the rest.
pub struct CuesChanged {
    pub key: TrackKey,
}

/// One session mark along a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// Unique for the run, so a strip can name the mark it is drawing
    /// without leaning on a position that another drop could match.
    pub id: u64,
    pub position_ms: u32,
}

/// How close a new cue has to land to an existing one before it's read as
/// the same mark. A quarter second is under what anyone aims for by hand,
/// and it saves a double-click from leaving two marks stacked at one spot
/// where only the front one is clickable.
const DUPLICATE_MS: u32 = 250;

/// How far from the playhead a cue has to be before a step goes to it, in
/// either direction. Sitting on a mark, Previous should reach the one
/// before rather than pin to the one under the head, which is how stepping
/// bookmarks and stepping tracks both already behave. Next needs the same
/// room: a seek onto a mark lands a few milliseconds short of it, and a
/// bare "past the head" compare would find that same mark again and stick
/// there for every press while paused.
const STEP_GRACE_MS: u32 = 1_000;

/// Every track's session marks. The sets stay sorted by position, since
/// every reader wants them in strip order and there are never enough of
/// them for the insert to cost anything.
#[derive(Default)]
pub struct Cues {
    marks: HashMap<TrackKey, Vec<Cue>>,
    next_id: u64,
}

impl EventEmitter<CuesChanged> for Cues {}

impl Cues {
    /// Drop a mark, or find the one already there. Returns the id either
    /// way, so a caller that wants to jump to what it just placed doesn't
    /// have to care which happened.
    pub fn add(&mut self, key: &TrackKey, position_ms: u32, cx: &mut Context<Self>) -> u64 {
        let set = self.marks.entry(key.clone()).or_default();

        // A near miss is the same mark. Nothing moves and nothing emits:
        // the set the subscribers hold is already right.
        if let Some(near) = set
            .iter()
            .find(|cue| cue.position_ms.abs_diff(position_ms) <= DUPLICATE_MS)
        {
            return near.id;
        }

        self.next_id += 1;
        let cue = Cue {
            id: self.next_id,
            position_ms,
        };
        let at = set.partition_point(|other| other.position_ms < position_ms);
        set.insert(at, cue);

        cx.emit(CuesChanged { key: key.clone() });
        cx.notify();
        cue.id
    }

    /// Take one mark off a track. An id that isn't there is a no-op, which
    /// is what a menu left open over a mark that has since gone hands in.
    pub fn remove(&mut self, key: &TrackKey, id: u64, cx: &mut Context<Self>) {
        let Some(set) = self.marks.get_mut(key) else {
            return;
        };
        let before = set.len();
        set.retain(|cue| cue.id != id);
        if set.len() == before {
            return;
        }

        // An empty set is the same as no set, and leaving the key behind
        // would grow the map with every track ever marked and unmarked.
        if set.is_empty() {
            self.marks.remove(key);
        }

        cx.emit(CuesChanged { key: key.clone() });
        cx.notify();
    }

    /// Drop a track's marks in one go.
    pub fn clear(&mut self, key: &TrackKey, cx: &mut Context<Self>) {
        if self.marks.remove(key).is_none() {
            return;
        }

        cx.emit(CuesChanged { key: key.clone() });
        cx.notify();
    }

    /// A track's marks in strip order.
    pub fn for_key(&self, key: &TrackKey) -> Vec<Cue> {
        self.marks.get(key).cloned().unwrap_or_default()
    }

    /// The first mark far enough past `position_ms` to count, for Next.
    /// See [`STEP_GRACE_MS`].
    pub fn next_after(&self, key: &TrackKey, position_ms: u32) -> Option<Cue> {
        let cutoff = position_ms.saturating_add(STEP_GRACE_MS);
        self.marks
            .get(key)?
            .iter()
            .find(|cue| cue.position_ms > cutoff)
            .copied()
    }

    /// The last mark far enough behind `position_ms` to count, for
    /// Previous. See [`STEP_GRACE_MS`].
    pub fn prev_before(&self, key: &TrackKey, position_ms: u32) -> Option<Cue> {
        let cutoff = position_ms.saturating_sub(STEP_GRACE_MS);
        self.marks
            .get(key)?
            .iter()
            .rev()
            .find(|cue| cue.position_ms < cutoff)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use gpui::{AppContext as _, TestAppContext};

    fn key(name: &str) -> TrackKey {
        TrackKey::from(PathBuf::from(format!("/music/{name}.flac")))
    }

    /// Drops land in strip order however they were made, and a second drop
    /// on top of the first answers with the first instead of stacking two
    /// marks at one spot.
    #[gpui::test]
    fn drops_sort_and_a_near_miss_is_the_same_mark(cx: &mut TestAppContext) {
        let cues = cx.new(|_| Cues::default());
        let track = key("a");
        cues.update(cx, |cues, cx| {
            let late = cues.add(&track, 90_000, cx);
            let early = cues.add(&track, 10_000, cx);
            assert_ne!(late, early);

            let positions: Vec<u32> = cues.for_key(&track).iter().map(|c| c.position_ms).collect();
            assert_eq!(positions, vec![10_000, 90_000]);

            // Inside the window: the mark already there answers, and the
            // set doesn't grow.
            assert_eq!(cues.add(&track, 10_200, cx), early);
            assert_eq!(cues.for_key(&track).len(), 2);

            // Outside it: a mark of its own.
            assert_ne!(cues.add(&track, 10_400, cx), early);
            assert_eq!(cues.for_key(&track).len(), 3);
        });
    }

    /// Next takes the mark ahead and Previous the one behind, and both skip
    /// the mark under the head, so a second press walks on rather than
    /// pinning where it landed.
    #[gpui::test]
    fn stepping_reads_ahead_and_back_with_a_grace(cx: &mut TestAppContext) {
        let cues = cx.new(|_| Cues::default());
        let track = key("a");
        cues.update(cx, |cues, cx| {
            for at in [10_000, 40_000, 70_000] {
                cues.add(&track, at, cx);
            }

            let next = |at| cues.next_after(&track, at).map(|c| c.position_ms);
            assert_eq!(next(0), Some(10_000));
            assert_eq!(next(40_000), Some(70_000));
            // A seek onto the 40s mark lands a hair short of it; Next still
            // reaches past it instead of finding it again.
            assert_eq!(next(39_917), Some(70_000));
            assert_eq!(next(70_000), None);

            let prev = |at| cues.prev_before(&track, at).map(|c| c.position_ms);
            assert_eq!(prev(45_000), Some(40_000));
            // Landed on the 40s mark, Previous reaches past it.
            assert_eq!(prev(40_500), Some(10_000));
            assert_eq!(prev(5_000), None);
        });
    }

    /// Marks are per track, and clearing one track leaves the others alone.
    #[gpui::test]
    fn clearing_takes_one_tracks_marks(cx: &mut TestAppContext) {
        let cues = cx.new(|_| Cues::default());
        let (a, b) = (key("a"), key("b"));
        cues.update(cx, |cues, cx| {
            cues.add(&a, 1_000, cx);
            let only_b = cues.add(&b, 2_000, cx);

            cues.clear(&a, cx);
            assert!(cues.for_key(&a).is_empty());
            assert_eq!(cues.for_key(&b).len(), 1);

            cues.remove(&b, only_b, cx);
            assert!(cues.for_key(&b).is_empty());
        });
    }

    /// Every move announces its own track, so a strip drawing one song
    /// isn't woken by a mark dropped on another.
    #[gpui::test]
    fn a_move_names_the_track_it_happened_on(cx: &mut TestAppContext) {
        let cues = cx.new(|_| Cues::default());
        let track = key("a");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&cues, move |_, event: &CuesChanged, _| {
                record.lock().unwrap().push(event.key.clone());
            })
        });

        cues.update(cx, |cues, cx| {
            let id = cues.add(&track, 1_000, cx);
            // The duplicate answers from the set without moving it, so it
            // has nothing to announce.
            cues.add(&track, 1_100, cx);
            cues.remove(&track, id, cx);
        });
        cx.run_until_parked();

        assert_eq!(seen.lock().unwrap().as_slice(), &[track.clone(), track]);
    }
}
