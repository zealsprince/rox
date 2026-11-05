//! Where a workspace window sits and whether it stays above other windows:
//! read when the mini toggle leaves a layout and at persist, written back
//! when a layout returns and at launch. gpui does the work on X11, Windows
//! and macOS. Wayland gives a client neither, so there it goes through
//! KWin's scripting (`kwin.rs`), and other Wayland compositors go without.
//! So does the Flatpak on Wayland: see the trust note in `kwin.rs`.

// Some parameters only reach the KWin arms, which only Linux builds have.
#![cfg_attr(not(target_os = "linux"), allow(unused_variables))]

use gpui::{App, Bounds, Pixels, Point, Size, Window};

#[cfg(target_os = "linux")]
use super::kwin;

#[derive(Clone, Copy, PartialEq)]
enum Backend {
    Native,
    #[cfg(target_os = "linux")]
    Kwin,
    /// Wayland without a way in: windows open wherever the compositor puts
    /// them and can't be pinned.
    #[cfg(target_os = "linux")]
    None,
}

fn backend() -> Backend {
    #[cfg(target_os = "linux")]
    if gpui::guess_compositor() == "Wayland" {
        return match rox_core::install::kind() {
            rox_core::install::Kind::Flatpak => Backend::None,
            _ => Backend::Kwin,
        };
    }
    Backend::Native
}

/// Before the first window opens, so the script is loading while it maps.
pub(crate) fn start(cx: &mut App) {
    #[cfg(target_os = "linux")]
    if backend() == Backend::Kwin {
        kwin::start(cx);
    }
}

/// Whether windows can be moved and pinned here. On Wayland that waits on
/// KWin running the script, and never comes on any other compositor.
pub(crate) fn available(cx: &App) -> bool {
    match backend() {
        Backend::Native => true,
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::live(cx),
        #[cfg(target_os = "linux")]
        Backend::None => false,
    }
}

/// Whether gpui's own bounds carry a real position. Wayland's origin is
/// whatever the window was created with, so it's never worth saving.
pub(crate) fn bounds_have_origin() -> bool {
    backend() == Backend::Native
}

/// Windows parks a minimized window at -32000, and that's what its origin
/// reads until it's restored. Saving it opens every later launch off-screen,
/// so an origin on no display isn't one.
pub(crate) fn origin(window: &Window, cx: &App) -> Option<Point<Pixels>> {
    match backend() {
        Backend::Native => window
            .screen_origin()
            .filter(|&origin| on_a_display(origin, window.bounds().size, cx)),
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::origin(window, cx),
        #[cfg(target_os = "linux")]
        Backend::None => None,
    }
}

/// The window manager's word on the pin, where it gives one, so a pin
/// changed from its own menu shows. None leaves rox's record as the truth.
pub(crate) fn pinned(window: &Window, cx: &App) -> Option<bool> {
    match backend() {
        Backend::Native => None,
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::above(window, cx),
        #[cfg(target_os = "linux")]
        Backend::None => None,
    }
}

pub(crate) fn set_pinned(window: &mut Window, pinned: bool, cx: &App) {
    match backend() {
        Backend::Native => window.set_keep_above(pinned),
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::place(window, None, None, Some(pinned), cx),
        #[cfg(target_os = "linux")]
        Backend::None => {}
    }
}

/// Move to `origin`, a point [`origin`] read. `size` is the content size a
/// resize just asked for, which the window may not have reached yet.
pub(crate) fn move_to(
    window: &mut Window,
    origin: Point<Pixels>,
    size: Option<Size<Pixels>>,
    cx: &App,
) {
    if window.is_maximized() || window.is_fullscreen() {
        return;
    }

    match backend() {
        Backend::Native => {
            if on_a_display(origin, window.bounds().size, cx) {
                window.move_to(origin);
            }
        }
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::place(window, Some(origin), size, None, cx),
        #[cfg(target_os = "linux")]
        Backend::None => {}
    }
}

/// Put a just-opened workspace window back where the last one sat, pinned
/// if it was. On KWin this also ties the window to its compositor side,
/// so it runs for every workspace window, with or without a position.
pub(crate) fn restore(
    window: &mut Window,
    origin: Option<Point<Pixels>>,
    pinned: bool,
    cx: &mut App,
) {
    let origin = origin.filter(|_| !window.is_maximized() && !window.is_fullscreen());
    match backend() {
        // An X11 window manager can ignore a move for a window it hasn't
        // taken on yet, so this waits for the first frame. The saved origin
        // can be off every display, from a monitor unplugged since or a
        // minimized window's parking spot saved by an older build.
        Backend::Native => {
            window.on_next_frame(move |window, cx| {
                if let Some(origin) = origin
                    && on_a_display(origin, window.bounds().size, cx)
                {
                    window.move_to(origin);
                }
                if pinned {
                    window.set_keep_above(true);
                }
            });
        }
        #[cfg(target_os = "linux")]
        Backend::Kwin => kwin::bind(window, origin, pinned.then_some(true), cx),
        #[cfg(target_os = "linux")]
        Backend::None => {}
    }
}

pub(crate) fn forget(window: &Window, cx: &mut App) {
    #[cfg(target_os = "linux")]
    if backend() == Backend::Kwin {
        kwin::forget(window, cx);
    }
}

fn on_a_display(origin: Point<Pixels>, size: Size<Pixels>, cx: &App) -> bool {
    let displays: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
    centered_on_any(origin, size, &displays)
}

/// By the window's center, the same test gpui uses when it opens one.
fn centered_on_any(origin: Point<Pixels>, size: Size<Pixels>, displays: &[Bounds<Pixels>]) -> bool {
    let center = Bounds { origin, size }.center();
    displays.iter().any(|display| display.contains(&center))
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    fn screen() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1920.), px(1080.)),
        }
    }

    #[test]
    fn a_minimized_windows_parking_spot_is_on_no_display() {
        let parked = point(px(-21326.), px(-21345.334));

        assert!(!centered_on_any(
            parked,
            size(px(1536.), px(864.)),
            &[screen()]
        ));
    }

    #[test]
    fn a_window_hanging_off_an_edge_still_counts() {
        let origin = point(px(-400.), px(100.));

        assert!(centered_on_any(
            origin,
            size(px(1280.), px(720.)),
            &[screen()]
        ));
    }

    #[test]
    fn a_monitor_left_of_the_primary_counts() {
        let left = Bounds {
            origin: point(px(-2560.), px(0.)),
            size: size(px(2560.), px(1440.)),
        };

        assert!(centered_on_any(
            point(px(-2000.), px(200.)),
            size(px(1280.), px(720.)),
            &[screen(), left]
        ));
    }
}
