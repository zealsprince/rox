//! The app-wide selection: the tracks the user last explicitly selected in
//! any panel, as library db ids so they survive projection reloads. Panels
//! that select (the library) publish here on click; panels that display the
//! selection (cover art, track info) subscribe and read. The mechanics stay
//! per-panel: duplicated panels with their own queries keep their own shift
//! anchors and highlights, only the resolved result bubbles up.
//!
//! A pick carries the id of the panel that made it. Two things need to know
//! where a selection came from: a drawer scoped to its own main panel, which
//! ignores picks made elsewhere in the layout, and a selection-following
//! view, which would otherwise narrow onto its own clicks until one row is
//! left.

use gpui::{Context, EntityId, EventEmitter};

/// The selection changed; subscribed panels re-read `tracks`. Carries the
/// panel that published it, so a subscriber can tell its own picks from
/// everyone else's.
pub struct SelectionEvent {
    pub source: EntityId,
}

/// The selected tracks, in the order the selecting view displayed them.
pub struct Selection {
    tracks: Vec<i64>,
    /// The panel that published the current pick.
    source: EntityId,
}

impl EventEmitter<SelectionEvent> for Selection {}

impl Selection {
    /// A fresh selection, owned by nothing. The entity's own id stands in as
    /// the empty source: no panel carries it, so no panel mistakes the
    /// initial state for its own pick.
    pub fn new(cx: &Context<Self>) -> Self {
        Selection {
            tracks: Vec::new(),
            source: cx.entity_id(),
        }
    }

    pub fn tracks(&self) -> &[i64] {
        &self.tracks
    }

    /// The panel that published the current pick.
    pub fn source(&self) -> EntityId {
        self.source
    }

    /// Publish a pick from `source`. Every call fires, an unchanged set
    /// included: a pick is a gesture rather than a value, and the surfaces
    /// that open on one - a selection drawer - have to answer a second click
    /// on the same album the way they answered the first.
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

    /// Stands in for a publishing panel: something with an entity id of its
    /// own to stamp picks with.
    struct Panel;

    /// Every pick fires, an unchanged track set included. This is what lets a
    /// selection drawer answer a second click on the same album: the ids come
    /// back identical, and a value-style dedupe would swallow the gesture and
    /// leave the drawer shut.
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

    /// The pick carries who made it, which is what a scoped drawer matches
    /// against and what stops a selection-following view from narrowing onto
    /// its own clicks.
    #[gpui::test]
    fn a_pick_names_its_publisher(cx: &mut TestAppContext) {
        let selection = cx.new(|cx| Selection::new(cx));
        let (wall, list) = (cx.new(|_| Panel), cx.new(|_| Panel));

        selection.update(cx, |s, cx| s.set(vec![7], wall.entity_id(), cx));
        assert_eq!(selection.read_with(cx, |s, _| s.source()), wall.entity_id());

        selection.update(cx, |s, cx| s.set(vec![7], list.entity_id(), cx));
        assert_eq!(selection.read_with(cx, |s, _| s.source()), list.entity_id());

        // A fresh selection belongs to no panel, so nothing mistakes the
        // opening state for its own pick.
        let fresh = cx.new(|cx| Selection::new(cx));
        assert_ne!(fresh.read_with(cx, |s, _| s.source()), wall.entity_id());
    }
}
