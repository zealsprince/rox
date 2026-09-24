//! The drawer panel: a main panel with a second tucked against one edge behind
//! a labeled handle, sliding out on hover and home when the pointer leaves.
//! Hosted through [`crate::composite`].
//!
//! A drawer can also open on a pick, chaining two panels: an album wall in the
//! main slot, its tracks in the drawer. That pick lands outside the drawer, so
//! a selection-opened drawer waits for the pointer to arrive before leaving
//! counts as a dismissal.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    Along, AnyElement, App, Axis, Bounds, Context, Div, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseMoveEvent, Pixels, SharedString, Subscription,
    WeakEntity, Window, canvas, div, point, prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::composite::{self, Slot};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::palette::Sides;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{ScrubState, choices_shared, setting_row};
use rox_services::selection::SelectionEvent;

const HANDLE: Pixels = px(18.);

/// Floor of the reveal fraction, so the open drawer can't shrink into its
/// handle.
const MIN_REVEAL: f32 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerEdge {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

impl DrawerEdge {
    fn axis(self) -> Axis {
        match self {
            DrawerEdge::Top | DrawerEdge::Bottom => Axis::Vertical,
            DrawerEdge::Left | DrawerEdge::Right => Axis::Horizontal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerTrigger {
    #[default]
    Hover,
    Selection,
}

/// The selection is app-wide, so without a scope every selection drawer opens
/// on every pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerScope {
    #[default]
    Main,
    /// Picks anywhere else in the layout, never its own contents. A tabbed
    /// browser drives its drawer this way, since dock tabs can't go in a slot.
    Any,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawerConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub edge: DrawerEdge,
    pub reveal: f32,
    pub dim: f32,
    pub open_on: DrawerTrigger,
    pub scope: DrawerScope,
    /// Hide the handle until a pick brings the drawer out. Ignored on the hover
    /// trigger, which would leave nothing to open it.
    pub hide_handle: bool,
}

impl Default for DrawerConfig {
    fn default() -> Self {
        DrawerConfig {
            chrome: PanelChrome::default(),
            edge: DrawerEdge::default(),
            reveal: 1.0,
            dim: 0.0,
            open_on: DrawerTrigger::default(),
            scope: DrawerScope::default(),
            hide_handle: false,
        }
    }
}

impl DrawerConfig {
    fn handle_hidden(&self) -> bool {
        self.hide_handle && self.open_on == DrawerTrigger::Selection
    }
}

/// The grip comes back once a pick primes the drawer, so a drawer that folded
/// home can be pulled out again by hand.
fn handle_dropped(config: &DrawerConfig, primed: bool) -> bool {
    config.handle_hidden() && !primed
}

pub struct DrawerPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: DrawerConfig,
    /// Main at 0, drawer at 1.
    slots: [Slot; 2],
    introduced: bool,
    active: bool,
    /// Never persisted: a restore starts home.
    open: bool,
    pinned: bool,
    /// Whether the pointer has been inside since it opened. A pick opens the
    /// drawer with the pointer outside, so leaving only dismisses once it has
    /// arrived.
    entered: bool,
    /// Applied on the next render, which has the window the child's active
    /// state needs.
    pending_open: Option<bool>,
    primed: bool,
    from: f32,
    open_at: Instant,
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    reveal_scrub: ScrubState,
    dim_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _selection: Subscription,
}

impl DrawerPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: DrawerConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: DrawerConfig,
        slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut fixed: [Slot; 2] = [None, None];
        for (slot, restored) in fixed.iter_mut().zip(slots) {
            *slot = restored;
        }
        let _selection = cx.subscribe(
            &state.selection.clone(),
            |this: &mut Self, selection, event: &SelectionEvent, cx| {
                this.on_selection(&selection, event.source, cx);
            },
        );
        DrawerPanel {
            state,
            workspace,
            config,
            slots: fixed,
            active: false,
            open: false,
            pinned: false,
            entered: false,
            pending_open: None,
            primed: false,
            from: 0.0,
            open_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            bounds: Arc::default(),
            reveal_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            introduced: false,
            _selection,
        }
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    fn openness(&self) -> f32 {
        let u = (self.open_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let u = u * u * (3.0 - 2.0 * u);
        let target = if self.open { 1.0 } else { 0.0 };
        self.from + (target - self.from) * u
    }

    fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.from = self.openness();
        self.open = open;
        self.open_at = Instant::now();
        if let Some(drawer) = &self.slots[1] {
            drawer.set_active(self.active && open, window, cx);
        }
        cx.notify();
    }

    fn on_selection(
        &mut self,
        selection: &Entity<rox_services::selection::Selection>,
        source: EntityId,
        cx: &mut Context<Self>,
    ) {
        if self.config.open_on != DrawerTrigger::Selection || self.slots[1].is_none() {
            return;
        }
        // A pick inside the drawer never chains: it would pin the drawer to its
        // own contents.
        match self.config.scope {
            DrawerScope::Main => {
                if !self.slot_holds(0, source, cx) {
                    return;
                }
            }
            // A main that publishes its own picks would otherwise slide the
            // drawer over itself.
            DrawerScope::Any => {
                if self.slot_holds(0, source, cx) || self.slot_holds(1, source, cx) {
                    return;
                }
            }
        }
        let picked = !selection.read(cx).tracks().is_empty();
        self.primed = picked;
        self.pending_open = Some(picked);
        cx.notify();
    }

    fn slot_holds(&self, ix: usize, id: EntityId, cx: &App) -> bool {
        fn walk(slot: &Slot, id: EntityId, cx: &App) -> bool {
            let Some(panel) = slot else {
                return false;
            };
            if panel.panel_id(cx) == id {
                return true;
            }
            composite::hosted_children(panel, cx)
                .is_some_and(|kids| kids.iter().any(|kid| walk(kid, id, cx)))
        }
        walk(&self.slots[ix], id, cx)
    }

    fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        self.pinned = !self.pinned;
        cx.notify();
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        self.introduced = false;
        cx.notify();
    }

    fn set_reveal(&mut self, reveal: f32, cx: &mut Context<Self>) {
        self.config.reveal = reveal;
        cx.notify();
    }

    fn set_dim(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.dim = fraction;
        cx.notify();
    }

    fn main_content(&self, cx: &mut Context<Self>) -> Div {
        match &self.slots[0] {
            // The drawer opted out of the dock's body menu, so the slot serves
            // the right-click itself.
            Some(child) => composite::menu_routed_slot(child, &self.tab_panel, cx),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    "drawer-add-main",
                    self.state.clone(),
                    self.workspace.clone(),
                    move |panel, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.set_slot(0, Some(panel), cx));
                        }
                    },
                )
            }
        }
    }

    fn drawer_box(&self, child: Arc<dyn PanelView>, u: f32, cx: &mut Context<Self>) -> Div {
        let edge = self.config.edge;
        let axis = edge.axis();
        let extent = px(self.extent(u));

        let name = child
            .tab_name(cx)
            .unwrap_or_else(|| SharedString::from(panel::display_name(child.panel_name(cx))));
        let grip = div()
            .rounded_full()
            .bg(if self.pinned {
                palette::accent()
            } else {
                palette::text_faint()
            })
            .map(|d| match axis {
                Axis::Vertical => d.w(px(32.)).h(px(4.)),
                Axis::Horizontal => d.w(px(4.)).h(px(32.)),
            });
        let handle = div()
            .flex_none()
            .map(|d| match axis {
                Axis::Vertical => d.h(HANDLE).w_full().flex_row(),
                Axis::Horizontal => d.w(HANDLE).h_full().flex_col(),
            })
            .flex()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .bg(palette::alpha(palette::bg_control(), 0xa0))
            .hover(|d| d.bg(palette::bg_control()))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, cx| {
                this.entered = true;
                this.set_open(true, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.open {
                        this.toggle_pin(cx);
                    } else {
                        this.entered = true;
                        this.set_open(true, window, cx);
                    }
                }),
            )
            .child(grip)
            .when(axis == Axis::Vertical, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(name),
                )
            });
        // The child only mounts while some of it shows.
        let content = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .when(u > 0.001, |d| {
                d.child(composite::menu_routed_slot(&child, &self.tab_panel, cx))
            });

        let boxed = div()
            .absolute()
            .flex()
            .map(|d| match edge {
                DrawerEdge::Top => d.left_0().right_0().top_0().h(extent).flex_col(),
                DrawerEdge::Bottom => d.left_0().right_0().bottom_0().h(extent).flex_col(),
                DrawerEdge::Left => d.top_0().bottom_0().left_0().w(extent).flex_row(),
                DrawerEdge::Right => d.top_0().bottom_0().right_0().w(extent).flex_row(),
            })
            // No fill of its own: the hosted panel's surface is the background.
            .border_color(palette::border())
            .map(|d| match edge {
                DrawerEdge::Top => d.border_b_1(),
                DrawerEdge::Bottom => d.border_t_1(),
                DrawerEdge::Left => d.border_r_1(),
                DrawerEdge::Right => d.border_l_1(),
            })
            .shadow_md()
            // Without occlusion, clicks land on the covered main too.
            .occlude();
        // The handle sits at the inner edge, so the stack order flips with the
        // edge.
        let handle = (!handle_dropped(&self.config, self.primed)).then_some(handle);
        let boxed = match edge {
            DrawerEdge::Bottom | DrawerEdge::Right => boxed.children(handle).child(content),
            DrawerEdge::Top | DrawerEdge::Left => boxed.child(content).children(handle),
        };
        // The chrome's surface shader goes over the box rather than the panel
        // (see render). Last child, so the screen pass samples the rest already
        // drawn.
        let surface = panel::shader::PanelSurface::build(&self.config.chrome, Sides::default());
        boxed.when_some(surface, |boxed, surface| {
            boxed.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| surface.paint(bounds, window, cx),
                )
                .absolute()
                .size_full(),
            )
        })
    }

    fn extent(&self, u: f32) -> f32 {
        let axis = self.config.edge.axis();
        let slot = self
            .bounds
            .lock()
            .unwrap()
            .map(|bounds| f32::from(bounds.size.along(axis)))
            .unwrap_or(0.0);
        let reveal = self.config.reveal.clamp(MIN_REVEAL, 1.0);
        let home = if handle_dropped(&self.config, self.primed) {
            0.0
        } else {
            f32::from(HANDLE)
        };
        let reveal_px = (reveal * slot).max(home);
        home + u * (reveal_px - home)
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The dock never sees a hosted panel, so the children offer this host
        // from their own menus.
        let drawer_title = rox_i18n::t!("drawer-title");
        composite::report_hosted(
            self.slots.iter().flatten(),
            self.config.chrome.title.as_deref().unwrap_or(&drawer_title),
            cx,
        );

        if let Some(open) = self.pending_open.take() {
            if open {
                self.entered = false;
            } else {
                self.pinned = false;
            }
            self.set_open(open, window, cx);
        }

        let u = self.openness();
        let settled = if self.open { 1.0 } else { 0.0 };
        if (u - settled).abs() > 0.001 {
            window.request_animation_frame();
        }

        let root = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // The handle keeps a lane on its edge: floating over the main, it
            // would swallow clicks on the main's scrollbar.
            .child({
                let edge = self.config.edge;
                // The lane stays even with the handle dropped, or the main
                // would relayout under the click that opened the drawer.
                let has_drawer = self.slots[1].is_some();
                div()
                    .absolute()
                    .inset_0()
                    .when(has_drawer, |d| match edge {
                        DrawerEdge::Top => d.top(HANDLE),
                        DrawerEdge::Bottom => d.bottom(HANDLE),
                        DrawerEdge::Left => d.left(HANDLE),
                        DrawerEdge::Right => d.right(HANDLE),
                    })
                    .overflow_hidden()
                    .child(self.main_content(cx))
            });

        let dim = self.config.dim.clamp(0.0, 1.0) * u;
        let root = if dim > 0.001 && self.slots[1].is_some() {
            root.child(div().absolute().inset_0().bg(palette::alpha(
                palette::bg_root_opaque(),
                (dim * 255.0).round() as u8,
            )))
        } else {
            root
        };

        // Window handlers last one frame, and hover can't close the drawer: a
        // press inside parks the hover state and an occluding scrollbar drops
        // it. The bounds check holds either way.
        let root = if self.slots[1].is_some() {
            let bounds_store = self.bounds.clone();
            let weak = cx.entity().downgrade();
            let armed = (self.open || u > 0.001) && !self.pinned;
            let entered = self.entered;
            let edge = self.config.edge;
            let extent = px(self.extent(u));
            root.child(
                canvas(
                    move |bounds, _, _| {
                        *bounds_store.lock().unwrap() = Some(bounds);
                    },
                    move |bounds, _, window, _| {
                        if !armed {
                            return;
                        }
                        let region = drawer_region(edge, extent, bounds);
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if !phase.bubble() {
                                return;
                            }
                            // Mid-press moves never close: a drag from inside may
                            // stray out.
                            if event.pressed_button.is_some() {
                                return;
                            }
                            if region.contains(&event.position) {
                                // The pointer arrived. Repaint so the next frame's
                                // handler, which sees `entered`, can close.
                                if !entered && let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.entered = true;
                                        cx.notify();
                                    });
                                }
                                return;
                            }
                            if !entered {
                                return;
                            }
                            if let Some(this) = weak.upgrade() {
                                this.update(cx, |this, cx| this.set_open(false, window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
        } else {
            root
        };

        let root = match self.slots[1].clone() {
            // Skipped at home with the handle dropped, or its border would line
            // the empty lane.
            Some(_) if handle_dropped(&self.config, self.primed) && u <= 0.001 => root,
            Some(drawer) => root.child(self.drawer_box(drawer, u, cx)),
            None => root,
        };

        // Finished layouts hide the builder's buttons; the Workspace page's
        // tree still swaps slots.
        if self.config.chrome.controls_hidden() {
            return root;
        }

        let shown = usize::from(self.open && self.slots[1].is_some());
        let controls = composite::corner_controls()
            .when(self.slots[1].is_none(), |d| {
                let add_weak = cx.entity().downgrade();
                let state = self.state.clone();
                let workspace = self.workspace.clone();
                d.child(
                    Button::new("drawer-add")
                        .icon(Icon::default().path(icons::PANEL_BOTTOM))
                        .small()
                        .ghost()
                        .tooltip(rox_i18n::t!("drawer-add-tooltip"))
                        .dropdown_menu(move |menu, window, cx| {
                            let add_weak = add_weak.clone();
                            composite::pick_items(
                                menu,
                                state.clone(),
                                workspace.clone(),
                                window,
                                cx,
                                move |panel, _, cx| {
                                    if let Some(this) = add_weak.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.set_slot(1, Some(panel), cx)
                                        });
                                    }
                                },
                            )
                        }),
                )
            })
            .children(self.slots[shown].clone().map(|child| {
                composite::slot_button(
                    ("drawer-slot", shown),
                    child,
                    self.state.clone(),
                    self.workspace.clone(),
                    move |this: &mut Self, panel, cx| this.set_slot(shown, Some(panel), cx),
                    move |this: &mut Self, cx| this.set_slot(shown, None, cx),
                    |menu, _| menu,
                    cx,
                )
            }));
        let parent = composite::parent_button(rox_i18n::t!("drawer-title"), cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

fn drawer_region(edge: DrawerEdge, extent: Pixels, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    match edge {
        DrawerEdge::Top => Bounds::new(bounds.origin, size(bounds.size.width, extent)),
        DrawerEdge::Bottom => Bounds::new(
            point(
                bounds.origin.x,
                bounds.origin.y + bounds.size.height - extent,
            ),
            size(bounds.size.width, extent),
        ),
        DrawerEdge::Left => Bounds::new(bounds.origin, size(extent, bounds.size.height)),
        DrawerEdge::Right => Bounds::new(
            point(
                bounds.origin.x + bounds.size.width - extent,
                bounds.origin.y,
            ),
            size(extent, bounds.size.height),
        ),
    }
}

impl PanelSettings for DrawerPanel {
    fn composite(&self) -> bool {
        true
    }

    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reveal = self.config.reveal.clamp(MIN_REVEAL, 1.0);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(setting_row(
                    rox_i18n::t!("drawer-edge"),
                    Some(rox_i18n::t!("drawer-edge.description")),
                    choices_shared(
                        &[
                            (rox_i18n::t!("drawer-edge-top"), DrawerEdge::Top),
                            (rox_i18n::t!("drawer-edge-bottom"), DrawerEdge::Bottom),
                            (rox_i18n::t!("side-left"), DrawerEdge::Left),
                            (rox_i18n::t!("side-right"), DrawerEdge::Right),
                        ],
                        self.config.edge,
                        |this: &mut Self, edge, cx| {
                            this.config.edge = edge;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("drawer-open-on"),
                    Some(rox_i18n::t!("drawer-open-on.description")),
                    choices_shared(
                        &[
                            (rox_i18n::t!("drawer-trigger-hover"), DrawerTrigger::Hover),
                            (
                                rox_i18n::t!("drawer-trigger-selection"),
                                DrawerTrigger::Selection,
                            ),
                        ],
                        self.config.open_on,
                        |this: &mut Self, trigger, cx| {
                            this.config.open_on = trigger;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.open_on == DrawerTrigger::Selection, |d| {
                    d.child(setting_row(
                        rox_i18n::t!("drawer-handle"),
                        Some(rox_i18n::t!("drawer-handle.description")),
                        panel::toggle(
                            !self.config.hide_handle,
                            |this: &mut Self, shown, cx| {
                                this.config.hide_handle = !shown;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("drawer-answers"),
                        Some(rox_i18n::t!("drawer-answers.description")),
                        choices_shared(
                            &[
                                (rox_i18n::t!("drawer-scope-main"), DrawerScope::Main),
                                (rox_i18n::t!("drawer-scope-elsewhere"), DrawerScope::Any),
                            ],
                            self.config.scope,
                            |this: &mut Self, scope, cx| {
                                this.config.scope = scope;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                })
                .child(setting_row(
                    rox_i18n::t!("drawer-reveal"),
                    Some(rox_i18n::t!("drawer-reveal.description")),
                    settings_ui::scalar(
                        &self.reveal_scrub,
                        &self.value_edit,
                        reveal * 100.0,
                        settings_ui::span(MIN_REVEAL * 100., 100., "%").hard(),
                        |this: &mut Self, percent, cx| this.set_reveal(percent / 100.0, cx),
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("drawer-dim"),
                    Some(rox_i18n::t!("drawer-dim.description")),
                    settings_ui::scalar(
                        &self.dim_scrub,
                        &self.value_edit,
                        self.config.dim.clamp(0.0, 1.0) * 100.0,
                        settings_ui::span(0., 100., "%").hard(),
                        |this: &mut Self, percent, cx| this.set_dim(percent / 100.0, cx),
                        cx,
                    ),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for DrawerPanel {}

impl Focusable for DrawerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for DrawerPanel {
    fn panel_name(&self) -> &'static str {
        "drawer"
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        composite::open_slot_settings(&self.slots, window, cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("drawer-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        if let Some(main) = &self.slots[0] {
            main.set_active(active, window, cx);
        }
        if let Some(drawer) = &self.slots[1] {
            drawer.set_active(active && self.open, window, cx);
        }
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state.children = composite::dump_slots(&self.slots, cx);
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.introduced = false;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.introduced = false;
        for child in self.slots.iter().flatten() {
            child.on_removed(window, cx);
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = menu.item(panel::check_row(
            rox_i18n::t!("drawer-pin-open"),
            Some(icons::PIN),
            |this: &Self| this.pinned,
            |this, cx| this.toggle_pin(cx),
            &cx.entity(),
        ));
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for DrawerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composite::introduce_slots(
            self.slots.iter().flatten(),
            &self.tab_panel,
            &mut self.introduced,
            window,
            cx,
        );
        // The shader goes on the drawer box, not the panel, whose rect also
        // covers the main.
        let mut chrome = self.config.chrome.clone();
        chrome.shader = None;
        panel::themed(&chrome, || self.body(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_dumps_load_as_hover_drawers() {
        let dump = serde_json::json!({
            "edge": "right",
            "reveal": 0.4,
            "dim": 0.0,
        });
        let config: DrawerConfig = serde_json::from_value(dump).unwrap();
        assert_eq!(config.edge, DrawerEdge::Right);
        assert_eq!(config.open_on, DrawerTrigger::Hover);
        assert_eq!(config.scope, DrawerScope::Main);
        assert!(!config.hide_handle);
    }

    #[test]
    fn a_dropped_handle_needs_a_pick_to_replace_it() {
        let hidden = DrawerConfig {
            open_on: DrawerTrigger::Selection,
            hide_handle: true,
            ..DrawerConfig::default()
        };
        assert!(hidden.handle_hidden());

        let no_way_in = DrawerConfig {
            open_on: DrawerTrigger::Hover,
            hide_handle: true,
            ..DrawerConfig::default()
        };
        assert!(!no_way_in.handle_hidden());

        assert!(!DrawerConfig::default().handle_hidden());
    }

    #[test]
    fn a_pick_brings_the_dropped_handle_back() {
        let config = DrawerConfig {
            open_on: DrawerTrigger::Selection,
            hide_handle: true,
            ..DrawerConfig::default()
        };
        assert!(handle_dropped(&config, false));
        assert!(!handle_dropped(&config, true));

        let plain = DrawerConfig {
            open_on: DrawerTrigger::Selection,
            ..DrawerConfig::default()
        };
        assert!(!handle_dropped(&plain, false));
        assert!(!handle_dropped(&plain, true));
    }

    #[test]
    fn trigger_knobs_round_trip() {
        let config = DrawerConfig {
            edge: DrawerEdge::Left,
            open_on: DrawerTrigger::Selection,
            scope: DrawerScope::Any,
            hide_handle: true,
            ..DrawerConfig::default()
        };
        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["open_on"], "selection");
        assert_eq!(value["scope"], "any");
        assert_eq!(value["hide_handle"], true);

        let back: DrawerConfig = serde_json::from_value(value).unwrap();
        assert_eq!(back.open_on, DrawerTrigger::Selection);
        assert_eq!(back.scope, DrawerScope::Any);
        assert_eq!(back.edge, DrawerEdge::Left);
        assert!(back.hide_handle);
    }

    /// Metro is the one bundled layout that ships the chain, so its drawers
    /// have to keep parsing.
    #[test]
    fn metro_ships_a_working_selection_drawer() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/workspaces/Metro.json")).unwrap();

        fn collect<'a>(node: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
            if node["panel_name"] == "drawer" {
                out.push(node);
            }
            for kid in node["children"].as_array().into_iter().flatten() {
                collect(kid, out);
            }
        }

        let mut drawers = Vec::new();
        collect(&doc["layouts"][0]["dump"]["center"], &mut drawers);
        // Two per browser tab: one drawer is one edge, and dock tabs can't
        // share a slot.
        assert_eq!(drawers.len(), 6);

        let mut titles = Vec::new();
        for outer in drawers
            .iter()
            .filter(|d| !d["info"]["panel"]["title"].is_null())
        {
            let config: DrawerConfig =
                serde_json::from_value(outer["info"]["panel"].clone()).unwrap();
            assert_eq!(config.open_on, DrawerTrigger::Hover);
            assert_eq!(config.edge, DrawerEdge::Right);
            titles.push(config.chrome.title.clone().expect("the tab is named"));
            assert_eq!(outer["children"][1]["panel_name"], "queue");
            assert!(config.chrome.hide_controls);

            let inner = &outer["children"][0];
            let inner_config: DrawerConfig =
                serde_json::from_value(inner["info"]["panel"].clone()).unwrap();
            assert_eq!(inner_config.open_on, DrawerTrigger::Selection);
            assert_eq!(inner_config.scope, DrawerScope::Main);
            assert_eq!(inner_config.edge, DrawerEdge::Left);
            assert!(inner_config.chrome.hide_controls);
            assert!(inner_config.handle_hidden());
            assert!(!config.handle_hidden());

            assert!(
                inner["children"][0]["panel_name"]
                    .as_str()
                    .is_some_and(|name| name.ends_with("grid"))
            );
            let slid_out = &inner["children"][1];
            assert_eq!(slid_out["panel_name"], "library");
            assert_eq!(slid_out["info"]["panel"]["query_source"], "selection");
        }
        assert_eq!(titles, ["Albums", "Artists", "Genres"]);
    }

    #[test]
    fn selection_query_source_round_trips() {
        use rox_panel_api::query::shared_query::QuerySource;
        let value = serde_json::to_value(QuerySource::Selection).unwrap();
        assert_eq!(value, "selection");
        let back: QuerySource = serde_json::from_value(value).unwrap();
        assert!(back == QuerySource::Selection);
    }
}
