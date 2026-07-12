//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, size, AnyView, App, Bounds, Context, Entity, SharedString,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, TabPanel};
use gpui_component::{IconName, Root};

use crate::library::Library;
use crate::player::Player;

/// The shared entities every panel renders over: one player and one catalog
/// per workspace. Cloning shares the handles, not the state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
}

/// A panel whose duplicate is just a fresh view over the shared state, no
/// per-view config to carry across. The library panel keeps its own
/// duplicate wiring because its copy takes the query along.
pub trait StatePanel: Panel {
    fn state(&self) -> AppState;
    fn tab_panel(&self) -> Option<WeakEntity<TabPanel>>;
    fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self;
}

/// The toolbar button that duplicates a panel into the tab panel it sits in.
pub fn duplicate_button<P: StatePanel>(panel: &Entity<P>) -> Button {
    let weak = panel.downgrade();
    Button::new("duplicate")
        .icon(IconName::Copy)
        .tooltip("duplicate this panel")
        .on_click(move |_, window, cx| {
            let Some(this) = weak.upgrade() else { return };
            let (state, tabs) = {
                let panel = this.read(cx);
                (panel.state(), panel.tab_panel())
            };
            let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                return;
            };
            let dup = cx.new(|cx| P::duplicate(state, cx));
            tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
        })
}

/// The toolbar button that pops a panel out of its dock into an OS window.
/// Pass the tab panel the panel currently sits in (from `on_added_to`).
pub fn popout_button<P: Panel>(
    panel: &Entity<P>,
    title: impl Into<SharedString>,
    tab_panel: Option<WeakEntity<TabPanel>>,
) -> Button {
    let panel = panel.clone();
    let title: SharedString = title.into();
    Button::new("pop-out")
        .icon(IconName::ExternalLink)
        .tooltip("pop out into its own window")
        .on_click(move |_, window, cx| {
            pop_out(panel.clone(), title.clone(), tab_panel.clone(), window, cx);
        })
}

/// Move a docked panel into its own OS window. The panel entity itself moves,
/// so it keeps rendering the same shared state; closing the window drops it.
pub fn pop_out<P: Panel>(
    panel: Entity<P>,
    title: SharedString,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) {
    // Detach from the dock first; the new window's host keeps the entity
    // alive from here on.
    if let Some(tabs) = tab_panel.and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(Arc::new(panel.clone()), window, cx);
        });
    }

    let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("rox - {title}").into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let panel: AnyView = panel.into();
    cx.open_window(options, move |window, cx| {
        let host = cx.new(|_| PopoutHost { panel });
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the panel window");
}

/// A popped-out panel's window content: the moved panel view, full-size, on
/// the same base styling the workspace root applies.
struct PopoutHost {
    panel: AnyView,
}

impl Render for PopoutHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .child(self.panel.clone())
    }
}
