//! What a window draws for itself when the compositor leaves it bare. Wayland
//! compositors without `zxdg_decoration_manager_v1` (GNOME's mutter) hand back
//! an undecorated surface whatever the window asked for.

use gpui::{
    Decorations, Div, MouseButton, ResizeEdge, Tiling, Window, WindowDecorations, div, prelude::*,
    px,
};

/// Wider than an OS frame's 1px: there's no visible border to aim at.
const GRIP: gpui::Pixels = px(6.);

const CORNER: gpui::Pixels = px(14.);

/// True only where the window asked for the OS frame and the compositor
/// refused. A window that asked to go bare keeps its own arrangement.
pub fn chrome_missing(asked: WindowDecorations, window: &Window) -> bool {
    refused(asked, window.window_decorations())
}

fn refused(asked: WindowDecorations, got: Decorations) -> bool {
    matches!(asked, WindowDecorations::Server) && matches!(got, Decorations::Client { .. })
}

/// Paint as the window root's last child. Tiled edges are skipped since they
/// won't resize.
///
/// Linux only: `start_window_resize` exists on X11 and Wayland and nowhere
/// else, so elsewhere the grips would be dead zones eating edge clicks.
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
            // Only the children take hits, so the content stays clickable.
            .children(edges(tiling))
            .children(corners(tiling)),
    )
}

/// Each inset by a corner at both ends so the corners win the overlap.
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

/// A corner needs both its edges free, or the diagonal drag goes nowhere.
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

    /// Getting this backwards strands a GNOME window with no close button or
    /// stacks a second titlebar on a layout that went bare on purpose.
    #[test]
    fn only_a_refused_ask_counts_as_missing() {
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };

        assert!(refused(WindowDecorations::Server, client));

        assert!(!refused(WindowDecorations::Server, Decorations::Server));

        assert!(!refused(WindowDecorations::Client, client));

        // xdg-decoration allows decorating a window that asked to go bare.
        assert!(!refused(WindowDecorations::Client, Decorations::Server));
    }
}
