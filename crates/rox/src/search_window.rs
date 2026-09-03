//! The power search window: the quick-play view in a window of its own,
//! sized to fill it instead of floating over the workspace.
//!
//! Two ways in, one window. From the Library menu it opens empty, as a place
//! to search the whole library without disturbing whatever the workspace is
//! currently showing. From the health window's drill-downs it opens seeded:
//! the offending tracks arrive as an explicit set the search runs inside,
//! named by the chip under the box, and typing narrows within it.
//!
//! Quick play rather than a library panel because it's already the search
//! surface: the suggestion menu on the box, artist and album heads over the
//! track hits, and a keyboard path from the box into the results. It only
//! needed to know it's filling a frame rather than covering one.
//!
//! Seeded is why this window exists at all. The drill-downs used to narrow
//! the shared query, which meant a look at 3,000 tracks missing album art
//! threw away whatever the user had in the library view and left every
//! global-following panel in the app pointing at a diagnostic. A window of
//! its own costs nothing the shared query was buying and takes nothing away.
//!
//! Singleton, like every other child window here: a second open activates the
//! one that's up. A second seeded open replaces the seed rather than stacking
//! a window, so clicking through several tiles in a row walks one window
//! through several answers.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, DismissEvent, Entity, Focusable, Global,
    WeakEntity, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::settings::{SearchWindowState, Settings};
use rox_design::palette;
use rox_panel_api::panel::{self, AppState};

use crate::quick_play::{self, QuickPlay};

/// Wide enough for the search box, the result rows and the hint footer under
/// them. The view is the whole window, so anything narrower is a list of
/// truncations.
const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(520.),
    height: px(320.),
};

/// What a still-open power search window is: the window itself, and the view
/// inside it that a second open reaches through to reseed or retype.
struct OpenSearch {
    window: WindowHandle<Root>,
    view: WeakEntity<SearchWindow>,
}

impl Global for OpenSearch {}

/// What a second open does to the view that's already up, once the window
/// has been raised.
enum Reopen {
    /// Nothing: the menu's door means "let me search", not "throw away what
    /// I was looking at", so an open window keeps its seed and its text.
    Keep,
    /// Replace the seed, the health window's id drill-downs.
    Seed(quick_play::Seed),
    /// Put text in the box, keeping whatever seed is set.
    Query(String),
}

/// Open the power search window, or bring the open one to the front.
pub fn open(state: AppState, cx: &mut App) {
    open_with(state, Reopen::Keep, cx);
}

/// Open the window over an explicit set of tracks, or point the open one at
/// it. The seed's label becomes the chip under the search box, since a set
/// somebody else chose is otherwise invisible: the rows are simply fewer
/// than the library has.
pub fn open_seeded(state: AppState, seed: quick_play::Seed, cx: &mut App) {
    open_with(state, Reopen::Seed(seed), cx);
}

/// Open the window with a query already typed, or type it into the open one.
/// The door for a drill-down the query language can say by itself, like
/// every track with no genre.
pub fn open_with_query(state: AppState, query: &str, cx: &mut App) {
    open_with(state, Reopen::Query(query.to_string()), cx);
}

fn open_with(state: AppState, reopen: Reopen, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenSearch>() {
        let window = open.window;
        let view = open.view.clone();
        // Raising comes first and on its own, because a window whose frame
        // is already gone leaves us to open a fresh one, and the reopen has
        // to still be in hand for that.
        let raised = window
            .update(cx, |_, window, cx| {
                window.activate_window();
                let Some(view) = view.upgrade() else {
                    return false;
                };
                // Typing should land in the box the moment the window comes
                // up, whether it's new or was already sitting behind
                // something.
                let focus = view.read(cx).quick_play.read(cx).focus_handle(cx);
                window.focus(&focus);
                true
            })
            .unwrap_or(false);
        if raised {
            window
                .update(cx, |_, window, cx| {
                    if let Some(view) = view.upgrade() {
                        view.update(cx, |this, cx| this.reopen(reopen, window, cx));
                    }
                })
                .ok();
            return;
        }
    }
    // The last closed window's size, sanity-floored, the health window's
    // restore shape.
    let (width, height) = Settings::load()
        .windows
        .search
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((900., 600.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    // The builder runs inside the open, so this is a handle by the time the
    // global is set; it's a cell only because the closure has to hand it
    // back out.
    let built: Rc<RefCell<Option<WeakEntity<SearchWindow>>>> = Rc::default();
    let sink = built.clone();
    let window = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("search-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| {
            let view = cx.new(|cx| SearchWindow::new(state, reopen, window, cx));
            let focus = view.read(cx).quick_play.read(cx).focus_handle(cx);
            window.focus(&focus);
            *sink.borrow_mut() = Some(view.downgrade());
            view
        },
    );
    let Some(view) = built.borrow().clone() else {
        return;
    };
    cx.set_global(OpenSearch { window, view });
}

/// Write the frame's size back to settings, so the next open comes up the
/// shape this one was left at.
fn save_frame(window: &Window) {
    let frame = window.window_bounds().get_bounds();
    Settings::update(move |s| {
        let saved = s
            .windows
            .search
            .get_or_insert_with(SearchWindowState::default);
        saved.width = frame.size.width.into();
        saved.height = frame.size.height.into();
    });
}

struct SearchWindow {
    /// The shared state, for the window's art tint.
    state: AppState,
    /// The one view this window hosts, filling it.
    quick_play: Entity<QuickPlay>,
    _dismissed: gpui::Subscription,
}

impl SearchWindow {
    fn new(state: AppState, reopen: Reopen, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The OS close button never runs remove_window, so the frame
        // persists through the should-close hook, the way the other child
        // windows do it.
        window.on_window_should_close(cx, |window, _| {
            save_frame(window);
            true
        });
        let quick_play = cx.new(|cx| QuickPlay::new(state.clone(), window, cx));
        quick_play.update(cx, |quick_play, _| quick_play.set_hosted(true));
        // Escape means "close the search" wherever the search is; over the
        // workspace that drops the overlay, and here it's the window that
        // goes.
        let _dismissed =
            cx.subscribe_in(&quick_play, window, |_, _, _: &DismissEvent, window, _| {
                save_frame(window);
                window.remove_window();
            });
        let this = SearchWindow {
            state,
            quick_play,
            _dismissed,
        };
        this.reopen(reopen, window, cx);
        this
    }

    /// Apply what a fresh or repeated open asked for. Replacing rather than
    /// stacking a window is the choice: several drill-downs in a row are one
    /// line of questioning, not several.
    fn reopen(&self, reopen: Reopen, window: &mut Window, cx: &mut Context<Self>) {
        match reopen {
            Reopen::Keep => {}
            Reopen::Seed(seed) => self
                .quick_play
                .update(cx, |quick_play, cx| quick_play.set_seed(Some(seed), cx)),
            Reopen::Query(query) => self.quick_play.update(cx, |quick_play, cx| {
                quick_play.set_query(&query, window, cx)
            }),
        }
    }
}

impl Render for SearchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Tinted by the playing track like the window that opened it, and
        // claiming the widget theme while it holds focus, the health
        // window's move.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(self.quick_play.clone())
                .into_any_element()
        })
    }
}
