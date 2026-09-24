//! The shared payload for dragging tracks, so library rows, other panels,
//! and external file drops all go through the same enqueue path.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{SharedString, div};

use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;

/// `keys` is what a drop enqueues: keys, not paths, so two tracks of one cue
/// rip queue as two tracks. `ids` is the same drag as catalog rows and isn't
/// index-aligned with `keys`. Both sit behind an Arc because every visible
/// selected row attaches the payload each frame.
#[derive(Clone)]
pub struct PlayDrag {
    pub keys: Arc<[TrackKey]>,
    /// Empty when the source had no catalog rows.
    pub ids: Arc<[i64]>,
    pub title: SharedString,
}

impl PlayDrag {
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

pub struct PlayDragPreview {
    pub title: SharedString,
    pub extra: usize,
}

impl Render for PlayDragPreview {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.extra > 0 {
            SharedString::from(format!("{} +{}", self.title, self.extra))
        } else {
            self.title.clone()
        };
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .text_color(palette::text())
            .child(label)
    }
}
