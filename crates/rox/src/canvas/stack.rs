//! A stacking group's children on a live canvas: how much room each takes
//! this frame. The layout itself is rox-canvas's (`rox_canvas::stack`).
//!
//! Off design mode a child whose case hides it gives its room up over the
//! same ease its fade takes, so its neighbours glide closed as it fades and
//! open again as it comes back. In design mode every child that's switched
//! on keeps its room, hidden case or not, which is the layout the editor's
//! arithmetic measures against.

use std::time::Instant;

use rox_canvas::{CanvasItem, Frame, child_frames};

use crate::canvas::fades::{self, Fades};
use crate::canvas::states::{self, Applied, Cases};

/// Where this frame lays out the children of `group`, one frame each.
#[allow(clippy::too_many_arguments)]
pub(crate) fn frames(
    group: &CanvasItem,
    frame: &Frame,
    children: &[CanvasItem],
    cases: &Cases,
    design: bool,
    still: bool,
    rooms: &mut Fades,
    now: Instant,
) -> Vec<Frame> {
    if group.stack.is_none() {
        return vec![*frame; children.len()];
    }

    child_frames(group, frame, children, |child| {
        if !child.visible {
            return 0.;
        }
        if design {
            return 1.;
        }

        // A fade of its own, with nothing but showing or not in it, so its
        // weight is the share of room it has.
        let room = Applied {
            visible: states::resolve(child, cases, None).visible,
            ..Applied::default()
        };
        rooms
            .step(&child.id, &room, fades::length(child, still), now)
            .opacity
    })
}
