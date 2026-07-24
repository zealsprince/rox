//! The drawer panel: a main panel with a second tucked against one edge,
//! only a labeled handle showing until the pointer rests on it, then the
//! drawer slides out over the main and slides home when the pointer
//! leaves. For the surfaces worth a glance but not a slot of their own -
//! the queue rising over a transport group, filters over a library. The
//! edge and how much of the panel the open drawer covers are per-panel
//! settings. Hosted through [`crate::composite`]; the drawer costs
//! nothing once it settles home.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, size, Along, AnyElement, App, Axis, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, MouseMoveEvent, Pixels, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::composite::{self, Slot};
use crate::design::{palette, tokens};
use crate::panel::{self, choices, setting_row, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::workspace::Workspace;

/// The handle strip's thickness: enough for the grip and label to read,
/// thin enough to stay a hint over the main.
const HANDLE: Pixels = px(18.);

/// The floor of the reveal fraction, so the slider can't shrink the open
/// drawer into its own handle.
const MIN_REVEAL: f32 = 0.15;

/// The edge the drawer rests against, and the direction it slides from.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
}

impl Default for DrawerConfig {
    fn default() -> Self {
        DrawerConfig {
            chrome: PanelChrome::default(),
            edge: DrawerEdge::default(),
            reveal: 1.0,
            dim: 0.0,
        }
    }
}

pub struct DrawerPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: DrawerConfig,
    /// Main at 0, drawer at 1.
    slots: [Slot; 2],
    /// Whether the panel itself is active, so the hover toggle can hand
    /// the drawer the right active state without waiting on the dock.
    active: bool,
    /// Whether the drawer is out. Hover-transient - never persisted, a
    /// restore always lands home.
    open: bool,
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
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
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
        DrawerPanel {
            state,
            workspace,
            config,
            slots: fixed,
            active: false,
            open: false,
            from: 0.0,
            open_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            bounds: Arc::default(),
            reveal_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
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

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        cx.notify();
    }

    fn set_reveal(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.reveal = MIN_REVEAL + fraction * (1.0 - MIN_REVEAL);
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
            Some(child) => div().size_full().child(child.view()),
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
        let grip = div()
            .rounded_full()
            .bg(palette::text_faint())
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
                this.set_open(true, window, cx);
            }))
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
            .when(u > 0.001, |d| d.child(child.view()));

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
        match edge {
            DrawerEdge::Bottom | DrawerEdge::Right => boxed.child(handle).child(content),
            DrawerEdge::Top | DrawerEdge::Left => boxed.child(content).child(handle),
        }
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
        let reveal_px = (reveal * slot).max(f32::from(HANDLE));
        f32::from(HANDLE) + u * (reveal_px - f32::from(HANDLE))
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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
            let armed = self.open || u > 0.001;
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
            Some(drawer) => root.child(self.drawer_box(drawer, u, cx)),
            None => root,
        };

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
                    "Reveal",
                    Some("How much of the panel the open drawer covers"),
                    panel::value_slider(
                        &self.reveal_scrub,
                        (reveal - MIN_REVEAL) / (1.0 - MIN_REVEAL),
                        format!("{:.0} %", reveal * 100.0),
                        Self::set_reveal,
                        cx,
                    ),
                ))
                .child(setting_row(
                    "Dim",
                    Some("How hard the main panel dims behind the open drawer"),
                    panel::value_slider(
                        &self.dim_scrub,
                        self.config.dim.clamp(0.0, 1.0),
                        format!("{:.0} %", self.config.dim.clamp(0.0, 1.0) * 100.0),
                        Self::set_dim,
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
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for DrawerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
