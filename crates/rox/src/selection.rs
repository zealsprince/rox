//! The app-wide selection: the tracks the user last explicitly selected in
//! any panel, as library db ids so they survive projection reloads. Panels
//! that select (the library) publish here on click; panels that display the
//! selection (cover art, track info) subscribe and read. The mechanics stay
//! per-panel: duplicated panels with their own queries keep their own shift
//! anchors and highlights, only the resolved result bubbles up.

use gpui::{Context, EventEmitter};

/// The selection changed; subscribed panels re-read `tracks`.
pub enum SelectionEvent {
    Changed,
}

/// The selected tracks, in the order the selecting view displayed them.
#[derive(Default)]
pub struct Selection {
    tracks: Vec<i64>,
}

impl EventEmitter<SelectionEvent> for Selection {}

impl Selection {
    pub fn tracks(&self) -> &[i64] {
        &self.tracks
    }

    pub fn set(&mut self, tracks: Vec<i64>, cx: &mut Context<Self>) {
        if self.tracks == tracks {
            return;
        }
        self.tracks = tracks;
        cx.emit(SelectionEvent::Changed);
        cx.notify();
    }
}
