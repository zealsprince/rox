//! The drawer panel: a main panel with a second tucked against one edge,
//! only a labeled handle showing until the pointer rests on it, then the
//! drawer slides out over the main and slides home when the pointer
//! leaves. For the surfaces worth a glance but not a slot of their own -
//! the queue rising over a transport group, filters over a library. The
//! edge and how much of the panel the open drawer covers are per-panel
//! settings. Hosted through [`crate::composite`]; the drawer costs
//! nothing once it settles home.
//!
//! A drawer can also open on a pick rather than a hover, which is what
//! chains two panels into one surface: an album wall in the main slot, a
//! track list in the drawer, and clicking a cover slides the tracks out
//! over the wall. The pick that opens it lands outside the drawer, so a
//! selection-opened drawer waits for the pointer to arrive before it
//! treats leaving as a dismissal, and a click on the handle pins it out
//! for the times the drawer is somewhere to work rather than glance.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, size, Along, AnyElement, App, Axis, Bounds, Context, Div,
    Entity, EntityId, EventEmitter, FocusHandle, Focusable, MouseButton, MouseMoveEvent, Pixels,
    SharedString, Subscription, WeakEntity, Window,
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
use rox_panel_kit::{choices, setting_row, ScrubState};
use rox_services::selection::SelectionEvent;

/// The handle strip's thickness: enough for the grip and label to read,
/// thin enough to stay a hint over the main.
const HANDLE: Pixels = px(18.);

/// The floor of the reveal fraction, so the slider can't shrink the open
/// drawer into its own handle.
const MIN_REVEAL: f32 = 0.15;

/// The edge the drawer rests against, and the direction it slides from.
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
    /// The axis the drawer's extent runs along: height for the horizontal
    /// edges, width for the vertical ones.
    fn axis(self) -> Axis {
        match self {
            DrawerEdge::Top | DrawerEdge::Bottom => Axis::Vertical,
            DrawerEdge::Left | DrawerEdge::Right => Axis::Horizontal,
        }
    }
}

const EDGE_CHOICES: &[(&str, DrawerEdge)] = &[
    ("Top", DrawerEdge::Top),
    ("Bottom", DrawerEdge::Bottom),
    ("Left", DrawerEdge::Left),
    ("Right", DrawerEdge::Right),
];

/// What slides the drawer out. Resting on the handle always works; this
/// picks whether a pick in the main panel is a second way in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerTrigger {
    /// The handle alone.
    #[default]
    Hover,
    /// A pick in the main panel slides the drawer out, and clearing the
    /// selection slides it home. The chaining mode.
    Selection,
}

const TRIGGER_CHOICES: &[(&str, DrawerTrigger)] = &[
    ("Hover", DrawerTrigger::Hover),
    ("Selection", DrawerTrigger::Selection),
];

/// Which picks a selection-triggered drawer answers. The selection is
/// app-wide, so without a scope every selection drawer in a layout opens on
/// every pick anywhere in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawerScope {
    /// Only picks made in this drawer's own main panel, nested hosts
    /// included. What chains one wall to one list.
    #[default]
    Main,
    /// Picks made anywhere else in the layout, for the drawer that answers a
    /// panel it doesn't host - real dock tabs can't live in a slot, so a
    /// tabbed browser drives its drawer this way. Its own contents are still
    /// excluded, main included.
    Any,
}

const SCOPE_CHOICES: &[(&str, DrawerScope)] = &[
    ("Main Panel", DrawerScope::Main),
    ("Elsewhere", DrawerScope::Any),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawerConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The edge the drawer rests against and slides out from.
    pub edge: DrawerEdge,
    /// How much of the panel the open drawer covers, a fraction of the
    /// slot along the drawer's axis.
    pub reveal: f32,
    /// How hard the scrim dims the main behind the open drawer, 0 leaving
    /// it bare to 1 blacking it out. Fades with the slide.
    pub dim: f32,
    /// What slides the drawer out, on top of the handle.
    pub open_on: DrawerTrigger,
    /// Which picks the selection trigger answers.
    pub scope: DrawerScope,
    /// Drop the handle strip, so nothing of the drawer shows until a pick
    /// brings it out. The edge lane stays reserved either way: the grip
    /// comes back with the first pick, and if the lane came back with it the
    /// main would relayout under the very click that opened the drawer. Only
    /// honored on the selection trigger: with no handle and no pick there
    /// would be nothing left to open it.
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
    /// Whether the handle strip is dropped: what `hide_handle` asks for, but
    /// only where a pick can still bring the drawer out. Asking for it on the
    /// hover trigger would leave nothing to open the drawer with, so that
    /// pairing is ignored rather than honored into a dead panel.
    fn handle_hidden(&self) -> bool {
        self.hide_handle && self.open_on == DrawerTrigger::Selection
    }
}

/// Whether the handle strip is dropped right now: what the config asks for,
/// held only while nothing is picked. Once a pick has primed the drawer the
/// grip comes back and stays for as long as the selection does, so a drawer
/// that folded closed can be pulled out again by hand instead of needing the
/// same album clicked twice.
fn handle_dropped(config: &DrawerConfig, primed: bool) -> bool {
    config.handle_hidden() && !primed
}

pub struct DrawerPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: DrawerConfig,
    /// Main at 0, drawer at 1.
    slots: [Slot; 2],
    /// Whether the hosted children have been told which tab panel this
    /// drawer sits under; see [`composite::introduce_slots`].
    introduced: bool,
    /// Whether the panel itself is active, so the hover toggle can hand
    /// the drawer the right active state without waiting on the dock.
    active: bool,
    /// Whether the drawer is out. Hover-transient - never persisted, a
    /// restore always lands home.
    open: bool,
    /// Held out by a click on the handle, until it is clicked again. As
    /// transient as `open`: a restore lands home and unpinned.
    pinned: bool,
    /// Whether the pointer has been inside the drawer since it opened. A
    /// pick opens the drawer with the pointer still out on the row that was
    /// clicked, so the leave check has to wait for the pointer to arrive
    /// before leaving can mean dismissal. A hover open sets it outright -
    /// the pointer is on the handle already.
    entered: bool,
    /// A pick landed with no window in hand to hand the child its active
    /// state; the next render opens or closes on it. The grid's resync
    /// idiom, for the same reason.
    pending_open: Option<bool>,
    /// Whether something this drawer answers is picked. A dropped handle
    /// comes back once it is, so the drawer stays reachable after it folds
    /// home; clearing the selection takes the grip away again. Transient
    /// like `open`, and only ever set by picks this drawer's scope lets
    /// through.
    primed: bool,
    /// Where the last glide started, in openness; with `open_at` this
    /// gives the animated openness without per-frame state.
    from: f32,
    open_at: Instant,
    /// Where the panel painted, for the reveal's px math and the armed
    /// close handler's leave check. Behind an Arc so the canvas closures
    /// can write it, the scrub strips' idiom.
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// The settings page's reveal slider strip.
    reveal_scrub: ScrubState,
    /// The settings page's dim slider strip.
    dim_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
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

    /// Build with already-restored children, the layout-dump route in.
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

    /// The hosted slots, main then drawer, for the settings window's
    /// layout tree.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// The animated openness, 0 home to 1 out: eased from `from` toward
    /// where `open` points, settled once the glide's window passes.
    fn openness(&self) -> f32 {
        let u = (self.open_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let u = u * u * (3.0 - 2.0 * u);
        let target = if self.open { 1.0 } else { 0.0 };
        self.from + (target - self.from) * u
    }

    /// Slide the drawer out or home. The drawer runs only while it is
    /// out; the main below keeps running the whole time.
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

    /// A pick landed on the app-wide selection. In selection mode it slides
    /// the drawer out, and an emptied selection slides it home; the work
    /// waits for the next render, which has the window the child's active
    /// state needs.
    fn on_selection(
        &mut self,
        selection: &Entity<rox_services::selection::Selection>,
        source: EntityId,
        cx: &mut Context<Self>,
    ) {
        if self.config.open_on != DrawerTrigger::Selection || self.slots[1].is_none() {
            return;
        }
        // Either way a pick inside the open drawer is ignored: clicking a row
        // in there is working, not chaining, and re-answering it would pin
        // the drawer to its own contents.
        match self.config.scope {
            // Scoped to the main, so the rest of the layout can't reach it
            // and two chained drawers don't both answer one pick.
            DrawerScope::Main => {
                if !self.slot_holds(0, source, cx) {
                    return;
                }
            }
            // Driven from anywhere else in the layout, but never from its own
            // contents - a main that publishes picks of its own (a queue, a
            // library) would otherwise slide the drawer over itself.
            DrawerScope::Any => {
                if self.slot_holds(0, source, cx) || self.slot_holds(1, source, cx) {
                    return;
                }
            }
        }
        let picked = !selection.read(cx).tracks().is_empty();
        // A dropped handle comes back with the first pick and stays while
        // the selection holds, so folding home doesn't strand the drawer.
        self.primed = picked;
        self.pending_open = Some(picked);
        cx.notify();
    }

    /// Whether `id` is the panel in slot `ix` or anything nested under it. A
    /// slot can host composites of its own, so this walks the whole subtree
    /// rather than checking the one panel.
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

    /// Hold the drawer out, or let it go. The handle's click, so a drawer
    /// worth working in stops sliding home the moment the pointer strays.
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

    /// The main slot's content: the child's view or the empty add
    /// affordance, filling the panel behind the drawer.
    fn main_content(&self, cx: &mut Context<Self>) -> Div {
        match &self.slots[0] {
            // Routed like a group cell: the drawer opted the dock's body
            // fallback out for everything it covers, so the main slot has
            // to serve the right-click itself.
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

    /// The drawer itself: the handle strip at the panel's edge with the
    /// child sliding out behind it. `u` is the animated openness.
    fn drawer_box(&self, child: Arc<dyn PanelView>, u: f32, cx: &mut Context<Self>) -> Div {
        let edge = self.config.edge;
        let axis = edge.axis();
        let extent = px(self.extent(u));

        // The handle names what waits behind it - the better hint than a
        // bare grip. Resting the pointer on it slides the drawer out.
        let name = child
            .tab_name(cx)
            .unwrap_or_else(|| SharedString::from(panel::display_name(child.panel_name(cx))));
        // The grip takes the accent while pinned: the one mark that says the
        // drawer is being held out rather than hovered out.
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
                // The pointer is on the handle, so the drawer opens already
                // arrived: leaving from here can dismiss it right away.
                this.entered = true;
                this.set_open(true, window, cx);
            }))
            // Clicking the handle pins the open drawer out, and clicking it
            // again lets go. On a closed drawer the click just opens it, the
            // same as resting on it.
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
            // The vertical handles are too thin for text; the grip alone
            // marks them.
            .when(axis == Axis::Vertical, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(name),
                )
            });
        // The child only mounts while some of it shows, so a settled-home
        // drawer costs what the handle does.
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
            // No fill of its own: the hosted panel's surface is the
            // background, so its opacity override (the Appearance page)
            // decides how much of the main shows through the drawer.
            .border_color(palette::border())
            // The border rides the inner edge, where the drawer meets the
            // main.
            .map(|d| match edge {
                DrawerEdge::Top => d.border_b_1(),
                DrawerEdge::Bottom => d.border_t_1(),
                DrawerEdge::Left => d.border_r_1(),
                DrawerEdge::Right => d.border_l_1(),
            })
            .shadow_md()
            // The drawer covers the main; without the occlusion, clicks on
            // it land on both and the covered panel reacts underneath.
            .occlude();
        // The handle stays at the inner edge as the drawer grows, so the
        // stack order flips with the edge.
        // A drawer with its handle dropped shows nothing at all until a pick
        // brings it out, so the strip goes with it.
        let handle = (!handle_dropped(&self.config, self.primed)).then_some(handle);
        let boxed = match edge {
            DrawerEdge::Bottom | DrawerEdge::Right => boxed.children(handle).child(content),
            DrawerEdge::Top | DrawerEdge::Left => boxed.child(content).children(handle),
        };
        // The host chrome's surface shader, painted over the box instead
        // of the panel (see render): the box is what reads as one pane, so
        // the handle strip gets shaded with the content. Last child, so
        // the screen pass samples both already drawn.
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

    /// The drawer's extent along its axis at openness `u`: the handle
    /// alone at home, the reveal fraction of the slot fully out. The slot
    /// size comes off the last paint; before one lands the drawer is home
    /// and the handle needs no measurement.
    fn extent(&self, u: f32) -> f32 {
        let axis = self.config.edge.axis();
        let slot = self
            .bounds
            .lock()
            .unwrap()
            .map(|bounds| f32::from(bounds.size.along(axis)))
            .unwrap_or(0.0);
        let reveal = self.config.reveal.clamp(MIN_REVEAL, 1.0);
        // What the drawer measures at home: its handle, or nothing at all
        // when the handle is dropped.
        let home = if handle_dropped(&self.config, self.primed) {
            0.0
        } else {
            f32::from(HANDLE)
        };
        let reveal_px = (reveal * slot).max(home);
        home + u * (reveal_px - home)
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Let the children reach this host from their own menus; the
        // dock never sees a hosted panel, so nothing else offers it.
        composite::report_hosted(
            self.slots.iter().flatten(),
            self.config.chrome.title.as_deref().unwrap_or("Drawer"),
            cx,
        );

        // A pick from the last frame lands here, where there is a window to
        // hand the child its active state.
        if let Some(open) = self.pending_open.take() {
            if open {
                // The click that opened it landed out on the main panel, so
                // the pointer has not arrived yet and the leave check stays
                // disarmed until it does.
                self.entered = false;
            } else {
                self.pinned = false;
            }
            self.set_open(open, window, cx);
        }

        // Frames only while a glide is actually running; settled costs
        // zero either way.
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
            // The main panel is always the base, at full weight. When a
            // drawer is present the handle keeps a lane on its edge:
            // without it the occluding handle floats over the main's own
            // scrollbar and swallows its clicks. No drawer, no lane - the
            // main fills.
            .child({
                let edge = self.config.edge;
                // The lane is held whether or not the handle is drawn. A
                // dropped handle comes back the moment something is picked,
                // and if the lane came back with it the main would relayout
                // under the very click that opened the drawer.
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

        // The scrim over the main, fading with the slide; at the default 0
        // it never mounts and the main stays bare.
        let dim = self.config.dim.clamp(0.0, 1.0) * u;
        let root = if dim > 0.001 && self.slots[1].is_some() {
            root.child(div().absolute().inset_0().bg(palette::alpha(
                palette::bg_root_opaque(),
                (dim * 255.0).round() as u8,
            )))
        } else {
            root
        };

        // The measuring canvas, and while the drawer is out, the armed
        // leave check: window handlers only live one frame (the scrub
        // strips' idiom), and hover alone can't close it - a press inside
        // the drawer parks the container's hover state, and an occluding
        // child (a scrollbar mid-drag) drops it outright. The bounds
        // check stays true wherever the pointer sits inside the drawer.
        let root = if self.slots[1].is_some() {
            let bounds_store = self.bounds.clone();
            let weak = cx.entity().downgrade();
            // A pinned drawer is held out on purpose, so nothing arms
            // against it.
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
                            // Mid-press moves never close: a drag started
                            // inside the drawer (a scrollbar, a row) may
                            // stray outside for a moment.
                            if event.pressed_button.is_some() {
                                return;
                            }
                            if region.contains(&event.position) {
                                // The pointer arrived. Mark it and repaint,
                                // so the next frame's handler is the one
                                // that can close - this closure captured
                                // the old value and would never fire.
                                if !entered {
                                    if let Some(this) = weak.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.entered = true;
                                            cx.notify();
                                        });
                                    }
                                }
                                return;
                            }
                            // A pick opens the drawer with the pointer still
                            // out on the row that was clicked. Until it has
                            // come inside once, being outside is where it
                            // started, not a dismissal.
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
            // A dropped handle leaves the box nothing to draw at home, but
            // its border and shadow would still line the empty lane. It sits
            // the frame out entirely until the glide starts.
            Some(_) if handle_dropped(&self.config, self.primed) && u <= 0.001 => root,
            Some(drawer) => root.child(self.drawer_box(drawer, u, cx)),
            None => root,
        };

        // A layout that ships as finished furniture drops the builder's
        // buttons; its slots are still swapped from the tree on the
        // Workspace settings page.
        if self.config.chrome.controls_hidden() {
            return root;
        }

        // The corner controls: fill the drawer slot while it is empty,
        // and the shown slot's menu - the drawer's while it is out, the
        // main's otherwise.
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
                        .tooltip("Add Drawer Panel")
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
        let parent = composite::parent_button("Drawer", cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

/// Where the drawer box sits inside the panel's painted bounds: the edge
/// strip `extent` deep, the region the pointer can rest in without the
/// drawer sliding home.
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
                    "Edge",
                    Some("The edge the drawer rests against and slides out from"),
                    choices(
                        EDGE_CHOICES,
                        self.config.edge,
                        |this: &mut Self, edge, cx| {
                            this.config.edge = edge;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    "Open On",
                    Some("Resting on the handle always opens the drawer; selection adds a pick in the main panel"),
                    choices(
                        TRIGGER_CHOICES,
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
                        "Handle",
                        Some("Show the grip at the panel's edge. Hidden, nothing of the drawer shows until a pick, and the grip then stays while the selection holds so a drawer that folded closed can be pulled back out"),
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
                        "Answers",
                        Some("Which picks open the drawer: only its own main panel, or any panel outside it"),
                        choices(
                            SCOPE_CHOICES,
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
                    "Reveal",
                    Some("How much of the panel the open drawer covers"),
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
                    "Dim",
                    Some("How hard the main panel dims behind the open drawer"),
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Drawer")
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
        // The main tracks the panel; the drawer tracks it only while out.
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
        // The pin also lives on the handle; the row is here for the drawer
        // that is already out and under the pointer.
        let menu = menu.item(panel::check_row(
            "Pin Open",
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
        // The chrome shader is worn by the drawer box, handle included,
        // rather than by the whole panel: the host's rect spans the main
        // slot too, and a surface over that would shade the panel the
        // drawer merely covers. `drawer_box` paints it over the box.
        let mut chrome = self.config.chrome.clone();
        chrome.shader = None;
        panel::themed(&chrome, || self.body(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout saved before the trigger knobs existed still loads, and lands
    /// on the old behavior: hover only, so no drawer starts answering picks
    /// on its own after an upgrade.
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

    /// Dropping the handle only holds where a pick can still bring the
    /// drawer out. On the hover trigger the handle is the only way in, so
    /// honoring the ask would leave a panel nothing could open.
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

        // The default drawer keeps its handle either way.
        assert!(!DrawerConfig::default().handle_hidden());
    }

    /// A dropped handle is only dropped until something is picked. Once the
    /// drawer has been primed the grip stays, which is the whole point: the
    /// drawer folds home when the pointer leaves, and without a grip left
    /// behind there would be no way back to it short of picking the same
    /// album again.
    #[test]
    fn a_pick_brings_the_dropped_handle_back() {
        let config = DrawerConfig {
            open_on: DrawerTrigger::Selection,
            hide_handle: true,
            ..DrawerConfig::default()
        };
        // Nothing picked yet, so the panel shows no trace of the drawer.
        assert!(handle_dropped(&config, false));
        // Primed by a pick, the grip is there whether the drawer is out or
        // has folded back home.
        assert!(!handle_dropped(&config, true));

        // A drawer that keeps its handle is unaffected by priming.
        let plain = DrawerConfig {
            open_on: DrawerTrigger::Selection,
            ..DrawerConfig::default()
        };
        assert!(!handle_dropped(&plain, false));
        assert!(!handle_dropped(&plain, true));
    }

    /// The knobs round-trip through a layout dump, which is the only way they
    /// persist.
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

    /// Metro ships the chain, so its drawer is the one bundled layout that
    /// has to keep parsing: the knobs read back as a selection drawer, and
    /// the panel it slides out follows the selection. A hand-edited layout
    /// that drifted from the config types would otherwise only show up as a
    /// silently reset panel at runtime.
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
        // Two per browser tab. One drawer is one edge and one pair of slots,
        // so a wall that wants a list from the left and filters from the
        // right needs the two nested; and a dock tab can't live in a slot, so
        // each tab carries its own pair rather than sharing.
        assert_eq!(drawers.len(), 6);

        let mut titles = Vec::new();
        for outer in drawers
            .iter()
            .filter(|d| !d["info"]["panel"]["title"].is_null())
        {
            let config: DrawerConfig =
                serde_json::from_value(outer["info"]["panel"].clone()).unwrap();
            // The outer one names the tab and holds the queue, on hover
            // only: nothing about a pick should pull it out.
            assert_eq!(config.open_on, DrawerTrigger::Hover);
            assert_eq!(config.edge, DrawerEdge::Right);
            titles.push(config.chrome.title.clone().expect("the tab is named"));
            assert_eq!(outer["children"][1]["panel_name"], "queue");
            // Metro ships finished, so no builder's buttons sit in its
            // corners.
            assert!(config.chrome.hide_controls);

            let inner = &outer["children"][0];
            let inner_config: DrawerConfig =
                serde_json::from_value(inner["info"]["panel"].clone()).unwrap();
            assert_eq!(inner_config.open_on, DrawerTrigger::Selection);
            // The wall it hosts is exactly what drives it, so the two tabs
            // stay independent and nothing else in the layout reaches them.
            assert_eq!(inner_config.scope, DrawerScope::Main);
            assert_eq!(inner_config.edge, DrawerEdge::Left);
            assert!(inner_config.chrome.hide_controls);
            // Nothing of it shows until a pick, so the wall gets the whole
            // panel while you browse.
            assert!(inner_config.handle_hidden());
            // The queue keeps its grip: hover is the only way to that one.
            assert!(!config.handle_hidden());

            // Slot 0 is the wall that publishes picks; slot 1 is what slides
            // out, and it only shows the pick if it follows the selection.
            assert!(inner["children"][0]["panel_name"]
                .as_str()
                .is_some_and(|name| name.ends_with("grid")));
            let slid_out = &inner["children"][1];
            assert_eq!(slid_out["panel_name"], "library");
            assert_eq!(slid_out["info"]["panel"]["query_source"], "selection");
        }
        assert_eq!(titles, ["Albums", "Artists", "Genres"]);
    }

    /// The query source is what a chained drawer's hosted panel rides, so its
    /// wire name has to survive too.
    #[test]
    fn selection_query_source_round_trips() {
        use rox_panel_api::query::shared_query::QuerySource;
        let value = serde_json::to_value(QuerySource::Selection).unwrap();
        assert_eq!(value, "selection");
        let back: QuerySource = serde_json::from_value(value).unwrap();
        assert!(back == QuerySource::Selection);
    }
}
