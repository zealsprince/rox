//! The titlebar a window grows when the compositor won't give it one.
//!
//! Wayland compositors that don't implement `zxdg_decoration_manager_v1`
//! (GNOME's mutter) hand back a bare surface whatever the window asked
//! for. A workspace window can live with that, since a layout can carry
//! the window controls panel, but the settings window and every editor
//! and dialog came up with no close button and no edge to drag: alt-F4
//! was the only way out (issue #128).
//!
//! So a window that asked for the OS frame and didn't get it draws this
//! instead: one strip with the title and the three buttons. A window that
//! asked to go bare on purpose (the OS Decorations toggle, off) keeps its
//! own arrangement, because a layout that went bare has the window
//! controls panel and a second set of buttons would be in the way.
//!
//! The resize grips underneath are the looser case, and go on any bare
//! window: an undecorated Wayland surface has no edge of its own to drag
//! whichever way it ended up undecorated.

use gpui::{
    AnyElement, AnyView, App, Context, Entity, MouseButton, MouseDownEvent, Render, SharedString,
    Window, WindowDecorations, div, prelude::*, px,
};
use rox_core::settings::{self, ChromeSide, ChromeStyle};
use rox_design::{palette, tokens};
use rox_panel_kit::{chrome_missing, icon_controls, resize_grips, traffic_lights};

/// The strip's height. The window controls panel's buttons are 24px, and
/// this matches it with the same breathing room a menubar row gets.
const BAR_HEIGHT: gpui::Pixels = px(32.);

/// Wrap a window's body in the chrome the compositor didn't supply, or
/// hand the body straight back when it did. `asked` is what the window
/// requested at open: always `Server` for a child window, the live
/// setting for a workspace window.
///
/// The strip's text comes off the window title registry rather than a
/// parameter, so a window that retitles itself after opening (a popped-out
/// panel takes its panel's rename) carries that into the strip without
/// anything plumbed through.
///
/// `close` is the caller's, because closing means different things per
/// window: a dialog just goes, a workspace window runs its teardown
/// first.
pub fn framed(
    asked: WindowDecorations,
    body: AnyElement,
    window: &Window,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    // The grips go on any undecorated window, including one that asked to
    // be bare. A bare Wayland surface has no edge of its own to drag
    // whichever way it got that way, so the layout that turned the OS
    // chrome off needs them as much as the one the compositor refused.
    let grips = resize_grips(window);

    // The strip is the narrower case: only where the window wanted the OS
    // frame and didn't get it. A layout that went bare on purpose has the
    // window controls panel for this, and a second set of buttons over the
    // top of it would be in the way.
    let strip =
        chrome_missing(asked, window).then(|| titlebar(window_title(window), window, close));

    if strip.is_none() && grips.is_none() {
        return body;
    }

    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .children(strip)
        // min_h_0 or the body's own content floors the flex item and the
        // strip gets pushed off the top of a window sized to its minimum.
        .child(div().flex_1().min_h_0().child(body))
        // Last, so the grips paint over whatever content reaches the edge.
        .children(grips)
        .into_any_element()
}

/// The strip itself: the title, the three buttons at the configured end,
/// and the whole thing a move handle.
fn titlebar(
    title: SharedString,
    window: &Window,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let side = settings::chrome_side();

    let buttons = div()
        .flex()
        .items_center()
        .map(|d| match settings::chrome_style() {
            ChromeStyle::Icons => d
                .gap(tokens::SPACE_XS)
                .children(icon_controls(window, close)),
            ChromeStyle::Traffic => d
                .gap(tokens::SPACE_SM)
                .children(traffic_lights(window, close)),
        });

    // The move handle is the label rather than the whole strip. A handler
    // on the strip would fire on a button press too, since gpui hands a
    // mouse down to every hitbox under it and the buttons don't stop it
    // bubbling: a click on close would start a window move on its way out.
    // The label is flex_1, so it already covers everything the buttons
    // don't.
    let label = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .text_sm()
        .text_color(palette::text_muted())
        .truncate()
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            // Double-click is the OS titlebar's maximize toggle, and a
            // move grab started on the first press would swallow it.
            if event.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .child(title);

    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .h(BAR_HEIGHT)
        .px(tokens::SPACE_SM)
        .gap(tokens::SPACE_SM)
        .bg(palette::bg_menubar())
        .map(|d| match side {
            ChromeSide::Left => d.child(buttons).child(label),
            ChromeSide::Right => d.child(label).child(buttons),
        })
}

/// The view every child window is wrapped in on its way into a Root: the
/// body it was built with, under the fallback titlebar when there's one to
/// draw. Type-erased because `open_window` is generic over the body and
/// the wrapper has nothing to say about it.
///
/// Always in place, even where the compositor decorates properly, so
/// there's one window tree rather than two. [`framed`] hands the body
/// straight back when the OS frame is there, which costs a render call
/// that returns its child.
pub struct Framed {
    inner: AnyView,
}

impl Render for Framed {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        framed(
            // Child windows all ask for the OS frame, so the ask is baked
            // in here rather than passed.
            WindowDecorations::Server,
            self.inner.clone().into_any_element(),
            window,
            // Nothing hangs off a child window's teardown, so the close is
            // the plain one. The same call the window controls panel makes
            // in a popped-out window.
            |_, window, _| window.remove_window(),
        )
    }
}

/// Wrap a built view for [`Framed`], the shape `open_window` needs.
pub fn wrap<V: 'static + Render>(view: Entity<V>, cx: &mut App) -> Entity<Framed> {
    cx.new(|_| Framed { inner: view.into() })
}

/// The window's own title, for the strip to show. Falls back to the app
/// name for a window that never titled itself, which beats an empty strip.
fn window_title(window: &Window) -> SharedString {
    let id = window.window_handle().window_id().as_u64();
    crate::windows::window_title(id)
        .map(SharedString::from)
        .unwrap_or_else(|| "rox".into())
}
