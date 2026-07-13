//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::sync::Arc;

use gpui::{
    anchored, deferred, div, prelude::*, px, relative, rgb, size, App, Bounds, Context,
    DismissEvent, Entity, Focusable as _, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Root;
use rox_dock::{Panel, PanelView, TabPanel};

use crate::library::Library;
use crate::player::Player;

/// The shared entities every panel renders over: one player and one catalog
/// per workspace. Cloning shares the handles, not the state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
    pub tab_hosts: Entity<TabHosts>,
}

/// Every tab panel that has hosted one of our panels, reported from each
/// panel's `on_added_to`. Dragging a tab into a split makes the dock create
/// tab panels on its own and nothing announces them to the workspace, so
/// this registry is how it finds them, to pick a live tab panel for
/// View-menu additions.
#[derive(Default)]
pub struct TabHosts {
    hosts: Vec<WeakEntity<TabPanel>>,
}

impl TabHosts {
    /// Record a hosting tab panel.
    pub fn report(&mut self, tabs: WeakEntity<TabPanel>) {
        if self.hosts.iter().any(|t| t.entity_id() == tabs.entity_id()) {
            return;
        }
        self.hosts.push(tabs);
    }

    /// The newest recorded tab panel that is still alive and showing panels.
    pub fn last_live(&self, cx: &App) -> Option<Entity<TabPanel>> {
        self.hosts.iter().rev().find_map(|tabs| {
            let tabs = tabs.upgrade()?;
            tabs.read(cx).visible(cx).then_some(tabs)
        })
    }
}

/// The compact clickable control chip the player bar introduced, shared
/// with the transport panels so the button style never forks.
pub fn control<V: 'static>(
    label: impl Into<SharedString>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0x2a2a2a))
        .hover(|d| d.bg(rgb(0x3a3a3a)))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_click(this, cx)),
        )
        .child(label.into())
}

/// The level meter strip: level as the filled share of a fixed track.
pub fn meter(level: f32) -> impl IntoElement {
    div()
        .w(px(60.))
        .h(px(6.))
        .flex_none()
        .rounded_sm()
        .bg(rgb(0x2a2a2a))
        .child(
            div()
                .h_full()
                .rounded_sm()
                .bg(rgb(0x3dff9c))
                .w(relative(level)),
        )
}

/// A panel whose duplicate is just a fresh view over the shared state, no
/// per-view config to carry across. The library panel keeps its own
/// duplicate wiring because its copy takes the query along.
pub trait StatePanel: Panel {
    fn state(&self) -> AppState;
    fn tab_panel(&self) -> Option<WeakEntity<TabPanel>>;
    fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self;
}

/// The Duplicate entry for a panel's dropdown menu, which the dock serves
/// on right-click and from the tab bar's ellipsis button. Duplicates into
/// the tab panel the original sits in.
pub fn duplicate_item<P: StatePanel>(menu: PopupMenu, panel: &Entity<P>) -> PopupMenu {
    let weak = panel.downgrade();
    menu.item(PopupMenuItem::new("Duplicate").on_click(move |_, window, cx| {
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
    }))
}

/// The Pop Out entry for a panel's dropdown menu: moves the panel out of its
/// dock into an OS window. Pass the tab panel the panel currently sits in
/// (from `on_added_to`); the state is what Dock Back later reaches the
/// workspace through.
pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
) -> PopupMenu {
    let panel = panel.clone();
    menu.item(PopupMenuItem::new("Pop Out").on_click(move |_, window, cx| {
        pop_out(panel.clone(), tab_panel.clone(), state.clone(), window, cx);
    }))
}

/// Move a docked panel into its own OS window. The panel entity itself moves,
/// so it keeps rendering the same shared state; closing the window drops it.
pub fn pop_out<P: Panel>(
    panel: Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
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
    pop_out_view(Arc::new(panel), state, cx);
}

/// Open an OS window hosting an already-detached panel. Also the dock's
/// middle-drag-out hook: dragging a panel out of the window lands here.
/// The window title comes from the panel's name.
pub fn pop_out_view(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    let title = panel.panel_name(cx);
    let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("rox - {title}").into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        let host = cx.new(|_| PopoutHost {
            panel_view: panel,
            state,
            context_menu: None,
        });
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the panel window");
}

/// A popped-out panel's window content: the moved panel view, full-size, on
/// the same base styling the workspace root applies. Right-click offers the
/// way back into the dock.
struct PopoutHost {
    panel_view: Arc<dyn PanelView>,
    state: AppState,
    /// The open right-click menu: its anchor position, the menu, and the
    /// dismiss subscription that clears it.
    context_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
}

impl PopoutHost {
    /// Open the right-click menu. Dock Back moves the panel into the newest
    /// live tab group of the workspace and closes this window; cross-window
    /// drags can't work (a held button pins pointer events to its window,
    /// and Wayland hides window positions), so this is the way home.
    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.panel_view.clone();
        let hosts = self.state.tab_hosts.clone();
        let dockable = hosts.read(cx).last_live(cx).is_some();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(
                PopupMenuItem::new("Dock Back")
                    .disabled(!dockable)
                    .on_click(move |_, window, cx| {
                        let Some(tabs) = hosts.read(cx).last_live(cx) else {
                            return;
                        };
                        tabs.update(cx, |tabs, cx| {
                            tabs.add_panel(panel.clone(), window, cx);
                        });
                        window.remove_window();
                    }),
            )
        });
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((position, menu, subscription));
        cx.notify();
    }
}

impl Render for PopoutHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.open_menu(event.position, window, cx);
                }),
            )
            .child(self.panel_view.view())
            // Same overlay structure as the dock's context menu: an
            // occluding layer swallows the dismissing click, the anchored
            // child pins the menu to the pointer.
            .when_some(self.context_menu.as_ref(), |this, (position, menu, _)| {
                this.child(
                    deferred(
                        anchored().child(
                            div()
                                .w(window.bounds().size.width)
                                .h(window.bounds().size.height)
                                .occlude()
                                .child(
                                    anchored()
                                        .position(*position)
                                        .snap_to_window_with_margin(px(8.))
                                        .child(menu.clone()),
                                ),
                        ),
                    )
                    .with_priority(1),
                )
            })
    }
}
