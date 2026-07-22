//! The shared payload for dragging tracks onto a drop target that plays them.
//! Carries the files in drag order so a drop queues them straight through the
//! path-based engine, plus the library id per path when the source had one
//! (None for an out-of-library file). One type so library rows, other panels,
//! and external file drops all land through the same enqueue path.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{div, SharedString};

use crate::design::{palette, tokens};

/// The value carried through a track drag. `paths` is the drag order a drop
/// enqueues, straight through the path-based engine so out-of-library files
/// ride along too. `title` labels the floating preview. The paths ride behind
/// an Arc so a row attaches the payload with a refcount bump: a grab inside a
/// big multi-selection would otherwise clone the whole path set into every
/// visible selected row on every frame.
#[derive(Clone)]
pub struct PlayDrag {
    pub paths: Arc<[PathBuf]>,
    pub title: SharedString,
}

impl PlayDrag {
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// The label that floats under the pointer while tracks are dragged. A multi
/// drag shows the grabbed title with a count of the rest.
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
