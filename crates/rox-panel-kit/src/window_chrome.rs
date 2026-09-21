//! What a window has to draw for itself when the compositor leaves it
//! bare: the resize grips along its edges, and the test for whether it's
//! in that spot at all.
//!
//! Wayland is the case that matters. A compositor advertises
//! `zxdg_decoration_manager_v1` or it doesn't, and the ones that don't
//! (GNOME's mutter) hand back an undecorated surface no matter what the
//! window asked for. Nothing else supplies a close button or an edge to
//! drag, so the window supplies both.

use gpui::{
    Decorations, Div, MouseButton, ResizeEdge, Tiling, Window, WindowDecorations, div, prelude::*,
    px,
};

/// How far in from an edge a press still counts as a resize. Wider than
/// the 1px an OS frame gets away with: there's no visible border here to
/// aim at, so the zone has to be findable by feel.
const GRIP: gpui::Pixels = px(6.);

/// The corner zones, where a press resizes both axes at once. Square, so
/// a corner reads as a corner rather than two edges meeting.
const CORNER: gpui::Pixels = px(14.);

/// Whether `window` is missing the chrome it asked for: it wanted the OS
/// frame and came up client-decorated anyway. True only where the
/// compositor refused, so a window that deliberately asked to go bare
/// (the OS Decorations toggle, off) answers false and keeps its own
/// arrangement.
pub fn chrome_missing(asked: WindowDecorations, window: &Window) -> bool {
    refused(asked, window.window_decorations())
}

/// [`chrome_missing`] without the window, so the rule itself is testable.
fn refused(asked: WindowDecorations, got: Decorations) -> bool {
    matches!(asked, WindowDecorations::Server) && matches!(got, Decorations::Client { .. })
}

/// The resize grips for a window drawing its own chrome: four edges and
/// four corners, absolutely placed, meant as the last child of the window
/// root so they paint over whatever content reaches the edge. None when
/// the OS owns the frame and there's nothing to stand in for.
///
/// Drawn whichever way the window ended up undecorated: the compositor
/// refused, or the OS Decorations toggle asked it to. Both leave the same
/// hole, since a bare Wayland surface has no edge of its own to drag.
///
/// An edge the compositor reports as tiled is skipped: it's flush against
/// a screen edge or a neighbour and won't resize, so a grip there would
/// be a cursor change that does nothing.
///
/// Linux only. `start_window_resize` is implemented on X11 and Wayland
/// and nowhere else, so the grips would be dead zones eating edge clicks
/// on Windows (which has its own resize border, see the Resize Border
/// setting) and macOS (which never goes client-decorated).
pub fn resize_grips(window: &Window) -> Option<Div> {
    if !cfg!(target_os = "linux") {
        return None;
    }

    let Decorations::Client { tiling } = window.window_decorations() else {
        return None;
    };

    Some(
        div()
            .absolute()
            .inset_0()
            // The container itself takes no hits, only the eight children
            // below do, so the content underneath stays clickable.
            .children(edges(tiling))
            .children(corners(tiling)),
    )
}

/// The four straight edges, each inset by a corner at both ends so the
/// corner zones win the overlap.
fn edges(tiling: Tiling) -> Vec<Div> {
    let mut out = Vec::with_capacity(4);

    if !tiling.top {
        out.push(
            grip(ResizeEdge::Top)
                .top_0()
                .left(CORNER)
                .right(CORNER)
                .h(GRIP)
                .cursor_ns_resize(),
        );
    }

    if !tiling.bottom {
        out.push(
            grip(ResizeEdge::Bottom)
                .bottom_0()
                .left(CORNER)
                .right(CORNER)
                .h(GRIP)
                .cursor_ns_resize(),
        );
    }

    if !tiling.left {
        out.push(
            grip(ResizeEdge::Left)
                .left_0()
                .top(CORNER)
                .bottom(CORNER)
                .w(GRIP)
                .cursor_ew_resize(),
        );
    }

    if !tiling.right {
        out.push(
            grip(ResizeEdge::Right)
                .right_0()
                .top(CORNER)
                .bottom(CORNER)
                .w(GRIP)
                .cursor_ew_resize(),
        );
    }

    out
}

/// The four corners. A corner needs both of its edges free to be worth
/// drawing: pinned on either axis, the diagonal drag can't go anywhere.
fn corners(tiling: Tiling) -> Vec<Div> {
    let mut out = Vec::with_capacity(4);

    if !tiling.top && !tiling.left {
        out.push(
            grip(ResizeEdge::TopLeft)
                .top_0()
                .left_0()
                .size(CORNER)
                .cursor_nwse_resize(),
        );
    }

    if !tiling.top && !tiling.right {
        out.push(
            grip(ResizeEdge::TopRight)
                .top_0()
                .right_0()
                .size(CORNER)
                .cursor_nesw_resize(),
        );
    }

    if !tiling.bottom && !tiling.left {
        out.push(
            grip(ResizeEdge::BottomLeft)
                .bottom_0()
                .left_0()
                .size(CORNER)
                .cursor_nesw_resize(),
        );
    }

    if !tiling.bottom && !tiling.right {
        out.push(
            grip(ResizeEdge::BottomRight)
                .bottom_0()
                .right_0()
                .size(CORNER)
                .cursor_nwse_resize(),
        );
    }

    out
}

/// One grip: an invisible absolutely-placed zone that hands the press to
/// the compositor's own resize loop. The caller sizes and places it and
/// picks the cursor; everything from the press on belongs to the
/// compositor, so there's no drag to track on this side.
fn grip(edge: ResizeEdge) -> Div {
    div()
        .absolute()
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            window.start_window_resize(edge);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule the fallback titlebar hangs off. Getting the ask backwards
    /// either strands a GNOME window with no close button (the bug this
    /// exists for) or stacks a second titlebar on a layout that turned the
    /// OS chrome off on purpose.
    #[test]
    fn only_a_refused_ask_counts_as_missing() {
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };

        // Wanted the OS frame, came up bare: the compositor refused.
        assert!(refused(WindowDecorations::Server, client));

        // Wanted the OS frame and got it.
        assert!(!refused(WindowDecorations::Server, Decorations::Server));

        // Asked to go bare, went bare. The layout owns its own chrome.
        assert!(!refused(WindowDecorations::Client, client));

        // Asked to go bare and the compositor decorated it anyway, which
        // xdg-decoration allows. Nothing is missing, so nothing is drawn.
        assert!(!refused(WindowDecorations::Client, Decorations::Server));
    }
}
