//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    anchored, deferred, div, prelude::*, px, rgb, size, svg, AnyElement, App, Bounds, Context,
    DismissEvent, Div, Entity, Focusable as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Root;
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};

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

/// The flat icon button the transport panels share so the button style
/// never forks: the icon alone at rest, a soft pill behind it on hover.
/// Icon paths come from [`crate::assets::icons`].
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: u32,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .p_1p5()
        .rounded_md()
        .hover(|d| d.bg(rgb(0x2a2a2a)))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_click(this, cx)),
        )
        .child(svg().path(icon).size_4().text_color(rgb(color)))
}

/// The shared state of a click-and-drag strip: where it painted and
/// whether a drag is live. Behind Arcs so the panel, its paint closures,
/// and the window-level mouse handlers can all hold it.
#[derive(Clone, Default)]
pub struct ScrubState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
}

impl ScrubState {
    /// Remember where the strip landed, from its prepaint.
    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        *self.bounds.lock().unwrap() = Some(bounds);
    }

    /// A drag started (mouse down on the strip).
    pub fn begin(&self) {
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    /// Where `x` lands along the strip, 0 to 1; positions off the ends
    /// clamp, so a drag can overshoot without letting go of the value.
    pub fn fraction(&self, x: Pixels) -> Option<f32> {
        let bounds = (*self.bounds.lock().unwrap())?;
        let w = f32::from(bounds.size.width);
        if w <= 0.0 {
            return None;
        }
        Some((f32::from(x - bounds.origin.x) / w).clamp(0.0, 1.0))
    }
}

/// Keep a live drag following the pointer: apply the strip fraction on
/// every move, end the drag on release. Call from the strip's paint pass -
/// window handlers only live one frame, the same idiom the dock's resize
/// handles use. Applying must notify an entity so the next frame re-arms
/// the handlers.
pub fn scrub_on_paint(
    scrub: &ScrubState,
    window: &mut Window,
    apply: impl Fn(f32, &mut App) + 'static,
) {
    if !scrub.is_dragging() {
        return;
    }
    window.on_mouse_event({
        let scrub = scrub.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() || !scrub.is_dragging() {
                return;
            }
            // A release outside the window never reaches the up handler;
            // a move without the button still held ends the drag instead.
            if event.pressed_button != Some(MouseButton::Left) {
                scrub.end();
                return;
            }
            if let Some(fraction) = scrub.fraction(event.position.x) {
                apply(fraction, cx);
            }
        }
    });
    window.on_mouse_event({
        let scrub = scrub.clone();
        move |_: &MouseUpEvent, phase, _, _| {
            if phase.bubble() {
                scrub.end();
            }
        }
    });
}

/// Map a strip fraction to an absolute seek on the playing track, the
/// seek strip's and the waveform's shared apply.
pub fn seek_fraction(player: &Entity<Player>, fraction: f32, cx: &App) {
    let player = player.read(cx);
    let Some(now) = player.now_playing() else {
        return;
    };
    let Some(duration) = now.duration_secs else {
        return;
    };
    player.seek_to(fraction as f64 * duration);
}

/// Read a panel's config back out of a dumped panel state; anything
/// missing or malformed falls back to defaults.
pub fn config_from_info<C: Default + serde::de::DeserializeOwned>(info: &PanelInfo) -> C {
    match info {
        PanelInfo::Panel(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        _ => C::default(),
    }
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
    menu.item(
        PopupMenuItem::new("Duplicate").on_click(move |_, window, cx| {
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
        }),
    )
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
    menu.item(
        PopupMenuItem::new("Pop Out").on_click(move |_, window, cx| {
            pop_out(panel.clone(), tab_panel.clone(), state.clone(), window, cx);
        }),
    )
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

/// A panel whose per-view config can be edited live in a customize window.
/// New knobs (colors, layout, whatever a panel grows) go on the panel's
/// config struct and get a row here.
pub trait Customizable: Panel {
    /// The customize window's control rows, editing the config in place.
    /// Changes apply live; the layout dump persists them.
    fn customize(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement;
}

/// The Customize entry for a panel's dropdown menu: opens the panel's
/// customize window.
pub fn customize_item<P: Customizable>(menu: PopupMenu, panel: &Entity<P>) -> PopupMenu {
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Customize...").on_click(move |_, _, cx| {
            open_customize(panel.clone(), cx);
        }),
    )
}

/// Open the small window that edits a panel's config. It holds the panel
/// weakly, so it never keeps a closed panel alive.
fn open_customize<P: Customizable>(panel: Entity<P>, cx: &mut App) {
    let title = panel.read(cx).panel_name();
    let bounds = Bounds::centered(None, size(px(360.), px(180.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("rox - customize {title}").into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let panel = panel.downgrade();
    cx.open_window(options, move |window, cx| {
        let host = cx.new(|cx| {
            let _panel_changed = panel
                .upgrade()
                .map(|panel| cx.observe(&panel, |_, _, cx| cx.notify()));
            CustomizeHost {
                panel,
                _panel_changed,
            }
        });
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the customize window");
}

/// The customize window's content: the panel's own control rows on the
/// workspace's base styling.
struct CustomizeHost<P: Customizable> {
    panel: WeakEntity<P>,
    /// Repaints this window when the panel changes from anywhere else.
    _panel_changed: Option<Subscription>,
}

impl<P: Customizable> Render for CustomizeHost<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.panel.upgrade() {
            Some(panel) => panel.update(cx, |panel, cx| panel.customize(window, cx)),
            None => div()
                .text_color(rgb(0x808080))
                .child("the panel was closed")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0x1c1c1c))
            .text_color(rgb(0xe0e0e0))
            .text_sm()
            .child(body)
    }
}

/// One labeled row of a customize window: the setting's name and its
/// control on one line, an optional dimmed description wrapping below.
pub fn setting_row(
    label: &'static str,
    description: Option<&'static str>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(label)
                .child(div().flex_none().child(control)),
        )
        .when_some(description, |d, description| {
            d.child(div().text_xs().text_color(rgb(0x808080)).child(description))
        })
}

/// An on/off switch: a pill track, the knob in the accent on the far side
/// while on.
pub fn toggle<P: 'static>(
    on: bool,
    on_change: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    div()
        .w(px(34.))
        .h(px(18.))
        .flex_none()
        .rounded_full()
        .bg(rgb(0x2a2a2a))
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .p(px(2.))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_change(this, !on, cx)),
        )
        .child(div().size(px(14.)).rounded_full().bg(if on {
            rgb(0xfdcb00)
        } else {
            rgb(0x707070)
        }))
}

/// The chrome shared by the segmented pickers: a joined group of segments,
/// the picked one filled with the accent, hairline gaps between the rest.
fn segments<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    render: impl Fn(&'static str, bool) -> AnyElement,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let last = options.len().saturating_sub(1);
    let mut group = div().flex().flex_row().flex_none().items_center();
    for (i, (key, value)) in options.iter().enumerate() {
        let value = *value;
        let picked = value == current;
        let on_pick = on_pick.clone();
        group = group.child(
            div()
                .px_2()
                .py_1()
                .when(i > 0, |d| d.ml(px(1.)))
                .when(i == 0, |d| d.rounded_l_md())
                .when(i == last, |d| d.rounded_r_md())
                .bg(if picked { rgb(0xfdcb00) } else { rgb(0x2a2a2a) })
                .when(!picked, |d| d.hover(|d| d.bg(rgb(0x3a3a3a))))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                )
                .child(render(key, picked)),
        );
    }
    group
}

/// A segmented picker of exclusive choices, labeled with text.
pub fn choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |label, picked| {
            div()
                .text_color(if picked { rgb(0x121212) } else { rgb(0xc0c0c0) })
                .child(label)
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// A segmented picker of exclusive choices, labeled with icons; each option
/// pairs an icon path from [`crate::assets::icons`] with its value.
pub fn icon_choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |icon, picked| {
            svg()
                .path(icon)
                .size_4()
                .text_color(if picked { rgb(0x121212) } else { rgb(0xc0c0c0) })
                .into_any_element()
        },
        on_pick,
        cx,
    )
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
