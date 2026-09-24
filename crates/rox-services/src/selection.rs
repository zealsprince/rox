//! The app-wide selection: the tracks last explicitly picked in any panel,
//! as library ids so they survive projection reloads. Selection mechanics
//! stay per panel; only the resolved result is published here.
//!
//! A pick carries the publishing panel's id, so a scoped drawer can ignore
//! picks from elsewhere and a selection-following view doesn't narrow onto
//! its own clicks.

use gpui::{Context, EntityId, EventEmitter};

pub struct SelectionEvent {
    pub source: EntityId,
}

pub struct Selection {
    tracks: Vec<i64>,
    source: EntityId,
}

impl EventEmitter<SelectionEvent> for Selection {}

impl Selection {
    /// The entity's own id stands in as the empty source, which no panel has.
    pub fn new(cx: &Context<Self>) -> Self {
        Selection {
            tracks: Vec::new(),
            source: cx.entity_id(),
        }
    }

    pub fn tracks(&self) -> &[i64] {
        &self.tracks
    }

    pub fn source(&self) -> EntityId {
        self.source
    }

    /// Every call fires, an unchanged set included: a second click on the same
    /// album has to reopen a drawer the way the first did.
    pub fn set(&mut self, tracks: Vec<i64>, source: EntityId, cx: &mut Context<Self>) {
        self.tracks = tracks;
        self.source = source;
        cx.emit(SelectionEvent { source });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use gpui::{AppContext as _, TestAppContext};

    struct Panel;

    /// A value-style dedupe here would swallow the second click and leave a
    /// selection drawer shut.
    #[gpui::test]
    fn repeating_a_pick_still_fires(cx: &mut TestAppContext) {
        let selection = cx.new(|cx| Selection::new(cx));
        let panel = cx.new(|_| Panel);
        let source = panel.entity_id();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&selection, move |_, event: &SelectionEvent, _| {
                sink.lock().unwrap().push(event.source);
            })
        });

        selection.update(cx, |s, cx| s.set(vec![1, 2, 3], source, cx));
        selection.update(cx, |s, cx| s.set(vec![1, 2, 3], source, cx));

        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(
            selection.read_with(cx, |s, _| s.tracks().to_vec()),
            [1, 2, 3]
        );
    }

    #[gpui::test]
    fn a_pick_names_its_publisher(cx: &mut TestAppContext) {
        let selection = cx.new(|cx| Selection::new(cx));
        let (wall, list) = (cx.new(|_| Panel), cx.new(|_| Panel));

        selection.update(cx, |s, cx| s.set(vec![7], wall.entity_id(), cx));
        assert_eq!(selection.read_with(cx, |s, _| s.source()), wall.entity_id());

        selection.update(cx, |s, cx| s.set(vec![7], list.entity_id(), cx));
        assert_eq!(selection.read_with(cx, |s, _| s.source()), list.entity_id());

        let fresh = cx.new(|cx| Selection::new(cx));
        assert_ne!(fresh.read_with(cx, |s, _| s.source()), wall.entity_id());
    }
}
