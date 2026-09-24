//! The power search window: the quick-play view in a window of its own. From
//! the Library menu it opens empty; from the health window's drill-downs it
//! opens seeded, so the offending tracks arrive as a set the search runs
//! inside.
//!
//! A drill-down never narrows the shared query: that would throw away the
//! user's library view and point every global-following panel at a diagnostic.
//!
//! Singleton. A second seeded open replaces the seed rather than stacking a
//! window.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, DismissEvent, Entity, Focusable, Global, WeakEntity, Window,
    WindowHandle, div, prelude::*, px, size,
};
use gpui_component::Root;

use rox_core::settings::{SearchWindowState, Settings};
use rox_design::palette;
use rox_panel_api::panel::{self, AppState};

use crate::quick_play::{self, QuickPlay};

const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(520.),
    height: px(320.),
};

struct OpenSearch {
    window: WindowHandle<Root>,
    view: WeakEntity<SearchWindow>,
}

impl Global for OpenSearch {}

enum Reopen {
    /// An open window keeps its seed and text when opened from the menu.
    Keep,
    Seed(quick_play::Seed),
    Query(String),
}

pub fn open(state: AppState, cx: &mut App) {
    open_with(state, Reopen::Keep, cx);
}

/// The seed's label becomes the chip under the search box, or the set would be
/// invisible.
pub fn open_seeded(state: AppState, seed: quick_play::Seed, cx: &mut App) {
    open_with(state, Reopen::Seed(seed), cx);
}

pub fn open_with_query(state: AppState, query: &str, cx: &mut App) {
    open_with(state, Reopen::Query(query.to_string()), cx);
}

fn open_with(state: AppState, reopen: Reopen, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenSearch>() {
        let window = open.window;
        let view = open.view.clone();
        // Raise first and on its own: a window whose frame is gone falls
        // through to opening a fresh one, which still needs the reopen.
        let raised = window
            .update(cx, |_, window, cx| {
                window.activate_window();
                let Some(view) = view.upgrade() else {
                    return false;
                };
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
    let (width, height) = Settings::load()
        .windows
        .search
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((900., 600.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    // The builder runs inside the open; the cell only hands the view back out.
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
    state: AppState,
    quick_play: Entity<QuickPlay>,
    _dismissed: gpui::Subscription,
}

impl SearchWindow {
    fn new(state: AppState, reopen: Reopen, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The OS close button never runs remove_window, so the frame persists
        // through the should-close hook.
        window.on_window_should_close(cx, |window, _| {
            save_frame(window);
            true
        });
        let quick_play = cx.new(|cx| QuickPlay::new(state.clone(), window, cx));
        quick_play.update(cx, |quick_play, _| quick_play.set_hosted(true));
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
