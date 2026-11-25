//! The undo stack the queue and the playlists keep, on the canvas's model
//! (crates/rox/src/canvas/edit.rs): bounded, session-only, and keyed edits
//! close together merge into one step.
//!
//! A step here is the edit that takes the last one back, worked out when it
//! is applied, so applying a step hands back the step that redoes it. Steps
//! never hold a whole copy of what they edit.

use std::time::{Duration, Instant};

/// Steps kept for undo, the canvas's bound.
pub const MAX_STEPS: usize = 200;

/// Edits to one thing this close together are one step, the canvas's window.
pub const MERGE_WINDOW: Duration = Duration::from_millis(800);

/// A step that can fold an older one into itself.
pub trait Step: Sized {
    /// One step that undoes `self` and then `older`.
    fn merge(self, older: Self) -> Self;
}

struct Entry<S> {
    step: S,
    /// Set for edits that merge with the next one to the same thing.
    key: Option<String>,
    at: Instant,
}

pub struct History<S> {
    undos: Vec<Entry<S>>,
    redos: Vec<Entry<S>>,
}

impl<S> Default for History<S> {
    fn default() -> Self {
        Self {
            undos: Vec::new(),
            redos: Vec::new(),
        }
    }
}

impl<S: Step> History<S> {
    /// A new edit drops whatever was undone, the way every editor does.
    pub fn record(&mut self, step: S, key: Option<&str>, now: Instant) {
        self.redos.clear();

        let merges = match (key, self.undos.last()) {
            (Some(key), Some(last)) => {
                last.key.as_deref() == Some(key)
                    && now.saturating_duration_since(last.at) < MERGE_WINDOW
            }
            _ => false,
        };
        if merges && let Some(last) = self.undos.pop() {
            self.undos.push(Entry {
                step: step.merge(last.step),
                key: last.key,
                at: now,
            });
            return;
        }

        if self.undos.len() >= MAX_STEPS {
            self.undos.remove(0);
        }

        self.undos.push(Entry {
            step,
            key: key.map(str::to_owned),
            at: now,
        });
    }

    /// The step that takes the last edit back. Hand what applying it gave
    /// back to [`undone`](Self::undone).
    pub fn take_undo(&mut self) -> Option<S> {
        self.undos.pop().map(|entry| entry.step)
    }

    pub fn undone(&mut self, redo: S) {
        self.redos.push(Entry {
            step: redo,
            key: None,
            at: Instant::now(),
        });
    }

    pub fn take_redo(&mut self) -> Option<S> {
        self.redos.pop().map(|entry| entry.step)
    }

    /// Unlike [`record`](Self::record), keeps the rest of the redos.
    pub fn redone(&mut self, undo: S) {
        if self.undos.len() >= MAX_STEPS {
            self.undos.remove(0);
        }

        self.undos.push(Entry {
            step: undo,
            key: None,
            at: Instant::now(),
        });
    }

    pub fn can_undo(&self) -> bool {
        !self.undos.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redos.is_empty()
    }

    pub fn clear(&mut self) {
        self.undos.clear();
        self.redos.clear();
    }

    /// Every held step, undos and redos, for a rewrite in place.
    pub fn steps_mut(&mut self) -> impl Iterator<Item = &mut S> {
        self.undos
            .iter_mut()
            .chain(self.redos.iter_mut())
            .map(|entry| &mut entry.step)
    }

    /// Drops every step `keep` says no to.
    pub fn retain(&mut self, mut keep: impl FnMut(&S) -> bool) {
        self.undos.retain(|entry| keep(&entry.step));
        self.redos.retain(|entry| keep(&entry.step));
    }

    #[cfg(test)]
    fn depth(&self) -> (usize, usize) {
        (self.undos.len(), self.redos.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A step that lists the edits it stands for, newest first.
    #[derive(Debug, PartialEq)]
    struct Edits(Vec<u32>);

    impl Step for Edits {
        fn merge(mut self, older: Self) -> Self {
            self.0.extend(older.0);
            self
        }
    }

    #[test]
    fn the_stack_is_bounded_and_drops_the_oldest() {
        let mut history = History::default();
        let now = Instant::now();

        for n in 0..(MAX_STEPS as u32 + 5) {
            history.record(Edits(vec![n]), None, now);
        }

        assert_eq!(history.depth(), (MAX_STEPS, 0));

        let mut last = None;
        while let Some(step) = history.take_undo() {
            last = Some(step);
        }
        assert_eq!(last, Some(Edits(vec![5])));
    }

    #[test]
    fn keyed_edits_merge_inside_the_window_and_unkeyed_never_do() {
        let mut history = History::default();
        let now = Instant::now();

        history.record(Edits(vec![1]), Some("move:4"), now);
        history.record(
            Edits(vec![2]),
            Some("move:4"),
            now + Duration::from_millis(300),
        );
        // Another key, so its own step.
        history.record(
            Edits(vec![3]),
            Some("move:5"),
            now + Duration::from_millis(400),
        );
        // Past the window after the last merge.
        history.record(
            Edits(vec![4]),
            Some("move:5"),
            now + Duration::from_millis(400) + MERGE_WINDOW,
        );
        history.record(Edits(vec![5]), None, now);
        history.record(Edits(vec![6]), None, now);

        assert_eq!(history.depth(), (5, 0));
        assert_eq!(history.take_undo(), Some(Edits(vec![6])));
        assert_eq!(history.take_undo(), Some(Edits(vec![5])));
        assert_eq!(history.take_undo(), Some(Edits(vec![4])));
        assert_eq!(history.take_undo(), Some(Edits(vec![3])));
        assert_eq!(history.take_undo(), Some(Edits(vec![2, 1])));
    }

    #[test]
    fn a_new_edit_drops_the_redos_and_a_redo_keeps_them() {
        let mut history = History::default();
        let now = Instant::now();

        history.record(Edits(vec![1]), None, now);
        history.record(Edits(vec![2]), None, now);

        let undo = history.take_undo().expect("two steps");
        history.undone(undo);
        let undo = history.take_undo().expect("one step");
        history.undone(undo);
        assert_eq!(history.depth(), (0, 2));

        let redo = history.take_redo().expect("two redos");
        history.redone(redo);
        assert_eq!(history.depth(), (1, 1));

        history.record(Edits(vec![3]), None, now);
        assert!(!history.can_redo());
    }
}
