//! Session cue points: throwaway marks dropped on a strip, gone when rox
//! closes. Never write them to SQLite: persisted cues would just be
//! bookmarks with a worse editor. Keyed by [`TrackKey`], so two cue subsongs
//! of one image keep their own sets.

use std::collections::HashMap;

use gpui::{Context, EventEmitter};
use rox_library::cue::TrackKey;

/// Carries the track so a strip drawing one song can ignore the rest.
pub struct CuesChanged {
    pub key: TrackKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// Unique for the run, so a mark isn't named by a position another drop
    /// could match.
    pub id: u64,
    pub position_ms: u32,
}

/// A drop this close to an existing mark is that mark, so a double-click
/// doesn't stack two where only the front one is clickable.
const DUPLICATE_MS: u32 = 250;

/// A step skips marks this close to the playhead in either direction. A seek
/// onto a mark lands a few ms short, and without the grace Next would find
/// the same mark again on every press.
const STEP_GRACE_MS: u32 = 1_000;

/// Sets stay sorted by position.
#[derive(Default)]
pub struct Cues {
    marks: HashMap<TrackKey, Vec<Cue>>,
    next_id: u64,
}

impl EventEmitter<CuesChanged> for Cues {}

impl Cues {
    /// Returns the id of the new mark, or of the existing one a near miss hit.
    pub fn add(&mut self, key: &TrackKey, position_ms: u32, cx: &mut Context<Self>) -> u64 {
        let set = self.marks.entry(key.clone()).or_default();

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

    /// An id that isn't there is a no-op: a menu can outlive its mark.
    pub fn remove(&mut self, key: &TrackKey, id: u64, cx: &mut Context<Self>) {
        let Some(set) = self.marks.get_mut(key) else {
            return;
        };
        let before = set.len();
        set.retain(|cue| cue.id != id);
        if set.len() == before {
            return;
        }

        // Drop empty sets, or the map grows with every track ever marked.
        if set.is_empty() {
            self.marks.remove(key);
        }

        cx.emit(CuesChanged { key: key.clone() });
        cx.notify();
    }

    pub fn clear(&mut self, key: &TrackKey, cx: &mut Context<Self>) {
        if self.marks.remove(key).is_none() {
            return;
        }

        cx.emit(CuesChanged { key: key.clone() });
        cx.notify();
    }

    pub fn for_key(&self, key: &TrackKey) -> Vec<Cue> {
        self.marks.get(key).cloned().unwrap_or_default()
    }

    pub fn next_after(&self, key: &TrackKey, position_ms: u32) -> Option<Cue> {
        let cutoff = position_ms.saturating_add(STEP_GRACE_MS);
        self.marks
            .get(key)?
            .iter()
            .find(|cue| cue.position_ms > cutoff)
            .copied()
    }

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

            assert_eq!(cues.add(&track, 10_200, cx), early);
            assert_eq!(cues.for_key(&track).len(), 2);

            assert_ne!(cues.add(&track, 10_400, cx), early);
            assert_eq!(cues.for_key(&track).len(), 3);
        });
    }

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
            // A seek onto the 40s mark lands a hair short of it.
            assert_eq!(next(39_917), Some(70_000));
            assert_eq!(next(70_000), None);

            let prev = |at| cues.prev_before(&track, at).map(|c| c.position_ms);
            assert_eq!(prev(45_000), Some(40_000));
            assert_eq!(prev(40_500), Some(10_000));
            assert_eq!(prev(5_000), None);
        });
    }

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
            cues.add(&track, 1_100, cx);
            cues.remove(&track, id, cx);
        });
        cx.run_until_parked();

        assert_eq!(seen.lock().unwrap().as_slice(), &[track.clone(), track]);
    }
}
