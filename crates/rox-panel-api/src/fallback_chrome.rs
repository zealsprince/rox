//! The titlebar a window grows when the compositor won't give it one.
//!
//! Wayland compositors without `zxdg_decoration_manager_v1` (GNOME's
//! mutter) hand back a bare surface, leaving dialogs with no close button
//! and no edge to drag (issue #128). A window that asked for the OS frame
//! and didn't get it draws one strip with the title and the three buttons.
//! A workspace window that went bare on purpose keeps its own arrangement,
//! since its layout carries the window controls panel. A child window has
//! no such panel, so it takes the strip whenever it's bare. Resize grips go
//! on any bare window.

use gpui::{
    AnyElement, AnyView, App, Context, Entity, EntityId, MouseButton, MouseDownEvent, Render,
    SharedString, Window, WindowDecorations, div, prelude::*, px,
};
use rox_core::settings::{self, ChromeSide, ChromeStyle};
use rox_design::{palette, tokens};
use rox_panel_kit::{chrome_missing, icon_controls, resize_grips, traffic_lights};

const BAR_HEIGHT: gpui::Pixels = px(32.);

/// Wrap a window's body in the chrome the compositor didn't supply, or hand
/// the body straight back when it did. `asked` is the decorations the
/// window requested. `player` picks the tint the strip draws under, the one
/// the body's `WindowTint` uses; None draws it in the base theme. The
/// strip's text comes off the window title registry, so a retitle after
/// opening shows up without plumbing. `close` is the caller's because a
/// workspace window runs its teardown first.
pub fn framed(
    asked: WindowDecorations,
    body: AnyElement,
    window: &Window,
    player: Option<EntityId>,
    close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let grips = resize_grips(window);

    // Under the window's tint like the body, or the strip reads the base
    // theme and sits grey over a song-tinted window.
    let strip = chrome_missing(asked, window).then(|| {
        let title = window_title(window);
        let strip = move || titlebar(title, window, close).into_any_element();
        match player {
            Some(player) => rox_panel_kit::window_body(player, strip).into_any_element(),
            None => strip(),
        }
    });

    if strip.is_none() && grips.is_none() {
        return body;
    }

    // The body keeps the parent it has without the strip, a plain full-size
    // block, and the strip sits in padding above it. Put in a flex column
    // item instead, inputs inside it laid out at their content width: the
    // settings search collapsed to its two icons.
    let has_strip = strip.is_some();

    div()
        .relative()
        .size_full()
        .when(has_strip, |d| d.pt(BAR_HEIGHT))
        .child(body)
        .children(strip.map(|strip| div().absolute().top_0().left_0().right_0().child(strip)))
        // Last, so the grips paint over whatever content reaches the edge.
        .children(grips)
        .into_any_element()
}

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

    // The move handle is the label, not the strip: gpui hands a mouse down to
    // every hitbox under it, so a strip handler would start a move on a click
    // on close.
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
            // A move grab on the first press would swallow the maximize double-click.
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

/// The view every child window is wrapped in. Always in place, even where
/// the compositor decorates, so there's one window tree; [`framed`] hands
/// the body straight back then.
pub struct Framed {
    inner: AnyView,
}

impl Render for Framed {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `Server` even for a child that opened bare on purpose: it has no window
        // controls panel, so the strip draws whenever the frame is missing. Unless
        // the user asked bare child windows to go without it.
        let asked = if settings::bare_child_windows() && !settings::child_titlebar() {
            WindowDecorations::Client
        } else {
            WindowDecorations::Server
        };

        // A popout is on the window registry. Settings and the editors aren't,
        // so they take the focused window's player, normally the one they were
        // opened from.
        let player = crate::panel::shader::window_player(window, cx)
            .map(|player| player.entity_id())
            .or_else(palette::focused_player);

        framed(
            asked,
            self.inner.clone().into_any_element(),
            window,
            player,
            |_, window, _| window.remove_window(),
        )
    }
}

pub fn wrap<V: 'static + Render>(view: Entity<V>, cx: &mut App) -> Entity<Framed> {
    cx.new(|_| Framed { inner: view.into() })
}

fn window_title(window: &Window) -> SharedString {
    let id = window.window_handle().window_id().as_u64();
    crate::windows::window_title(id)
        .map(SharedString::from)
        .unwrap_or_else(|| "rox".into())
}
