//! The canvas panel (ADR 33): a composite that places items in 3D space. Live
//! UI goes in as grid items, each a dock tree of its own, so a canvas holds
//! real tab groups that drag, zoom and pop out like any other. Images,
//! textures and plain shaders take real perspective; grids, and shaders
//! that can only run as a screen pass, stay flat to the screen and take
//! their place and depth order from the projection.
//!
//! A canvas is only ever a panel. The window's root stays the dock, and a
//! full-window canvas is a dock holding one canvas panel. Its saved state is
//! its config, nested dock dumps included, and `PanelState.children` stays
//! empty.
//!
//! In design mode the panel is also the canvas's editor: it picks, moves,
//! resizes and turns items with handles drawn on the projection, keeps the
//! undo history, and hosts the inspector ([`crate::canvas::overlay`]). The
//! arithmetic behind every gesture is in [`crate::canvas::edit`]. Off design
//! mode none of that shows and a canvas is plain content, where a click on an
//! item that names a command runs it through [`keymap::dispatch`].

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, AnyWindowHandle, App, Axis, Bounds, Context, CursorStyle, DismissEvent, Div,
    Entity, EntityId, EventEmitter, ExternalPaths, FocusHandle, Focusable, HitboxBehavior,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, PathBuilder,
    PathPromptOptions, Pixels, Point, Rgba, ScrollDelta, ScrollWheelEvent, SharedString,
    Subscription, WeakEntity, Window, actions, anchored, canvas, deferred, div, point, prelude::*,
    px,
};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_canvas::{
    Anchor, Camera, CanvasItem, CanvasState, Content, Frame, ImageFit, Length, Live, Place,
    Viewport, child_frame, inside, paint_order, project, unit_point,
};
use rox_core::settings::AssetKind;
use rox_core::settings::assets;
use rox_dock::{
    DockArea, DockAreaState, Panel, PanelEvent, PanelInfo, PanelState, PanelView, StackPanel,
    TabPanel,
};
use rox_viz::signal::{Route, SignalHub};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canvas::edit::{self, Edges, History};
use crate::canvas::grid::{self, Grid};
use crate::canvas::image::{self, Images};
use crate::canvas::overlay::Inspector;
use crate::canvas::routes::{self, Snapshot};
use crate::canvas::shader::{self as shader_item, Shaders};
use crate::canvas::states::{self, Cases};
use crate::canvas::target::{self, Pointer, Status as TargetStatus, Targets};
use crate::canvas::text::{self, Values};
use crate::canvas::texture::{self, TextureView};
use crate::canvas::{self, Body, Placing};
use crate::enable_card::EnableCard;
use crate::keymap;
use crate::objects;
use crate::panels::controls;
use crate::workspace::{LAYOUT_VERSION, Workspace};
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette;
use rox_panel_api::buttons;
use rox_panel_api::marks::MarkCache;
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings, Tone, ValueEdit};
use rox_panel_api::panel_settings;
use rox_panel_api::signal_ui::{RouteHost, SignalHost, SignalUi};
use rox_panel_api::toast::Toast;
use rox_panel_api::windows::WorkspaceWindows;
use rox_services::catalog::LibraryEvent;
use rox_services::player::EqChanged;

/// The registry name.
pub const PANEL_NAME: &str = "canvas";

pub use rox_panel_api::actions::{Redo, Undo};

actions!(
    canvas,
    [
        DeleteItem,
        GroupItem,
        UngroupItem,
        NudgeLeft,
        NudgeRight,
        NudgeUp,
        NudgeDown,
        NudgeLeftFar,
        NudgeRightFar,
        NudgeUpFar,
        NudgeDownFar,
        StepOut,
    ]
);

/// The key context a canvas sets while its own focus holds in design mode.
/// Panels inside its grids hold their own focus, so the canvas's keys never
/// reach past them.
pub const KEY_CONTEXT: &str = "Canvas";

/// Added beside [`KEY_CONTEXT`] while an item is selected.
pub const SELECTED_CONTEXT: &str = "CanvasSelected";

/// The title strip a flat item wears in design mode, its drag handle.
const STRIP_H: f32 = 18.;

/// A resize handle's side.
const HANDLE: f32 = 8.;

/// How near the pointer has to come to a turn handle to take it.
const GRIP_REACH: f32 = 7.;

/// A second press this close to the first steps into a group.
const SAME_SPOT: f32 = 4.;

/// The smallest a non-grid item resizes to, in pixels.
const MIN_BOX: f32 = 16.;

/// How near the canvas's side the pointer comes before the inspector's tab
/// shows, and how tall the band it shows in, both in pixels.
const EDGE_REACH: f32 = 28.;
const EDGE_BAND: f32 = 120.;

thread_local! {
    /// The canvas whose selection and inspector show. Only one does at a
    /// time, so a canvas nested in another doesn't stack a second inspector
    /// over the first.
    static ACTIVE: Cell<Option<EntityId>> = const { Cell::new(None) };
}

/// One item's place on screen this frame, for picking and for the chrome.
#[derive(Clone)]
pub(crate) struct Shape {
    pub id: String,
    /// The groups it sits in, outermost first.
    pub groups: Vec<String>,
    /// Panel-relative, clockwise from the top-left.
    pub corners: [[f32; 2]; 4],
    pub label: SharedString,
    pub kind: ShapeKind,
    pub locked: bool,
    /// Drawn as a ghost in design mode because a case hides it or a group
    /// around it. It can be picked, and a click passes through it.
    pub hidden: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKind {
    /// Live panels: picked by the strip, and the body's input is theirs.
    Grid,
    /// Square to the screen but not a panel.
    Flat,
    Perspective,
    /// The box around its children. Never hit itself.
    Group,
}

impl Shape {
    fn flat(&self) -> bool {
        matches!(self.kind, ShapeKind::Grid | ShapeKind::Flat)
    }

    /// Whether the turn handles show: anything that takes perspective, a
    /// group included, and isn't locked.
    fn turns(&self) -> bool {
        matches!(self.kind, ShapeKind::Perspective | ShapeKind::Group) && !self.locked
    }

    /// Left, top, right, bottom.
    fn edges(&self) -> [f32; 4] {
        let (origin, extent) = canvas::bbox(&self.corners);
        [
            origin[0],
            origin[1],
            origin[0] + extent[0],
            origin[1] + extent[1],
        ]
    }

    fn center(&self) -> [f32; 2] {
        let c = &self.corners;
        [
            (c[0][0] + c[1][0] + c[2][0] + c[3][0]) / 4.,
            (c[0][1] + c[1][1] + c[2][1] + c[3][1]) / 4.,
        ]
    }

    fn chain(&self) -> Vec<String> {
        let mut chain = self.groups.clone();
        chain.push(self.id.clone());
        chain
    }
}

/// A drag in progress. It edits the canvas live and lands in the history as
/// one step on release.
struct Gesture {
    id: String,
    before: CanvasState,
    /// The item as it stood when the press landed.
    item: CanvasItem,
    parent: Frame,
    camera: Camera,
    viewport: Viewport,
    from: [f32; 2],
    start_box: [f32; 4],
    /// Whether the item drew flat at the press, which a shader item decides
    /// by what it runs, not by its content kind alone.
    flat: bool,
    kind: GestureKind,
}

enum GestureKind {
    /// Snap lines, x then y.
    Move([Vec<f32>; 2]),
    Resize {
        edges: Edges,
        floor: [f32; 2],
    },
    Turn {
        axis: usize,
        center: [f32; 2],
        radius: f32,
    },
}

/// What the add menu can put on the stage.
#[derive(Clone)]
pub(crate) enum NewItem {
    Grid,
    Group,
    Image(String),
    Shader,
    Text,
    /// A render target, by the owner it saves.
    Target(String),
}

/// `shader` and `owner` are the dump walkers' keys everywhere, and the
/// chrome already holds both. Nothing else in here may use them.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(flatten)]
    pub canvas: CanvasState,
    /// Targets are `<item id>.<property>`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
}

/// Where Add Panel goes in `ws`'s window when an emptied grid is the target:
/// the grid item selected in the canvas whose inspector shows.
pub(crate) fn selected_empty_grid(ws: &Workspace, cx: &App) -> Option<Entity<TabPanel>> {
    let active = ACTIVE.get()?;
    objects::canvases(ws, cx)
        .into_iter()
        .find(|canvas| canvas.entity_id() == active)?
        .read(cx)
        .selected_empty_group(cx)
}

/// Clears the lock on every panel in a dock dump. It walks the dump's own
/// tree and stops at each panel's config, so a canvas in the layout keeps its
/// items' locks and its grids' panels as they were.
fn unlock(node: &mut Value) {
    if node.get("panel_name").is_some() {
        if let Some(config) = node
            .pointer_mut("/info/panel")
            .and_then(Value::as_object_mut)
        {
            config.remove("locked");
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            children.iter_mut().for_each(unlock);
        }
        return;
    }

    match node {
        Value::Object(map) => map.values_mut().for_each(unlock),
        Value::Array(list) => list.iter_mut().for_each(unlock),
        _ => {}
    }
}

/// The dock state a canvas saves as: its config, with no children.
pub(crate) fn state_of(config: &CanvasConfig) -> PanelState {
    PanelState {
        panel_name: PANEL_NAME.to_string(),
        children: Vec::new(),
        info: PanelInfo::panel(serde_json::to_value(config).unwrap_or(Value::Null)),
    }
}

/// The id [`wrap_layout`] gives the grid it puts the layout in.
pub(crate) const WRAPPED_GRID: &str = "grid";

/// A layout as a canvas: one grid holding `dump` across the whole stage, the
/// one panel in the one tab group of a fresh root. The window looks the same
/// once it's applied. Every panel's lock comes off, since a locked panel
/// can't be dragged in or out of its group, and the grid is there to be
/// rearranged.
pub(crate) fn wrap_layout(mut dump: Value, version: usize) -> DockAreaState {
    unlock(&mut dump);

    let grid = CanvasItem {
        id: WRAPPED_GRID.to_string(),
        place: Place {
            anchor: Anchor::Center,
            size: [Length::Stage(1.), Length::Stage(1.)],
            ..Place::default()
        },
        content: Content::Grid { dock: dump },
        ..CanvasItem::default()
    };
    let config = CanvasConfig {
        canvas: CanvasState {
            items: vec![grid],
            ..CanvasState::default()
        },
        ..CanvasConfig::default()
    };

    let tabs = PanelState {
        panel_name: "TabPanel".to_string(),
        children: vec![state_of(&config)],
        info: PanelInfo::tabs(0),
    };
    let root = PanelState {
        panel_name: "StackPanel".to_string(),
        children: vec![tabs],
        info: PanelInfo::stack(Vec::new(), Axis::Vertical),
    };

    DockAreaState {
        version: Some(version),
        center: root,
        ..DockAreaState::default()
    }
}

pub struct CanvasPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    pub(crate) config: CanvasConfig,
    /// At the same indices as `config.canvas.items`.
    bodies: Vec<Body<Grid>>,
    /// Grids built ahead of the edit that names them.
    spare: HashMap<String, Grid>,
    /// Canvases and groups this canvas sits inside.
    level: usize,
    images: Images,
    pub(crate) shaders: Shaders,
    /// The texture items' views, by item id.
    textures: HashMap<String, Entity<TextureView>>,
    /// The render targets' sessions and views, by item id.
    targets: Targets,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Renders still owed after a resize.
    settle: Rc<Cell<u8>>,
    /// The pointer while it's over the panel, for drop routing.
    pointer: Rc<Cell<Option<Point<Pixels>>>>,
    /// Whether the pointer is in the band where the inspector's tab shows.
    near_edge: Rc<Cell<bool>>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    pub(crate) selected: Option<String>,
    history: History<Snapshot>,
    /// The routes as the history last saw them. The shared binding rows
    /// write `config.routes` directly, so a difference is an edit to record.
    seen_routes: Vec<Route>,
    motion: routes::Motion,
    signal_ui: SignalUi,
    gesture: Option<Gesture>,
    /// Where the last press picked, and the chain of ids under it, so a
    /// second press on the same spot steps one group in.
    last_press: Option<([f32; 2], Vec<String>)>,
    /// Every visible item's place from the last render, in paint order.
    shapes: Vec<Shape>,
    /// Design mode as the last render saw it.
    design: bool,
    /// For edits from controls that aren't handed the window.
    window: Option<AnyWindowHandle>,
    pub(crate) inspector: Inspector,
    menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// The playing track's fields for text items, dropped when the track or
    /// its tags change.
    text_values: Option<Rc<Values>>,
    /// The item whose command a left press landed on, so the release runs it
    /// only over the same item.
    pressed: Option<String>,
    /// Whether the pointer is over an item that would run a command.
    pointing: Rc<Cell<bool>>,
    /// The render target the pointer is over, off design mode.
    hovered: Option<String>,
    /// The export card or the place confirm, over the whole window.
    pub(crate) object_dialog: Option<objects::Dialog>,
    /// The playing track's marks, resolved only while an item follows the
    /// favourite.
    marks: MarkCache,
    /// The followed states' cases as the last render drew them.
    cases: Cases,
    _player: Subscription,
    _player_view: Subscription,
    _cover: Subscription,
    _library: Subscription,
    _states: [Subscription; 5],
}

impl CanvasPanel {
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: CanvasConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The clock ticks every pump tick while audio moves. Only a canvas
        // with routes has anything to redraw for it, and only while playback
        // runs or just changed: an ease asks for its own frames.
        let player = rox_services::player::observe_clock(
            &state.player,
            cx,
            |this: &mut Self, player, cx| {
                let player = player.read(cx);
                let running = routes::playback_running(player.is_playing(), player.queue_ended());
                if routes::running(&this.config.routes) && this.motion.wants(running) {
                    cx.notify();
                }
            },
        );

        // `observe_view`'s compare, inlined so the change also drops the held
        // text values. A canvas with no text doesn't redraw for it.
        let mut last = state.player.read(cx).view();
        let player_view = cx.observe(&state.player, move |this: &mut Self, player, cx| {
            let now = player.read(cx).view();
            if now == last {
                return;
            }

            last = now;
            this.text_values = None;
            if text::holds_text(&this.config.canvas) {
                cx.notify();
            }
        });
        // A render target granted the cover hears of a new track here, with
        // no frame drawn for it. One resting for the music draws nothing new
        // while it rests, so playback going on is the redraw that wakes it.
        let mut was_running = None;
        let cover = cx.observe(&state.player, move |this: &mut Self, player, cx| {
            if !target::holds_targets(&this.config.canvas) {
                return;
            }

            if let Some(ask) = this.playing_cover(cx) {
                this.targets.follow_cover(ask);
            }

            let player = player.read(cx);
            let running = routes::playback_running(player.is_playing(), player.queue_ended());
            if was_running.replace(running) != Some(running)
                && target::holds_resting(&this.config.canvas)
            {
                cx.notify();
            }
        });

        // A tag edit to the playing track, as track info follows it, and a
        // mark on it for an item following the favourite.
        let library = cx.subscribe(
            &state.library,
            |this: &mut Self, library, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.text_values = None;
                    if text::holds_text(&this.config.canvas) {
                        cx.notify();
                    }
                }

                if states::needs_marks(&this.config.canvas)
                    && this.marks.apply(event, library.read(cx))
                {
                    cx.notify();
                }
            },
        );

        // What Custom Controls watches, for the states an item follows. The
        // player notifies on every command, so each of these redraws only
        // when a followed state's case moved.
        let followed = [
            cx.observe_in(&state.player, window, |this: &mut Self, _, window, cx| {
                this.recheck_states(window, cx)
            }),
            cx.observe_in(
                &state.scrobbler,
                window,
                |this: &mut Self, _, window, cx| this.recheck_states(window, cx),
            ),
            cx.observe_in(&state.discord, window, |this: &mut Self, _, window, cx| {
                this.recheck_states(window, cx)
            }),
            cx.observe_global_in::<EqChanged>(window, |this: &mut Self, window, cx| {
                this.recheck_states(window, cx)
            }),
            // The watch switch notifies without an event.
            cx.observe_in(&state.library, window, |this: &mut Self, _, window, cx| {
                this.recheck_states(window, cx)
            }),
        ];

        let seen_routes = config.routes.clone();
        let mut this = CanvasPanel {
            state,
            workspace,
            config,
            bodies: Vec::new(),
            spare: HashMap::new(),
            level: canvas::level_here(),
            images: Images::default(),
            shaders: Shaders::default(),
            textures: HashMap::new(),
            targets: Targets::default(),
            bounds: Rc::default(),
            settle: Rc::default(),
            pointer: Rc::default(),
            near_edge: Rc::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            selected: None,
            history: History::default(),
            seen_routes,
            motion: routes::Motion::default(),
            signal_ui: SignalUi::default(),
            gesture: None,
            last_press: None,
            shapes: Vec::new(),
            design: settings::design_mode(),
            window: None,
            inspector: Inspector::default(),
            menu: None,
            text_values: None,
            pressed: None,
            pointing: Rc::default(),
            hovered: None,
            object_dialog: None,
            marks: MarkCache::default(),
            cases: Cases::new(),
            _player: player,
            _player_view: player_view,
            _cover: cover,
            _library: library,
            _states: followed,
        };
        this.build(HashMap::new(), window, cx);
        this
    }

    /// Every grid item's dock, groups included, in item order.
    pub fn grids(&self) -> Vec<Entity<DockArea>> {
        let mut out = Vec::new();
        canvas::each_grid(&self.config.canvas.items, &self.bodies, &mut |_, grid| {
            out.push(grid.area.clone())
        });
        out
    }

    /// What the layout tree calls each of [`grids`](Self::grids), in the
    /// same order: the item's name, or its id when it has none.
    pub(crate) fn grid_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        canvas::each_grid(&self.config.canvas.items, &self.bodies, &mut |item, _| {
            out.push(if item.name.is_empty() {
                item.id.clone()
            } else {
                item.name.clone()
            })
        });
        out
    }

    /// The one path every edit takes: applies it, records undo, asks the
    /// workspace to save. Live grids are kept across it by item id.
    pub(crate) fn change(
        &mut self,
        edit: impl FnOnce(&mut CanvasState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_keyed(None, edit, window, cx);
    }

    /// [`change`](Self::change), merged into the last undo step when that
    /// one carried the same key a moment ago: a scrub or a run of nudges
    /// undoes in one go.
    pub(crate) fn change_keyed(
        &mut self,
        key: Option<&str>,
        edit: impl FnOnce(&mut CanvasState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_with_routes(key, |canvas, _| edit(canvas), window, cx);
    }

    /// [`change_keyed`](Self::change_keyed) for an edit that adds routes
    /// with its items, so one undo step takes both back out.
    pub(crate) fn change_routed(
        &mut self,
        edit: impl FnOnce(&mut CanvasState, &mut Vec<Route>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_with_routes(None, edit, window, cx);
    }

    fn change_with_routes(
        &mut self,
        key: Option<&str>,
        edit: impl FnOnce(&mut CanvasState, &mut Vec<Route>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The edit sees every grid as it stands now, not as it was loaded.
        self.config = self.saved(cx);
        let before = self.snapshot();

        let mut routes = std::mem::take(&mut self.config.routes);
        self.rebuild_with(|canvas| edit(canvas, &mut routes), window, cx);
        self.config.routes = routes;
        routes::prune(&mut self.config.routes, &self.config.canvas);
        self.seen_routes = self.config.routes.clone();

        self.history
            .record(before, self.snapshot(), key, Instant::now());
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            canvas: self.config.canvas.clone(),
            routes: self.config.routes.clone(),
        }
    }

    /// Puts back a step from the history, routes and all.
    fn restore_snapshot(&mut self, to: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.config.routes = to.routes;
        self.seen_routes = self.config.routes.clone();
        self.rebuild_with(move |canvas| *canvas = to.canvas, window, cx);
    }

    /// A route bound, tuned or dropped from the inspector since the last
    /// render becomes an undo step and a save. A new position or rotation
    /// route gets a sway on its axis in the same step.
    fn absorb_route_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if routes::same(&self.config.routes, &self.seen_routes) {
            return;
        }

        self.config = self.saved(cx);
        let before = Snapshot {
            canvas: self.config.canvas.clone(),
            routes: std::mem::take(&mut self.seen_routes),
        };

        routes::sway_new(&before.routes, &self.config.routes, &mut self.config.canvas);
        self.seen_routes = self.config.routes.clone();
        self.inspector.reweigh();
        self.history
            .record(before, self.snapshot(), Some("routes"), Instant::now());

        cx.defer_in(window, |this, window, cx| this.save_soon(window, cx));
    }

    /// For controls whose callbacks aren't handed the window: the edit runs
    /// on the next turn, through the same path.
    pub(crate) fn change_later(
        &mut self,
        key: &'static str,
        edit: impl FnOnce(&mut CanvasState) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = self.window else {
            return;
        };

        let me = cx.entity().downgrade();
        let app: &mut App = cx;
        app.defer(move |cx| {
            handle
                .update(cx, |_, window, cx| {
                    me.update(cx, |this, cx| {
                        this.change_keyed(Some(key), edit, window, cx);
                    })
                    .ok();
                })
                .ok();
        });
    }

    /// Hands the keys back to the canvas after a typed inspector field
    /// commits. The field goes away and leaves focus nowhere, so undo went
    /// unheard until a click.
    pub(crate) fn refocus_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.on_next_frame(window, |this, window, cx| {
            window.focus(&this.focus);
            cx.notify();
        });
    }

    /// Applies an edit and rebuilds, without touching the history.
    fn rebuild_with(
        &mut self,
        edit: impl FnOnce(&mut CanvasState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut keep = std::mem::take(&mut self.spare);
        canvas::take_grids(
            &self.config.canvas.items,
            std::mem::take(&mut self.bodies),
            &mut keep,
        );

        edit(&mut self.config.canvas);
        self.build(keep, window, cx);
        self.inspector.reweigh();

        if let Some(id) = &self.selected
            && self.config.canvas.find(id).is_none()
        {
            self.selected = None;
        }

        self.save_soon(window, cx);
        cx.notify();
    }

    pub(crate) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gesture = None;
        self.config = self.saved(cx);

        if let Some(state) = self.history.undo() {
            self.restore_snapshot(state, window, cx);
        }
    }

    pub(crate) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gesture = None;
        self.config = self.saved(cx);

        if let Some(state) = self.history.redo() {
            self.restore_snapshot(state, window, cx);
        }
    }

    pub(crate) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn images_mut(&mut self) -> &mut Images {
        &mut self.images
    }

    /// A target item's status line, as its session last left it.
    pub(crate) fn target_status(&self, item: &CanvasItem) -> Option<TargetStatus> {
        self.targets.status(item)
    }

    pub(crate) fn start_target_again(&mut self, id: &str) {
        self.targets.start_again(id);
    }

    /// Ask the workspace for its debounced layout save. Only from the
    /// workspace's own window: a popped-out canvas isn't in its layout, and
    /// the save would record the pop-out's frame as the window's.
    pub(crate) fn save_soon(&self, window: &mut Window, cx: &mut App) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let home = cx.try_global::<WorkspaceWindows>().and_then(|windows| {
            windows
                .open
                .iter()
                .find(|open| open.workspace.entity_id() == workspace.entity_id())
                .map(|open| open.handle)
        });
        if home != Some(window.window_handle()) {
            return;
        }

        workspace.update(cx, |workspace, cx| workspace.save_layout_soon(window, cx));
    }

    fn build(
        &mut self,
        mut keep: HashMap<String, Grid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        self.bodies = canvas::build_bodies(
            &self.config.canvas.items,
            self.level,
            &mut |item, dock, level| match keep.remove(&item.id) {
                Some(grid) => Some(grid),
                None => grid::restore(&item.id, dock, level, &state, window, cx),
            },
        );

        // Views for items that are gone go with them. A texture is freed
        // here, where the window is in hand.
        let canvas = &self.config.canvas;
        self.shaders.keep(canvas);
        self.targets.keep(canvas, window, cx);
        self.textures.retain(|id, view| {
            let kept = canvas
                .find(id)
                .is_some_and(|item| texture::is_test(&item.content));
            if !kept {
                view.update(cx, |view, cx| view.release(window, cx));
            }
            kept
        });
    }

    /// The cover a render target should hold now. None between two tracks,
    /// where the session is still alive and the old cover holds, as the
    /// backdrop's does.
    fn playing_cover(&self, cx: &App) -> Option<target::CoverAsk> {
        let player = self.state.player.read(cx);
        let now = player.now_playing();
        if now.is_none() && player.is_active() && !player.queue_ended() {
            return None;
        }

        let track = now.map(|now| now.key.path);
        let store = self.state.thumbs.read(cx).store_conn();
        Some(target::CoverAsk::new(track, store))
    }

    /// Whether the item sits square to the screen this frame. A shader item
    /// answers from its last paint, so editing its source can flip it.
    pub(crate) fn is_flat(&self, item: &CanvasItem, cx: &App) -> bool {
        shader_item::sits_flat(&item.content, self.shaders.painted_flat(&item.id, cx))
    }

    /// The config with every live grid's dump folded in.
    pub(crate) fn saved(&self, cx: &App) -> CanvasConfig {
        let mut config = self.config.clone();
        canvas::fold_dumps(&mut config.canvas.items, &self.bodies, &|grid: &Grid| {
            grid::dump(grid, cx)
        });
        config
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let bounds = self.bounds.get();
        let camera = self.config.canvas.camera;
        let parallax = camera.parallax > 0.;
        let design = self.design;

        let at = Placing {
            camera,
            viewport: Viewport {
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            },
            pointer: parallax.then(|| {
                let at = window.mouse_position() - bounds.origin;
                [f32::from(at.x), f32::from(at.y)]
            }),
            design,
        };
        let stage = Frame::stage(&self.config.canvas.stage, at.viewport);

        // Reading the hub advances it. A canvas with no routes doesn't, and
        // asks for no frames, and neither does one settled at rest or held.
        let lives = if routes::running(&self.config.routes) {
            let player = self.state.player.read(cx);
            let running = routes::playback_running(player.is_playing(), player.queue_ended());
            let pose = routes::pose(
                &mut self.motion,
                &self.config.canvas,
                &self.config.routes,
                running,
                &self.state.signals,
            );

            // Every frame a routed grid moves, its dock lays out again at the
            // new place. That's allowed, and it's the cost to count when
            // weighing a canvas.
            if pose.more {
                window.request_animation_frame();
            }
            pose.lives
        } else {
            HashMap::new()
        };

        // Every render target drawn this frame is sent the same reading.
        // It rides on frames the canvas draws anyway and asks for none.
        if target::holds_targets(&self.config.canvas) {
            if let Some(ask) = self.playing_cover(cx) {
                self.targets.follow_cover(ask);
            }

            // The window's colours once any ease lands, so an ease sends one
            // line and not one a frame.
            let art = palette::art_theming()
                .then(|| palette::seed(self.state.player.entity_id()))
                .flatten();

            // A tab group paints the theme's background under the panel it
            // holds, solid only while surfaces are opaque. A popped-out
            // canvas, or one inside a composite, sits on another surface.
            let docked = self
                .tab_panel
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|tabs| {
                    // By id alone: reading a panel's visibility would read
                    // this canvas, mid render.
                    let me = cx.entity_id();
                    tabs.read(cx)
                        .panels()
                        .iter()
                        .any(|held| held.view().entity_id() == me)
                });
            let stage = (docked && palette::app_surface_opacity() >= 1.0)
                .then(|| palette::theme_target().bg_root);
            let colours = target::palette_line(&palette::resolved(), art.as_ref(), stage);

            let player = self.state.player.read(cx);
            let running = routes::playback_running(player.is_playing(), player.queue_ended());
            let time = || player.now_playing().map_or(0., |now| now.position_secs);
            self.targets
                .read(&self.state.signals, running, time, colours);
        }

        let values = if text::holds_text(&self.config.canvas) {
            self.text_values
                .get_or_insert_with(|| Rc::new(text::now_playing(&self.state, cx)))
                .clone()
        } else {
            Rc::default()
        };

        // A canvas where nothing follows a state reads none of it.
        self.cases = if states::follows(&self.config.canvas) {
            self.read_cases(window, cx)
        } else {
            self.marks.clear();
            Cases::new()
        };

        // The case the inspector shows stands in for the live one, on the
        // selected item only.
        let active = design && self.is_active(cx);
        let preview = self
            .inspector
            .preview()
            .filter(|(id, _)| active && self.selected.as_deref() == Some(*id));

        // Before the first bounds land there's nothing to place against, and
        // a grid laid out at no size would lay its panels out at none.
        let mut draw = Draw {
            at: &at,
            lives: &lives,
            values: &values,
            cases: &self.cases,
            preview,
            ghost: false,
            images: &mut self.images,
            shaders: &mut self.shaders,
            textures: &mut self.textures,
            targets: &mut self.targets,
            layers: Vec::new(),
            shapes: Vec::new(),
        };
        if at.viewport.width > 0. && at.viewport.height > 0. {
            place(
                &self.config.canvas.items,
                &self.bodies,
                &stage,
                &[],
                &mut draw,
                cx,
            );
        }
        let Draw { layers, shapes, .. } = draw;
        self.shapes = shapes;
        self.images.wake_when_due(cx);

        // Bounds land a frame late, so a resize re-renders once they move.
        // gpui drops a notify made while the window draws, so it's deferred.
        // A grid's splits fit themselves to the new size during that next
        // paint and lose their notify the same way, and they render cached,
        // so they're notified straight on the pass after.
        let me = cx.entity_id();
        let captured = self.bounds.clone();
        let settle = self.settle.clone();
        let grids = self.grids();
        let pointer = self.pointer.clone();
        let near_edge = self.near_edge.clone();
        let left = self.inspector.on_left();
        let watch = canvas(
            move |bounds, _, cx| {
                if captured.get() != bounds {
                    captured.set(bounds);
                    settle.set(2);
                }
                if settle.get() > 0 {
                    settle.set(settle.get() - 1);
                    cx.defer(move |cx| {
                        cx.notify(me);
                        for area in &grids {
                            grid::refresh_splits(area, cx);
                        }
                    });
                }
            },
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }

                    let inside = bounds.contains(&event.position);
                    pointer.set(inside.then_some(event.position));
                    if parallax && inside {
                        cx.notify(me);
                    }

                    // Only a crossing re-renders, so a pointer moving about
                    // the stage costs nothing.
                    let near = design && inside && edge_band(bounds, event.position, left);
                    if near != near_edge.get() {
                        near_edge.set(near);
                        cx.notify(me);
                    }
                });
            },
        )
        .absolute()
        .size_full();

        if active {
            shader_item::absorb_hot(self, cx);
        }
        let chrome = design.then(|| self.chrome_layer(active, cx));
        let canvas = &self.config.canvas;
        let clicks = (!design && (has_commands(canvas) || target::holds_targets(canvas)))
            .then(|| self.click_layer(cx));
        let inspector = active.then(|| crate::canvas::overlay::render(self, window, cx));
        let tab = (design && !active && self.near_edge.get()).then(|| self.edge_tab(cx));
        let menu = self.menu_overlay(window);
        let dialog = crate::canvas::overlay::object_dialog(self, window, cx);

        let scoped = design && self.focus.is_focused(window);
        let mut context = gpui::KeyContext::default();
        context.add(KEY_CONTEXT);
        if self.selected.is_some() {
            context.add(SELECTED_CONTEXT);
        }

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .when(scoped, |d| d.key_context(context))
            .on_action(cx.listener(|this, _: &Undo, window, cx| this.undo(window, cx)))
            .on_action(cx.listener(|this, _: &Redo, window, cx| this.redo(window, cx)))
            .on_action(
                cx.listener(|this, _: &DeleteItem, window, cx| this.delete_selected(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &GroupItem, window, cx| this.group_selected(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &UngroupItem, window, cx| this.ungroup_selected(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeLeft, window, cx| this.nudge([-1., 0.], window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeRight, window, cx| this.nudge([1., 0.], window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeUp, window, cx| this.nudge([0., -1.], window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeDown, window, cx| this.nudge([0., 1.], window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeLeftFar, window, cx| {
                    this.nudge([-10., 0.], window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &NudgeRightFar, window, cx| {
                    this.nudge([10., 0.], window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &NudgeUpFar, window, cx| this.nudge([0., -10.], window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NudgeDownFar, window, cx| this.nudge([0., 10.], window, cx)),
            )
            .on_action(cx.listener(|this, _: &StepOut, window, cx| this.step_out(window, cx)))
            .when(!design, |d| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.open_panel_menu(event, window, cx)
                    }),
                )
            })
            // A `.roxo` dropped on the stage goes in here. Anything else is
            // left alone, as it was before the canvas took drops.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                for path in paths.paths().iter().filter(|path| is_object_file(path)) {
                    objects::offer_file(this, path, window, cx);
                }
            }))
            .child(watch)
            .children(layers)
            .children(clicks)
            .children(chrome)
            .children(inspector)
            .children(tab)
            .children(menu)
            .children(dialog)
    }

    /// Every followed state's case now. Marks resolve only when an item
    /// follows the favourite, and then only on a new track.
    fn read_cases(&mut self, window: &Window, cx: &mut Context<Self>) -> Cases {
        let wanted = states::needs_marks(&self.config.canvas);
        let marks = controls::playing_marks(&mut self.marks, &self.state, wanted, cx);

        let live = buttons::Live {
            player: self.state.player.read(cx),
            library: self.state.library.read(cx),
            scrobbler: self.state.scrobbler.read(cx),
            discord: self.state.discord.read(cx),
            marks,
            shell: buttons::Shell {
                mini: crate::workspace::mini_showing(window, cx),
            },
        };
        states::read(&self.config.canvas, &live)
    }

    /// From an observer: a render only when a case moved since the last one.
    fn recheck_states(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !states::follows(&self.config.canvas) {
            return;
        }

        if self.read_cases(window, cx) != self.cases {
            cx.notify();
        }
    }

    pub(crate) fn viewport(&self) -> Viewport {
        let size = self.bounds.get().size;
        Viewport {
            width: f32::from(size.width),
            height: f32::from(size.height),
        }
    }

    /// How many group levels this canvas has left before the cap, counting
    /// the canvases it sits inside.
    pub(crate) fn levels_left(&self) -> usize {
        edit::levels_left(self.level)
    }

    fn is_active(&self, cx: &Context<Self>) -> bool {
        ACTIVE.get() == Some(cx.entity_id())
    }
}

/// What one render collects while it places the tree.
struct Draw<'a> {
    at: &'a Placing,
    lives: &'a HashMap<String, Live>,
    values: &'a Values,
    cases: &'a Cases,
    /// The selected item's id and the case the inspector previews on it.
    preview: Option<(&'a str, &'a str)>,
    /// Inside a group its case hides, in design mode.
    ghost: bool,
    images: &'a mut Images,
    shaders: &'a mut Shaders,
    textures: &'a mut HashMap<String, Entity<TextureView>>,
    targets: &'a mut Targets,
    layers: Vec<AnyElement>,
    shapes: Vec<Shape>,
}

/// One sibling's slot in paint order.
struct Layer {
    depth: f32,
    elements: Vec<AnyElement>,
    shapes: Vec<Shape>,
}

/// Siblings back to front. A group takes one slot at its own depth and lays
/// its children out inside it.
fn place(
    items: &[CanvasItem],
    bodies: &[Body<Grid>],
    parent: &Frame,
    groups: &[String],
    draw: &mut Draw,
    cx: &mut Context<CanvasPanel>,
) {
    let at = draw.at;
    let mut layers: Vec<Layer> = Vec::new();

    for (item, body) in items.iter().zip(bodies) {
        if !item.visible {
            draw.targets.hold(&item.id);
            continue;
        }

        let preview = draw
            .preview
            .filter(|(id, _)| *id == item.id)
            .map(|(_, case)| case);
        let applied = states::resolve(item, draw.cases, preview);

        // Off design mode a hidden case takes the item, and a group's
        // children, off the stage and out of the hit walk. In design mode it
        // draws as a ghost so it can still be picked.
        if !applied.visible && !at.design {
            draw.targets.hold(&item.id);
            continue;
        }
        let ghost = draw.ghost || !applied.visible;

        let mut live = routes::live_for(draw.lives, item);
        if applied.visible {
            live = states::fold(live, &applied);
        } else if !draw.ghost {
            live.opacity *= states::GHOST;
        }

        let flat = shader_item::sits_flat(&item.content, draw.shaders.painted_flat(&item.id, cx));
        let kind = if flat {
            ShapeKind::Flat
        } else {
            ShapeKind::Perspective
        };
        let shape = |corners, kind| Shape {
            id: item.id.clone(),
            groups: groups.to_vec(),
            corners,
            label: SharedString::from(if item.name.is_empty() {
                item.id.clone()
            } else {
                item.name.clone()
            }),
            kind,
            locked: item.locked,
            hidden: ghost,
        };
        let projected = || {
            project(
                item,
                parent,
                &at.camera,
                at.viewport,
                at.pointer,
                flat,
                &live,
            )
        };

        match (body, &item.content) {
            (Body::Grid(grid), _) => {
                let Some(projected) = projected() else {
                    continue;
                };
                layers.push(Layer {
                    depth: projected.depth,
                    elements: vec![grid_element(
                        item,
                        grid,
                        projected.corners,
                        projected.opacity,
                    )],
                    shapes: vec![shape(projected.corners, ShapeKind::Grid)],
                });
            }

            (Body::Group(children), Content::Group { items }) => {
                let frame = child_frame(item, parent, &live);
                let center = [frame.size[0] / 2., frame.size[1] / 2., 0.];
                let depth = frame.transform.point(center)[2];

                let mut path = groups.to_vec();
                path.push(item.id.clone());

                let mut inner = Draw {
                    at,
                    lives: draw.lives,
                    values: draw.values,
                    cases: draw.cases,
                    preview: draw.preview,
                    ghost,
                    images: &mut *draw.images,
                    shaders: &mut *draw.shaders,
                    textures: &mut *draw.textures,
                    targets: &mut *draw.targets,
                    layers: Vec::new(),
                    shapes: Vec::new(),
                };
                place(items, children, &frame, &path, &mut inner, cx);
                let Draw {
                    layers: elements,
                    mut shapes,
                    ..
                } = inner;

                // A group draws nothing, so its outline is the box around
                // what it holds.
                if let Some(bound) = around(&shapes) {
                    shapes.push(shape(bound, ShapeKind::Group));
                }
                layers.push(Layer {
                    depth,
                    elements,
                    shapes,
                });
            }

            (Body::Image, _) => {
                let element =
                    image::element(item, applied.asset, parent, at, &live, draw.images, cx);
                let Some(projected) = projected() else {
                    continue;
                };
                layers.push(Layer {
                    depth: projected.depth,
                    elements: element.map(|(_, e)| e).into_iter().collect(),
                    shapes: vec![shape(projected.corners, ShapeKind::Perspective)],
                });
            }

            (Body::Shader, _) => {
                let Some(projected) = projected() else {
                    continue;
                };
                let element = canvas::on_stage(&projected.corners, at.viewport)
                    .then(|| shader_item::element(item, projected, at.design, draw.shaders, cx))
                    .flatten();
                layers.push(Layer {
                    depth: projected.depth,
                    elements: element.into_iter().collect(),
                    shapes: vec![shape(projected.corners, kind)],
                });
            }

            (
                Body::Text(line),
                Content::Text {
                    font,
                    size,
                    color,
                    align,
                    ..
                },
            ) => {
                let Some(projected) = projected() else {
                    continue;
                };
                let paint = text::Paint {
                    text: SharedString::from(line.render(draw.values)),
                    family: font.clone(),
                    size: size * parent.k * projected.scale,
                    color: text::color(applied.color.unwrap_or(color.as_str())),
                    opacity: projected.opacity,
                    align: *align,
                };
                let element = canvas::on_stage(&projected.corners, at.viewport)
                    .then(|| text::element(paint, &projected.corners));

                layers.push(Layer {
                    depth: projected.depth,
                    elements: element.into_iter().collect(),
                    shapes: vec![shape(projected.corners, ShapeKind::Flat)],
                });
            }

            (Body::Target, Content::Target { .. }) => {
                let Some(projected) = projected() else {
                    draw.targets.hold(&item.id);
                    continue;
                };
                let seen = canvas::on_stage(&projected.corners, at.viewport);
                let element = draw
                    .targets
                    .element(item, seen.then_some(projected), at.design, cx);

                layers.push(Layer {
                    depth: projected.depth,
                    elements: element.into_iter().collect(),
                    shapes: vec![shape(projected.corners, kind)],
                });
            }

            (Body::Unknown, content) if texture::is_test(content) => {
                let Some(projected) = projected() else {
                    continue;
                };
                let view = draw
                    .textures
                    .entry(item.id.clone())
                    .or_insert_with(|| {
                        cx.new(|cx| TextureView::new(texture::gradient(256, 256), cx))
                    })
                    .clone();
                let seen = canvas::on_stage(&projected.corners, at.viewport);
                view.update(cx, |view, _| view.place(seen.then_some(projected)));

                layers.push(Layer {
                    depth: projected.depth,
                    elements: vec![view.into_any_element()],
                    shapes: vec![shape(projected.corners, kind)],
                });
            }

            // Kinds with no body yet still have a place, so the editor can
            // pick and move them.
            _ => {
                let Some(projected) = projected() else {
                    continue;
                };
                layers.push(Layer {
                    depth: projected.depth,
                    elements: Vec::new(),
                    shapes: vec![shape(projected.corners, kind)],
                });
            }
        }
    }

    let depths: Vec<f32> = layers.iter().map(|layer| layer.depth).collect();
    let mut layers: Vec<Option<Layer>> = layers.into_iter().map(Some).collect();
    for ix in paint_order(&depths) {
        if let Some(layer) = layers[ix].take() {
            draw.layers.extend(layer.elements);
            draw.shapes.extend(layer.shapes);
        }
    }
}

/// The screen box around every shape, as corners. None for no shapes.
fn around(shapes: &[Shape]) -> Option<[[f32; 2]; 4]> {
    let corners: Vec<[f32; 2]> = shapes.iter().flat_map(|shape| shape.corners).collect();
    let first = *corners.first()?;

    let (mut min, mut max) = (first, first);
    for [x, y] in corners {
        min = [min[0].min(x), min[1].min(y)];
        max = [max[0].max(x), max[1].max(y)];
    }

    Some([min, [max[0], min[1]], max, [min[0], max[1]]])
}

/// A grid is flat, so its corners are a screen-aligned box.
fn grid_element(
    item: &CanvasItem,
    grid: &Grid,
    corners: [[f32; 2]; 4],
    opacity: f32,
) -> AnyElement {
    let (origin, extent) = canvas::bbox(&corners);

    div()
        .id(SharedString::from(format!("canvas-grid-{}", item.id)))
        .absolute()
        .left(px(origin[0]))
        .top(px(origin[1]))
        .w(px(extent[0]))
        .h(px(extent[1]))
        .when(opacity < 1., |d| d.opacity(opacity))
        .child(grid.area.clone())
        .into_any_element()
}

// ── Editing ──

impl CanvasPanel {
    /// Selects `id` and makes this the canvas whose chrome and inspector
    /// show. Focus comes here too, so the canvas's keys apply.
    pub(crate) fn select(
        &mut self,
        id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = id;
        self.activate(window, cx);
        cx.notify();
    }

    /// The kept group of the grid item selected here, while that grid holds
    /// no panel. Empty, the group draws nothing to click, so selecting its
    /// item in design mode is how Add Panel is pointed at it.
    fn selected_empty_group(&self, cx: &App) -> Option<Entity<TabPanel>> {
        fn kept(node: &Arc<dyn PanelView>, cx: &App) -> Option<Entity<TabPanel>> {
            let view = node.view();
            if let Ok(tabs) = view.clone().downcast::<TabPanel>() {
                return tabs.read(cx).keeps_place(cx).then_some(tabs);
            }
            let stack = view.downcast::<StackPanel>().ok()?;
            stack
                .read(cx)
                .panels()
                .iter()
                .find_map(|child| kept(child, cx))
        }

        if !self.design {
            return None;
        }
        let id = self.selected.as_deref()?;

        let mut area = None;
        canvas::each_grid(
            &self.config.canvas.items,
            &self.bodies,
            &mut |item, grid| {
                if item.id == id {
                    area = Some(grid.area.clone());
                }
            },
        );
        kept(&area?.read(cx).items().view(), cx)
    }

    /// Closes the object dialog and hands the keys back to the canvas. Next
    /// turn, because the button pressed to close it takes focus on the same
    /// press, after this runs, and undo would go nowhere until a click.
    pub(crate) fn close_object_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.object_dialog = None;
        cx.notify();

        cx.defer_in(window, |this, window, cx| {
            window.focus(&this.focus);
            cx.notify();
        });
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);

        let me = cx.entity_id();
        if let Some(previous) = ACTIVE.replace(Some(me))
            && previous != me
        {
            App::notify(cx, previous);
        }
    }

    /// Hides the inspector and the selection chrome, leaving the stage as
    /// plain content until the edge tab or a pick brings them back. Nothing
    /// floats over the panels' own controls meanwhile.
    pub(crate) fn put_away(&mut self, cx: &mut Context<Self>) {
        if ACTIVE.get() == Some(cx.entity_id()) {
            ACTIVE.set(None);
        }

        self.selected = None;
        self.gesture = None;
        cx.notify();
    }

    fn shape(&self, id: &str) -> Option<&Shape> {
        self.shapes.iter().find(|shape| shape.id == id)
    }

    /// The handles and strips over the stage, painted after every item.
    fn chrome_layer(&self, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity().downgrade();
        let selected = active
            .then(|| self.selected.as_ref().and_then(|id| self.shape(id)))
            .flatten()
            .cloned();

        let paint_selected = selected.clone();
        let canvas = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                if let Some(shape) = &paint_selected {
                    paint_outline(shape, bounds.origin, window);
                    if shape.turns() {
                        paint_turn_handles(shape, bounds.origin, window);
                    }
                }

                // A popover from an inspector picker floats outside the
                // inspector's own press guard, and a press on its rows must
                // not land on the stage.
                let down = me.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !hitbox.is_hovered(window) {
                        return;
                    }

                    let local = local_point(event.position, bounds.origin);
                    down.update(cx, |this, cx| this.press(local, event, window, cx))
                        .ok();
                });

                let moving = me.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }

                    let local = local_point(event.position, bounds.origin);
                    let held = event.pressed_button == Some(MouseButton::Left);
                    moving
                        .update(cx, |this, cx| {
                            if this.gesture.is_some() {
                                if held {
                                    this.drag_to(local, event.modifiers.shift, cx);
                                } else {
                                    // The release happened where this
                                    // window couldn't see it.
                                    this.gesture = None;
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });

                let up = me.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.bubble() || event.button != MouseButton::Left {
                        return;
                    }

                    up.update(cx, |this, cx| this.release(window, cx)).ok();
                });
            },
        )
        .absolute()
        .size_full();

        // Every flat item gets a strip, so any grid can be picked; later ones
        // in paint order sit over earlier ones.
        // A strip on an item flush with the top would sit inside it, over the
        // panels' own top row, so that one shows only once the item is
        // selected from the inspector's tree. A converted window keeps its
        // menu button and window controls until editing starts.
        let strips = self
            .shapes
            .iter()
            .filter(|shape| shape.flat())
            .filter_map(|shape| {
                let lit = active && self.selected.as_deref() == Some(shape.id.as_str());
                let flush = shape.edges()[1] < STRIP_H;
                (lit || !flush).then(|| self.strip(shape, lit, cx))
            })
            .collect::<Vec<_>>();

        let handles = selected
            .filter(|shape| shape.flat() && !shape.locked)
            .map(|shape| self.resize_handles(&shape, cx))
            .unwrap_or_default();

        div()
            .absolute()
            .size_full()
            .child(canvas)
            .children(strips)
            .children(handles)
            .into_any_element()
    }

    /// The way into the editor when nothing on the stage can be clicked, as a
    /// full-window grid leaves it: a tab on the inspector's side that shows
    /// while the pointer is near that edge, and draws nothing otherwise.
    fn edge_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let left = self.inspector.on_left();
        let height = self.viewport().height;

        div()
            .id("canvas-edge-tab")
            .absolute()
            .top(px((height - 48.) / 2.))
            .when(left, |d| d.left_0().rounded_r(px(6.)).border_r_1())
            .when(!left, |d| d.right_0().rounded_l(px(6.)).border_l_1())
            .w(px(22.))
            .h(px(48.))
            .flex()
            .items_center()
            .justify_center()
            .bg(palette::bg_menu_opaque())
            .border_t_1()
            .border_b_1()
            .border_color(palette::accent())
            .cursor_pointer()
            .occlude()
            .child(
                gpui::svg()
                    .path(icons::LAYERS)
                    .size(px(14.))
                    .text_color(palette::accent()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.select(None, window, cx);
                }),
            )
            .into_any_element()
    }

    fn strip(&self, shape: &Shape, lit: bool, cx: &mut Context<Self>) -> AnyElement {
        let [left, top, right, _] = shape.edges();

        // Outside the top edge when there's room, so it covers none of the
        // panel; inside it for an item flush with the top of the canvas.
        let y = if top >= STRIP_H { top - STRIP_H } else { top };
        let accent = palette::accent();
        let chain = shape.chain();

        div()
            .id(SharedString::from(format!("canvas-strip-{}", shape.id)))
            .absolute()
            .left(px(left))
            .top(px(y))
            .w(px((right - left).max(STRIP_H)))
            .h(px(STRIP_H))
            .px(px(6.))
            .flex()
            .items_center()
            .overflow_hidden()
            .text_xs()
            .bg(palette::alpha(accent, if lit { 0xd0 } else { 0x55 }))
            .text_color(if lit {
                palette::text_on_accent()
            } else {
                palette::text()
            })
            .cursor(if shape.locked {
                CursorStyle::Arrow
            } else {
                CursorStyle::OpenHand
            })
            .child(div().truncate().child(shape.label.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let local = local_point(event.position, this.bounds.get().origin);
                    let target = this.choose(local, &chain);
                    this.select(Some(target.clone()), window, cx);
                    this.begin(&target, local, Begin::Move, cx);
                }),
            )
            .into_any_element()
    }

    fn resize_handles(&self, shape: &Shape, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let [left, top, right, bottom] = shape.edges();
        let mid = [(left + right) / 2., (top + bottom) / 2.];

        Edges::ALL
            .iter()
            .enumerate()
            .map(|(ix, &edges)| {
                let x = if edges.left {
                    left
                } else if edges.right {
                    right
                } else {
                    mid[0]
                };
                let y = if edges.top {
                    top
                } else if edges.bottom {
                    bottom
                } else {
                    mid[1]
                };
                let cursor = match (edges.left || edges.right, edges.top || edges.bottom) {
                    (true, false) => CursorStyle::ResizeLeftRight,
                    (false, true) => CursorStyle::ResizeUpDown,
                    _ if edges.left == edges.top => CursorStyle::ResizeUpLeftDownRight,
                    _ => CursorStyle::ResizeUpRightDownLeft,
                };
                let id = shape.id.clone();

                div()
                    .id(("canvas-resize", ix))
                    .absolute()
                    .left(px(x - HANDLE / 2.))
                    .top(px(y - HANDLE / 2.))
                    .size(px(HANDLE))
                    .bg(palette::bg_panel())
                    .border_1()
                    .border_color(palette::accent())
                    .cursor(cursor)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let local = local_point(event.position, this.bounds.get().origin);
                            this.begin(&id, local, Begin::Resize(edges), cx);
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// A press on the stage that no strip, handle or inspector took.
    fn press(
        &mut self,
        local: [f32; 2],
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.design
            && self.is_active(cx)
            && let Some(turn) = self.turn_handle_at(local)
        {
            cx.stop_propagation();
            let id = self.selected.clone().unwrap_or_default();
            self.begin(&id, local, Begin::Turn(turn), cx);
            return;
        }

        let hit = self
            .shapes
            .iter()
            .rev()
            .filter(|shape| shape.kind != ShapeKind::Group)
            .find(|shape| inside(&shape.corners, local))
            .cloned();

        // A grid's body belongs to its panels.
        if hit
            .as_ref()
            .is_some_and(|shape| shape.kind == ShapeKind::Grid)
        {
            return;
        }

        cx.stop_propagation();

        // The add menu belongs to empty stage only.
        if event.button == MouseButton::Right {
            if hit.is_none() {
                self.open_add_menu(event.position, Some(local), window, cx);
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }

        match hit {
            Some(shape) => {
                let target = self.choose(local, &shape.chain());
                self.select(Some(target.clone()), window, cx);
                self.begin(&target, local, Begin::Move, cx);
            }
            None => {
                self.last_press = None;
                self.select(None, window, cx);
            }
        }
    }

    /// The outermost group under the press, or one level further in when
    /// the press lands where the last one did and that level is selected.
    fn choose(&mut self, local: [f32; 2], chain: &[String]) -> String {
        let again = self.last_press.as_ref().is_some_and(|(at, last)| {
            (at[0] - local[0]).hypot(at[1] - local[1]) <= SAME_SPOT && last == chain
        });

        let deeper = again
            .then(|| {
                let at = chain
                    .iter()
                    .position(|id| Some(id) == self.selected.as_ref())?;
                chain.get(at + 1).cloned()
            })
            .flatten();

        self.last_press = Some((local, chain.to_vec()));
        deeper.unwrap_or_else(|| chain[0].clone())
    }

    /// Which turn handle of the selected item sits under the pointer: the
    /// ring turns about z, the right grip about y, the top grip about x.
    fn turn_handle_at(&self, local: [f32; 2]) -> Option<usize> {
        let shape = self.shape(self.selected.as_deref()?)?;
        if !shape.turns() {
            return None;
        }

        let handles = TurnHandles::of(shape);
        let near = |p: [f32; 2]| (p[0] - local[0]).hypot(p[1] - local[1]) <= GRIP_REACH;

        if near(handles.right) {
            return Some(1);
        }
        if near(handles.top) {
            return Some(0);
        }

        let off_center = (local[0] - handles.center[0]).hypot(local[1] - handles.center[1]);
        ((off_center - handles.ring).abs() <= GRIP_REACH / 1.5).then_some(2)
    }

    fn begin(&mut self, id: &str, local: [f32; 2], begin: Begin, cx: &mut Context<Self>) {
        self.config = self.saved(cx);

        let viewport = self.viewport();
        let canvas = &self.config.canvas;
        let (Some(item), Some(parent), Some(shape)) = (
            canvas.find(id).cloned(),
            edit::parent_frame(canvas, id, viewport),
            self.shape(id).cloned(),
        ) else {
            return;
        };
        if item.locked {
            return;
        }

        let kind = match begin {
            Begin::Move => GestureKind::Move(self.snap_lines(&shape, &parent, viewport)),
            Begin::Resize(edges) => GestureKind::Resize {
                edges,
                floor: self.floor(id, cx),
            },
            Begin::Turn(axis) => {
                let handles = TurnHandles::of(&shape);
                let reach = if axis == 1 {
                    handles.right
                } else {
                    handles.top
                };
                GestureKind::Turn {
                    axis,
                    center: handles.center,
                    radius: (reach[0] - handles.center[0]).hypot(reach[1] - handles.center[1]),
                }
            }
        };

        let flat = self.is_flat(&item, cx);
        self.gesture = Some(Gesture {
            id: id.to_string(),
            before: self.config.canvas.clone(),
            item,
            parent,
            camera: self.config.canvas.camera,
            viewport,
            from: local,
            start_box: shape.edges(),
            flat,
            kind,
        });
    }

    /// The parent box's edges and center, and the edges of every flat
    /// sibling, as they sit on screen.
    fn snap_lines(&self, moving: &Shape, parent: &Frame, viewport: Viewport) -> [Vec<f32>; 2] {
        let mut xs = Vec::new();
        let mut ys = Vec::new();

        let parent_box = CanvasItem {
            place: Place {
                anchor: Anchor::TopLeft,
                size: [Length::Units(parent.size[0]), Length::Units(parent.size[1])],
                ..Place::default()
            },
            ..CanvasItem::default()
        };
        let camera = self.config.canvas.camera;
        if let Some(projected) = project(
            &parent_box,
            parent,
            &camera,
            viewport,
            None,
            false,
            &Live::default(),
        ) {
            let (origin, extent) = canvas::bbox(&projected.corners);
            xs.extend([origin[0], origin[0] + extent[0] / 2., origin[0] + extent[0]]);
            ys.extend([origin[1], origin[1] + extent[1] / 2., origin[1] + extent[1]]);
        }

        for sibling in &self.shapes {
            if sibling.id == moving.id || sibling.groups != moving.groups || !sibling.flat() {
                continue;
            }

            let [left, top, right, bottom] = sibling.edges();
            xs.extend([left, right]);
            ys.extend([top, bottom]);
        }

        [xs, ys]
    }

    /// The smallest box a resize leaves, in pixels: a grid's dock floor.
    fn floor(&self, id: &str, cx: &App) -> [f32; 2] {
        let mut floor = [MIN_BOX, MIN_BOX];
        canvas::each_grid(
            &self.config.canvas.items,
            &self.bodies,
            &mut |item, grid| {
                if item.id == id {
                    let min = grid.area.read(cx).items().view().min_size(cx);
                    floor = [f32::from(min.width), f32::from(min.height)];
                }
            },
        );
        floor
    }

    fn drag_to(&mut self, local: [f32; 2], shift: bool, cx: &mut Context<Self>) {
        let Some(gesture) = &self.gesture else {
            return;
        };

        let delta = [local[0] - gesture.from[0], local[1] - gesture.from[1]];
        let g = gesture;
        let mut place = g.item.place;

        match &g.kind {
            GestureKind::Move([xs, ys]) => {
                let b = g.start_box;
                let moved = [
                    b[0] + delta[0],
                    b[1] + delta[1],
                    b[2] + delta[0],
                    b[3] + delta[1],
                ];
                let pull = edit::snap(moved, xs, ys, edit::SNAP_REACH);
                let delta = [delta[0] + pull[0], delta[1] + pull[1]];

                place.position = edit::moved(
                    &g.item,
                    &g.parent,
                    &g.camera,
                    g.viewport,
                    g.item.place.position,
                    delta,
                    g.flat,
                );
            }

            GestureKind::Resize { edges, floor } => {
                place = edit::resized(
                    &g.item,
                    &g.parent,
                    &g.camera,
                    g.viewport,
                    g.start_box,
                    *edges,
                    delta,
                    *floor,
                    g.flat,
                );
            }

            GestureKind::Turn {
                axis,
                center,
                radius,
            } => {
                let swept = match axis {
                    2 => edit::ring_turn(*center, g.from, local),
                    // Pushing the right edge left takes it away from the
                    // viewer, which is a positive turn about y.
                    1 => edit::grip_turn(-delta[0], *radius),
                    _ => edit::grip_turn(delta[1], *radius),
                };
                place.rotation[*axis] = edit::snapped(g.item.place.rotation[*axis] + swept, shift);
            }
        }

        let id = g.id.clone();
        if let Some(item) = self.config.canvas.find_mut(&id) {
            item.place = place;
        }
        cx.notify();
    }

    fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };

        let routes = self.config.routes.clone();
        let before = Snapshot {
            canvas: gesture.before,
            routes: routes.clone(),
        };
        let after = Snapshot {
            canvas: self.saved(cx).canvas,
            routes,
        };
        self.history.record(before, after, None, Instant::now());

        self.save_soon(window, cx);
        cx.notify();
    }

    fn menu_overlay(&self, window: &Window) -> Option<AnyElement> {
        let (position, menu, _) = self.menu.as_ref()?;
        let size = window.bounds().size;

        Some(
            deferred(
                anchored().child(
                    div().w(size.width).h(size.height).occlude().child(
                        anchored()
                            .position(*position)
                            .snap_to_window_with_margin(px(8.))
                            .child(menu.clone()),
                    ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// Add Grid, Group, Image, Shader, Text and, with a renderer installed,
    /// Render Target at `position`, then the panel menu. `at` places the new
    /// item where the press was; None puts it at the stage center.
    pub(crate) fn open_add_menu(
        &mut self,
        position: Point<Pixels>,
        at: Option<[f32; 2]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        let pick = move |what: NewItem| {
            let me = me.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                me.update(cx, |this, cx| this.add(what.clone(), at, window, cx))
                    .ok();
            }
        };

        let images = image_assets();
        let image_menu = {
            let pick = pick.clone();
            let me = cx.entity().downgrade();
            PopupMenu::build(window, cx, move |mut menu, _, _| {
                for (id, file) in images {
                    menu = menu.item(
                        PopupMenuItem::new(SharedString::from(file))
                            .on_click(pick(NewItem::Image(id))),
                    );
                }

                menu.separator().item(
                    PopupMenuItem::new(rox_i18n::t!("canvas-add-file"))
                        .icon(Icon::default().path(icons::FOLDER))
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| this.add_file(at, window, cx)).ok();
                        }),
                )
            })
        };

        let object_menu = crate::canvas::overlay::objects_menu(window, cx);

        // Read once per opening: the list is the gate's, read off disk. One
        // that isn't ready is marked and still picks: placing it raises its
        // enable card.
        let renderers = target::renderer_choices(&rox_services::renderers::all());
        let target_menu = (!renderers.is_empty()).then(|| {
            let pick = pick.clone();
            PopupMenu::build(window, cx, move |mut menu, _, _| {
                for choice in renderers {
                    menu = menu.item(
                        PopupMenuItem::new(choice.shown())
                            .on_click(pick(NewItem::Target(choice.owner))),
                    );
                }
                menu
            })
        });

        let (panel, tab_panel, state) = (cx.entity(), self.tab_panel.clone(), self.state.clone());
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            let menu = menu
                .item(
                    PopupMenuItem::new(rox_i18n::t!("canvas-add-grid"))
                        .icon(Icon::default().path(icons::LAYOUT_GRID))
                        .on_click(pick(NewItem::Grid)),
                )
                .item(
                    PopupMenuItem::new(rox_i18n::t!("canvas-add-group"))
                        .icon(Icon::default().path(icons::LAYERS))
                        .on_click(pick(NewItem::Group)),
                )
                .item(
                    PopupMenuItem::submenu(rox_i18n::t!("canvas-add-image"), image_menu)
                        .icon(Icon::default().path(icons::IMAGE)),
                )
                .item(
                    PopupMenuItem::new(rox_i18n::t!("canvas-add-shader"))
                        .icon(Icon::default().path(icons::BLEND))
                        .on_click(pick(NewItem::Shader)),
                )
                .item(
                    PopupMenuItem::new(rox_i18n::t!("canvas-add-text"))
                        .icon(Icon::default().path(icons::A_LARGE_SMALL))
                        .on_click(pick(NewItem::Text)),
                );

            let menu = match target_menu {
                Some(targets) => menu.item(
                    PopupMenuItem::submenu(rox_i18n::t!("canvas-add-target"), targets)
                        .icon(Icon::default().path(icons::MONITOR_PLAY)),
                ),
                None => menu,
            };

            let menu = menu.separator().item(
                PopupMenuItem::submenu(rox_i18n::t!("canvas-add-objects"), object_menu)
                    .icon(Icon::default().path(icons::BOOKMARK)),
            );
            panel_tail(menu, &panel, tab_panel, state, window, cx)
        });
        self.open_menu(position, menu, window, cx);
    }

    /// Off design mode a right-click opens the panel menu, except over a
    /// grid, whose panels serve their own, or a render target, whose
    /// renderer gets the press.
    fn open_panel_menu(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = local_point(event.position, self.bounds.get().origin);
        if matches!(self.hit_at(local), Hit::Grid | Hit::Target { .. }) {
            return;
        }

        let (panel, tab_panel, state) = (cx.entity(), self.tab_panel.clone(), self.state.clone());
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            panel_tail(menu, &panel, tab_panel, state, window, cx)
        });
        self.open_menu(event.position, menu, window, cx);
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    /// Shows `menu` at `position` over everything, until it's dismissed.
    pub(crate) fn open_menu(
        &mut self,
        position: Point<Pixels>,
        menu: Entity<PopupMenu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        menu.focus_handle(cx).focus(window);

        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu = None;
            cx.notify();
        });
        self.menu = Some((position, menu, dismissed));
        cx.notify();
    }

    fn add(
        &mut self,
        what: NewItem,
        at: Option<[f32; 2]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;

        let placed = match &what {
            NewItem::Target(owner) => Some(owner.clone()),
            _ => None,
        };

        let (stem, content, size) = match what {
            NewItem::Grid => ("grid", Content::Grid { dock: empty_dock() }, None),
            NewItem::Group => ("group", Content::Group { items: Vec::new() }, None),
            NewItem::Image(asset) => (
                "image",
                Content::Image {
                    asset,
                    fit: ImageFit::Contain,
                },
                Some([400., 300.]),
            ),
            NewItem::Shader => (
                "shader",
                Content::Shader {
                    shader: serde_json::to_value(panel::PanelShader::default())
                        .unwrap_or(Value::Null),
                },
                Some([400., 300.]),
            ),
            NewItem::Text => (
                "text",
                Content::Text {
                    text: text::NEW_TEXT.to_string(),
                    font: None,
                    size: 32.,
                    color: "text".to_string(),
                    align: rox_canvas::Align::Center,
                },
                Some([480., 64.]),
            ),
            NewItem::Target(owner) => (
                "target",
                Content::Target {
                    rests: suggests_rest(&owner),
                    owner,
                    width: TARGET_PIXELS[0],
                    height: TARGET_PIXELS[1],
                    config: Value::Null,
                },
                Some(TARGET_UNITS),
            ),
        };

        let id = self.config.canvas.fresh_id(stem);
        let mut item = CanvasItem {
            id: id.clone(),
            content,
            ..CanvasItem::default()
        };
        if let Some([w, h]) = size {
            item.place.size = [Length::Units(w), Length::Units(h)];
        }

        // Named like its Add Target entry. An empty name shows the id.
        if let Content::Target { owner, .. } = &item.content {
            item.name = target_choices(&rox_services::renderers::all())
                .into_iter()
                .find(|(choice, _)| choice == owner)
                .map(|(_, label)| label)
                .unwrap_or_default();
        }

        // Carried from the stage center to the press.
        let viewport = self.viewport();
        if let Some(at) = at {
            let stage = Frame::stage(&self.config.canvas.stage, viewport);
            let camera = self.config.canvas.camera;
            let flat = edit::is_flat(&item.content);
            if let Some(projected) = project(
                &item,
                &stage,
                &camera,
                viewport,
                None,
                flat,
                &Live::default(),
            ) {
                let (origin, extent) = canvas::bbox(&projected.corners);
                let shift = [
                    at[0] - (origin[0] + extent[0] / 2.),
                    at[1] - (origin[1] + extent[1] / 2.),
                ];
                item.place.position =
                    edit::moved(&item, &stage, &camera, viewport, [0.; 3], shift, flat);
            }
        }

        // A press is where the user pointed. The stage center cascades, so a
        // second add there doesn't land exactly over the first.
        let cascade = at.is_none();
        self.change(
            move |canvas| {
                if cascade {
                    item.place.position = edit::cascaded(&canvas.items, &item.place);
                }
                canvas.items.push(item);
            },
            window,
            cx,
        );
        self.select(Some(id), window, cx);

        // A renderer that isn't switched on asks first, and offers its
        // signals once its card is accepted.
        if let Some(owner) = placed {
            match target::renderer_id(&owner).map(target::Readiness::of) {
                Some(target::Readiness::Off) => self.ask_enable(&owner, true, window, cx),
                _ => self.offer_signals(&owner, window, cx),
            }
        }
    }

    /// Raises the enable card for the renderer `owner` names over this
    /// window. `offer` is a target just placed, whose wanted signals are
    /// offered once the card is accepted. A renderer with no card to show,
    /// one that doesn't load, opens the Renderers page instead.
    pub(crate) fn ask_enable(
        &mut self,
        owner: &str,
        offer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = target::renderer_id(owner).and_then(EnableCard::for_renderer) else {
            panel_settings::open_app_page(RENDERERS_PAGE, window, cx);
            return;
        };

        let focus = cx.focus_handle();
        focus.focus(window);
        self.object_dialog = Some(objects::Dialog::Enable {
            card: Box::new(card),
            then_offer: offer.then(|| owner.to_string()),
            focus,
        });
        cx.notify();
    }

    /// [`ask_enable`](Self::ask_enable) from a control that isn't handed the
    /// window, on the next turn.
    pub(crate) fn ask_enable_later(&mut self, owner: String, cx: &mut Context<Self>) {
        let Some(handle) = self.window else {
            return;
        };

        let me = cx.entity().downgrade();
        let app: &mut App = cx;
        app.defer(move |cx| {
            handle
                .update(cx, |_, window, cx| {
                    me.update(cx, |this, cx| this.ask_enable(&owner, false, window, cx))
                        .ok();
                })
                .ok();
        });
    }

    /// The enable card's Switch On, the only approval on this path: what the
    /// settings switch does, then the signals offer for a target just placed.
    pub(crate) fn confirm_enable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(objects::Dialog::Enable {
            card, then_offer, ..
        }) = self.object_dialog.take()
        else {
            return;
        };
        self.close_object_dialog(window, cx);

        card.accept(cx);
        if let Some(owner) = then_offer {
            self.offer_signals(&owner, window, cx);
        }
    }

    /// The signals the renderer `owner` names asks for that the pool hasn't
    /// got, worded for the prompt and the inspector. Only for a renderer
    /// approved as it stands: its card listed them before anything is
    /// offered.
    pub(crate) fn missing_signals(
        &self,
        owner: &str,
        renderers: &[rox_services::renderers::Renderer],
    ) -> Vec<rox_viz::signal::Signal> {
        let renderer = target::renderer_id(owner)
            .and_then(|id| renderers.iter().find(|r| r.id == id && r.approved));

        match renderer {
            Some(renderer) => rox_services::renderers::signals_to_add(
                &renderer.signals,
                &self.state.signals.pool(),
            ),
            None => Vec::new(),
        }
    }

    /// Asks before adding what a new render target's renderer follows. Never
    /// outside design mode, and never on its own: nothing goes in the pool
    /// until Add.
    fn offer_signals(&mut self, owner: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.design {
            return;
        }

        let renderers = rox_services::renderers::all();
        let missing = self.missing_signals(owner, &renderers);
        if missing.is_empty() {
            return;
        }

        let name = target::renderer_id(owner)
            .and_then(|id| renderers.iter().find(|r| r.id == id))
            .map(|renderer| renderer.name.clone())
            .unwrap_or_default();
        let words = target::words_for(&missing, &self.state.signals.pool());

        let focus = cx.focus_handle();
        focus.focus(window);
        self.object_dialog = Some(objects::Dialog::Signals {
            name,
            owner: owner.to_string(),
            words,
            focus,
        });
        cx.notify();
    }

    /// The prompt's Add.
    pub(crate) fn confirm_signals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(objects::Dialog::Signals { owner, .. }) = self.object_dialog.take() else {
            return;
        };
        self.close_object_dialog(window, cx);

        self.add_wanted_signals(&owner, cx);
    }

    /// Adds what renderer `owner` asks for and the pool lacks, read afresh,
    /// the way an object's signals go in. The inspector's Add Signals and
    /// the prompt's Add, both a click of the user's.
    pub(crate) fn add_wanted_signals(&mut self, owner: &str, cx: &mut Context<Self>) {
        if !self.design {
            return;
        }

        let adding = self.missing_signals(owner, &rox_services::renderers::all());
        if adding.is_empty() {
            return;
        }

        let hub = self.state.signals.clone();
        hub.edit(|pool| {
            objects::merge_signals(pool, &adding);
        });
        rox_panel_api::signal_ui::persist_pool_soon(&hub, cx);

        log::info!("canvas: added {} signals {owner} asks for", adding.len());
        cx.notify();
    }

    /// Imports a picked file into the asset store, then adds it as an image.
    fn add_file(&mut self, at: Option<[f32; 2]>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;

        let rx = rox_core::desktop::prompt_for_paths(
            cx,
            "canvas: add file",
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: None,
            },
        );

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };

            let staged = cx
                .background_executor()
                .spawn(stage_image(path.clone()))
                .await;

            this.update_in(cx, |this, window, cx| {
                match staged.and_then(assets::commit_staged) {
                    Ok(asset) => this.add(NewItem::Image(asset.id), at, window, cx),
                    Err(e) => {
                        log::warn!("canvas: adding {}: {e}", path.display());
                        Toast::new(Tone::Bad, e)
                            .title(rox_i18n::t!("canvas-add-image-failed"))
                            .show(window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Installs a picked `.roxo`, then offers it to this canvas.
    pub(crate) fn add_object_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;

        let rx = rox_core::desktop::prompt_for_paths(
            cx,
            "canvas: add object from file",
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: None,
            },
        );

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };

            this.update_in(cx, |this, window, cx| {
                objects::offer_file(this, &path, window, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.take() else {
            return;
        };

        self.change(move |canvas| drop(edit::remove(canvas, &id)), window, cx);
    }

    pub(crate) fn group_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        let levels = self.levels_left();
        let mut made = None;
        self.change(|canvas| made = edit::group(canvas, &id, levels), window, cx);

        if made.is_some() {
            self.select(made, window, cx);
        }
    }

    pub(crate) fn ungroup_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !matches!(
            self.config.canvas.find(&id).map(|item| &item.content),
            Some(Content::Group { .. })
        ) {
            return;
        }

        let viewport = self.viewport();
        self.change(
            |canvas| {
                edit::ungroup(canvas, &id, viewport);
            },
            window,
            cx,
        );
        self.select(None, window, cx);
    }

    /// Moves the selected item into `group`, or to the top level for None.
    pub(crate) fn move_into(
        &mut self,
        id: &str,
        group: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.viewport();
        let levels = self.levels_left();
        self.change(
            |canvas| {
                edit::move_into(canvas, id, group, viewport, levels);
            },
            window,
            cx,
        );
    }

    /// Bring Forward and Send Back: one sibling past in paint order.
    pub(crate) fn step_depth(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        let viewport = self.viewport();
        self.change(
            |canvas| {
                let Some(parent) = edit::parent_frame(canvas, &id, viewport) else {
                    return;
                };
                let Some(siblings) = edit::siblings(&canvas.items, &id) else {
                    return;
                };
                let Some(index) = siblings.iter().position(|item| item.id == id) else {
                    return;
                };

                let depths = edit::depths(siblings, &parent);
                let Some(target) = edit::step_depth(&depths, index, forward) else {
                    return;
                };
                let z = edit::z_for_depth(&siblings[index], &parent, depths[index], target);

                if let Some(item) = canvas.find_mut(&id) {
                    item.place.position[2] = z;
                }
            },
            window,
            cx,
        );
    }

    /// Bring to Front and Send to Back: past every sibling in paint order.
    pub(crate) fn end_depth(&mut self, front: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        let viewport = self.viewport();
        self.change(
            |canvas| {
                let Some(parent) = edit::parent_frame(canvas, &id, viewport) else {
                    return;
                };
                let Some(siblings) = edit::siblings(&canvas.items, &id) else {
                    return;
                };
                let Some(index) = siblings.iter().position(|item| item.id == id) else {
                    return;
                };

                let depths = edit::depths(siblings, &parent);
                let Some(target) = edit::end_depth(&depths, index, front) else {
                    return;
                };
                let z = edit::z_for_depth(&siblings[index], &parent, depths[index], target);

                if let Some(item) = canvas.find_mut(&id) {
                    item.place.position[2] = z;
                }
            },
            window,
            cx,
        );
    }

    /// The inspector's align, fill and fit, on the selected item against its
    /// parent's box: the stage, or the group it's in. One undo step. A locked
    /// item doesn't move.
    pub(crate) fn arrange_selected(
        &mut self,
        arrange: canvas::align::Arrange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(item) = self.config.canvas.find(&id).filter(|item| !item.locked) else {
            return;
        };

        let flat = self.is_flat(item, cx);
        let viewport = self.viewport();
        self.change(
            |canvas| {
                let Some(parent) = edit::parent_frame(canvas, &id, viewport) else {
                    return;
                };
                let Some(item) = canvas.find_mut(&id) else {
                    return;
                };

                if let Some(place) =
                    canvas::align::arranged(&item.place, parent.size, flat, arrange)
                {
                    item.place = place;
                }
            },
            window,
            cx,
        );
    }

    /// Arrow keys: one unit in the parent's space, ten with shift.
    fn nudge(&mut self, by: [f32; 2], window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.config.canvas.find(&id).is_none_or(|item| item.locked) {
            return;
        }

        self.change_keyed(
            Some("nudge"),
            |canvas| {
                if let Some(item) = canvas.find_mut(&id) {
                    item.place.position[0] += by[0];
                    item.place.position[1] += by[1];
                }
            },
            window,
            cx,
        );
    }

    /// Escape: out to the group holding the selection, then to nothing.
    fn step_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parent = self.selected.as_ref().and_then(|id| {
            let mut path = edit::path_to(&self.config.canvas.items, id)?;
            path.pop();
            path.pop()
        });

        self.select(parent, window, cx);
    }
}

// ── Commands ──

/// What a click outside design mode lands on.
#[derive(Debug, PartialEq)]
pub(crate) enum Hit {
    /// A grid's panels own their input.
    Grid,
    /// The item whose command runs: the one clicked, or the nearest group
    /// around it that has one.
    Command { id: String, action: String },
    /// A render target with no command over it, and where on its surface,
    /// 0 to 1 from its top-left.
    Target { id: String, at: [f32; 2] },
    /// Nothing commanded, so the click goes on to whatever is behind.
    Nothing,
}

/// The front item under `point`, then its command or the nearest group's.
/// `shapes` is in paint order, back to front.
pub(crate) fn hit(shapes: &[Shape], canvas: &CanvasState, point: [f32; 2]) -> Hit {
    let front = shapes
        .iter()
        .rev()
        .filter(|shape| shape.kind != ShapeKind::Group && !shape.hidden)
        .filter(|shape| canvas.find(&shape.id).is_some_and(|item| item.visible))
        .find(|shape| inside(&shape.corners, point));
    let Some(front) = front else {
        return Hit::Nothing;
    };

    if front.kind == ShapeKind::Grid {
        return Hit::Grid;
    }

    let commanded = front
        .chain()
        .iter()
        .rev()
        .filter_map(|id| canvas.find(id))
        .find(|item| commands(item));
    if let Some(item) = commanded {
        return Hit::Command {
            id: item.id.clone(),
            action: item.action.clone(),
        };
    }

    // A target's renderer gets the pointer only where nothing else claims
    // it: it's the front item and no command covers it.
    let target = canvas
        .find(&front.id)
        .is_some_and(|item| matches!(item.content, Content::Target { .. }));
    match unit_point(&front.corners, point) {
        Some([x, y]) if target => Hit::Target {
            id: front.id.clone(),
            at: [x.clamp(0., 1.), y.clamp(0., 1.)],
        },
        _ => Hit::Nothing,
    }
}

/// A button's name on the renderer wire.
pub(crate) fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Navigate(NavigationDirection::Back) => "back",
        MouseButton::Navigate(NavigationDirection::Forward) => "forward",
    }
}

/// A wheel's travel in lines, at the 20 pixels a line the other wheel
/// handlers use.
pub(crate) fn scroll_lines(delta: ScrollDelta) -> [f32; 2] {
    match delta {
        ScrollDelta::Lines(lines) => [lines.x, lines.y],
        ScrollDelta::Pixels(pixels) => [f32::from(pixels.x) / 20., f32::from(pixels.y) / 20.],
    }
}

/// A grid's command is ignored: its panels own their input. An id this
/// build can't run counts as none, so it claims no click.
fn commands(item: &CanvasItem) -> bool {
    keymap::dispatches(&item.action) && !matches!(item.content, Content::Grid { .. })
}

fn has_commands(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= commands(item));
    found
}

impl CanvasPanel {
    /// Over every item, off design mode, in a canvas that has a command or
    /// a render target somewhere. It hit tests against what the last render
    /// placed, and hands a target the pointer only where [`hit`] says so.
    fn click_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity().downgrade();
        let me_id = cx.entity_id();
        let pointing = self.pointing.clone();

        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                if pointing.get() {
                    window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                }

                let (hover, moving) = (hitbox.clone(), me.clone());
                let pointing = pointing.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }

                    // Only a crossing re-renders, for the cursor.
                    let local = local_point(event.position, bounds.origin);
                    let hovered = hover.is_hovered(window);
                    let over = moving
                        .update(cx, |this, _| {
                            let hit = this.hit_over(hovered, local);
                            this.point_at(&hit, Pointer::Move);
                            matches!(hit, Hit::Command { .. })
                        })
                        .unwrap_or(false);
                    if over != pointing.get() {
                        pointing.set(over);
                        cx.notify(me_id);
                    }
                });

                let (hover, down) = (hitbox.clone(), me.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !hover.is_hovered(window) {
                        return;
                    }

                    let local = local_point(event.position, bounds.origin);
                    let button = event.button;
                    down.update(cx, |this, cx| {
                        let hit = this.hit_at(local);
                        this.point_at(&hit, |at| Pointer::Down(at, button_name(button)));
                        if button != MouseButton::Left {
                            return;
                        }

                        this.pressed = match hit {
                            Hit::Command { id, .. } => {
                                cx.stop_propagation();
                                Some(id)
                            }
                            _ => None,
                        };
                    })
                    .ok();
                });

                let (hover, wheel) = (hitbox.clone(), me.clone());
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if !phase.bubble() || !hover.is_hovered(window) {
                        return;
                    }

                    let local = local_point(event.position, bounds.origin);
                    let lines = scroll_lines(event.delta);
                    wheel
                        .update(cx, |this, _| {
                            let hit = this.hit_at(local);
                            this.point_at(&hit, |at| Pointer::Scroll(at, lines));
                        })
                        .ok();
                });

                let up = me.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }

                    let local = local_point(event.position, bounds.origin);
                    let over = hitbox.is_hovered(window);
                    let button = event.button;
                    up.update(cx, |this, cx| {
                        let hit = this.hit_over(over, local);
                        this.point_at(&hit, |at| Pointer::Up(at, button_name(button)));
                        if button != MouseButton::Left {
                            return;
                        }

                        let pressed = this.pressed.take();
                        if !over {
                            return;
                        }

                        // The id comes from a workspace file that may not be
                        // the user's own. It only ever goes to the fixed
                        // command list, which ignores an id it doesn't have.
                        if let Hit::Command { id, action } = this.hit_at(local)
                            && pressed.as_deref() == Some(id.as_str())
                        {
                            cx.stop_propagation();
                            keymap::dispatch(&action, window, cx);

                            // A setting's own repaint skips the window the
                            // command came from, so a canvas showing that
                            // state looks again once the flip has landed.
                            if states::follows(&this.config.canvas) {
                                cx.defer_in(window, |this, window, cx| {
                                    this.recheck_states(window, cx)
                                });
                            }
                        }
                    })
                    .ok();
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    fn hit_at(&self, local: [f32; 2]) -> Hit {
        hit(&self.shapes, &self.config.canvas, local)
    }

    /// [`hit_at`](Self::hit_at), or nothing while the canvas isn't the
    /// hovered thing.
    fn hit_over(&self, hovered: bool, local: [f32; 2]) -> Hit {
        if hovered {
            self.hit_at(local)
        } else {
            Hit::Nothing
        }
    }

    /// Sends the target under the pointer its event, and a leave to the one
    /// it just left. Nothing else of the pointer goes to a renderer.
    fn point_at(&mut self, hit: &Hit, pointer: impl FnOnce([f32; 2]) -> Pointer) {
        let over = match hit {
            Hit::Target { id, at } => Some((id.as_str(), *at)),
            _ => None,
        };

        if self.hovered.as_deref() != over.map(|(id, _)| id) {
            if let Some(left) = self.hovered.take() {
                self.targets.point(&left, Pointer::Leave);
            }
            self.hovered = over.map(|(id, _)| id.to_string());
        }

        if let Some((id, at)) = over {
            self.targets.point(id, pointer(at));
        }
    }

    /// Why a text item's pattern doesn't parse, if it doesn't.
    pub(crate) fn text_error(&self, id: &str) -> Option<String> {
        text::line_of(&self.config.canvas.items, &self.bodies, id)
            .and_then(text::Line::error)
            .map(str::to_string)
    }
}

enum Begin {
    Move,
    Resize(Edges),
    /// The axis: 0 for x, 1 for y, 2 for z.
    Turn(usize),
}

/// Where the turn handles sit on a projected quad.
struct TurnHandles {
    center: [f32; 2],
    ring: f32,
    /// The grip on the right edge, which turns about y.
    right: [f32; 2],
    /// The grip on the top edge, which turns about x.
    top: [f32; 2],
}

impl TurnHandles {
    fn of(shape: &Shape) -> TurnHandles {
        let c = &shape.corners;
        let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
        let center = shape.center();
        let right = mid(c[1], c[2]);
        let top = mid(c[0], c[1]);

        let reach = (right[0] - center[0])
            .hypot(right[1] - center[1])
            .min((top[0] - center[0]).hypot(top[1] - center[1]));

        TurnHandles {
            center,
            ring: (reach * 0.5).clamp(14., 64.),
            right,
            top,
        }
    }
}

/// Whether `at` is in the band along the canvas's side where the edge tab
/// shows: near the edge and around the middle.
fn edge_band(bounds: Bounds<Pixels>, at: Point<Pixels>, left: bool) -> bool {
    let [x, y] = local_point(at, bounds.origin);
    let width = f32::from(bounds.size.width);
    let middle = f32::from(bounds.size.height) / 2.;

    let near = if left {
        x <= EDGE_REACH
    } else {
        x >= width - EDGE_REACH
    };
    near && (y - middle).abs() <= EDGE_BAND / 2.
}

fn local_point(position: Point<Pixels>, origin: Point<Pixels>) -> [f32; 2] {
    [
        f32::from(position.x - origin.x),
        f32::from(position.y - origin.y),
    ]
}

fn at_origin(p: [f32; 2], origin: Point<Pixels>) -> Point<Pixels> {
    point(origin.x + px(p[0]), origin.y + px(p[1]))
}

fn stroke(points: &[Point<Pixels>], width: f32, color: Rgba, window: &mut Window) {
    let mut path = PathBuilder::stroke(px(width));
    path.add_polygon(points, true);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn disc(center: Point<Pixels>, radius: f32, color: Rgba, window: &mut Window) {
    let points: Vec<Point<Pixels>> = (0..16)
        .map(|ix| {
            let a = ix as f32 / 16. * std::f32::consts::TAU;
            point(
                center.x + px(radius * a.cos()),
                center.y + px(radius * a.sin()),
            )
        })
        .collect();

    let mut path = PathBuilder::fill();
    path.add_polygon(&points, true);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_outline(shape: &Shape, origin: Point<Pixels>, window: &mut Window) {
    let color = if shape.locked {
        palette::text_faint()
    } else {
        palette::accent()
    };
    let points = shape.corners.map(|p| at_origin(p, origin));
    stroke(&points, 1.5, color, window);
}

fn paint_turn_handles(shape: &Shape, origin: Point<Pixels>, window: &mut Window) {
    let handles = TurnHandles::of(shape);
    let accent = palette::accent();

    let ring: Vec<Point<Pixels>> = (0..48)
        .map(|ix| {
            let a = ix as f32 / 48. * std::f32::consts::TAU;
            at_origin(
                [
                    handles.center[0] + handles.ring * a.cos(),
                    handles.center[1] + handles.ring * a.sin(),
                ],
                origin,
            )
        })
        .collect();
    stroke(&ring, 2., accent, window);

    for grip in [handles.right, handles.top] {
        disc(at_origin(grip, origin), 6., palette::bg_panel(), window);
        disc(at_origin(grip, origin), 4.5, accent, window);
    }
}

/// A new render target's resolution, and its size on the stage.
const TARGET_PIXELS: [u32; 2] = [1280, 720];
const TARGET_UNITS: [f32; 2] = [640., 360.];

/// Each installed renderer's owner and label, for naming a new target. An
/// empty list hides the Add Render Target row.
pub(crate) fn target_choices(
    renderers: &[rox_services::renderers::Renderer],
) -> Vec<(String, String)> {
    renderers
        .iter()
        .map(|renderer| {
            (
                target::owner_of(&renderer.id),
                target::renderer_label(renderer),
            )
        })
        .collect()
}

/// The Renderers settings page's nav key.
pub(crate) const RENDERERS_PAGE: &str = "settings-page-renderers";

/// Whether the renderer `owner` names suggests resting with the music: a new
/// target's starting value, which the user's setting replaces.
pub(crate) fn suggests_rest(owner: &str) -> bool {
    target::renderer_id(owner).is_some_and(|id| {
        rox_services::renderers::all()
            .iter()
            .any(|renderer| renderer.id == id && renderer.rests)
    })
}

/// The image assets in the store, id and file, for the Add Image list.
pub(crate) fn image_assets() -> Vec<(String, String)> {
    assets::all()
        .into_iter()
        .filter(|asset| asset.kind == AssetKind::Image)
        .map(|asset| (asset.id, asset.file))
        .collect()
}

fn is_object_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(rox_core::settings::roxo::EXT))
}

/// Copies a picked file into the store's staging folder. Only images, so
/// a font or a clip can't come in through here.
async fn stage_image(path: PathBuf) -> Result<assets::Staged, String> {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if assets::kind_for(&file) != Some(AssetKind::Image) {
        return Err(rox_i18n::t!("canvas-add-image-not-image").to_string());
    }

    assets::stage(&path, u64::MAX)
}

/// A new grid's dock: one empty tab group in a split root, the shape the
/// grid's restore gates on.
fn empty_dock() -> Value {
    let tabs = PanelState {
        panel_name: "TabPanel".to_string(),
        children: Vec::new(),
        info: PanelInfo::tabs(0),
    };
    let root = PanelState {
        panel_name: "StackPanel".to_string(),
        children: vec![tabs],
        info: PanelInfo::stack(Vec::new(), Axis::Vertical),
    };

    serde_json::to_value(DockAreaState {
        version: Some(LAYOUT_VERSION),
        center: root,
        ..DockAreaState::default()
    })
    .unwrap_or(Value::Null)
}

impl PanelSettings for CanvasPanel {
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
}

/// The value edit is the inspector's, so a route slider and a place slider
/// never type at once.
impl SignalHost for CanvasPanel {
    fn hub(&self) -> &Arc<SignalHub> {
        &self.state.signals
    }

    fn routes(&self) -> &[Route] {
        &self.config.routes
    }

    fn signal_ui(&self) -> &SignalUi {
        &self.signal_ui
    }

    fn signal_ui_mut(&mut self) -> &mut SignalUi {
        &mut self.signal_ui
    }

    fn value_edit(&self) -> &ValueEdit {
        self.inspector.value_edit()
    }
}

impl RouteHost for CanvasPanel {
    fn routes_mut(&mut self) -> &mut Vec<Route> {
        &mut self.config.routes
    }

    fn span_lines(&self, target: &str) -> Option<(SharedString, SharedString)> {
        routes::sways(target).then(|| {
            (
                rox_i18n::t!("canvas-route-sway-quiet"),
                rox_i18n::t!("canvas-route-sway-loud"),
            )
        })
    }
}

impl EventEmitter<PanelEvent> for CanvasPanel {}

impl Focusable for CanvasPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CanvasPanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        panel_settings::open(cx.entity(), cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("canvas-panel-title"),
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

    // The grids' own tab groups serve their panels' right-click.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// A drop over a grid is the grid's panel's to take, which the workspace
    /// can't see past the canvas.
    fn accepts_drop(&self, cx: &App) -> bool {
        let Some(at) = self.pointer.get() else {
            return false;
        };

        self.grids()
            .iter()
            .any(|area| area.read(cx).accepts_drop_at(at, cx))
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, cx: &App) -> PanelState {
        state_of(&self.saved(cx))
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.menu = None;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;

        // Pop Out and Dock Back run from this menu, and a new window draws
        // the canvas before the click returns: a menu still held here would
        // render mid update.
        self.menu = None;

        self.images.clear(window);

        // A Pop Out takes the targets along: a session on shared memory
        // carries on, and one lent a GPU texture starts again with one made
        // over there, since its texture belongs to this window.
        match rox_dock::panel_moving() {
            true => self.targets.carry(window, cx),
            false => self.targets.clear(window, cx),
        }

        for view in self.textures.values() {
            view.update(cx, |view, cx| view.release(window, cx));
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        panel_tail(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
            cx,
        )
    }
}

/// The canvas's panel menu, which its own right-click menus end with: the
/// canvas owns its right-click, so this is the only way to Dock Back from a
/// pop-out. It reads nothing of the canvas, so it builds mid update.
fn panel_tail(
    menu: PopupMenu,
    panel: &Entity<CanvasPanel>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let menu = panel_settings::rename_item(menu, panel, tab_panel.clone(), window, cx);
    let menu = panel_settings::settings_item(menu, panel, cx);
    panel::popout_item(menu, panel, tab_panel, state, window)
}

impl Render for CanvasPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.images.follow_store(window);
        self.window = Some(window.window_handle());
        self.targets.arrive(window);

        // Off design mode a canvas is plain content: no drag carries over and
        // no menu stays open.
        let design = !self.config.chrome.controls_hidden();
        if design != self.design {
            self.design = design;
            self.pointing.set(false);
            self.pressed = None;
            if let Some(id) = self.hovered.take() {
                self.targets.point(&id, Pointer::Leave);
            }
            if !design {
                self.gesture = None;
                self.menu = None;
            }
        }

        self.absorb_route_edits(window, cx);
        if self.focus.contains_focused(window, cx) {
            objects::note_focused(cx.entity_id());
        }

        let chrome = self.config.chrome.clone();
        let body = self.body(window, cx);
        panel::themed(&chrome, move || body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layout() -> Value {
        json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [{
                    "panel_name": "TabPanel",
                    "children": [{
                        "panel_name": "library",
                        "children": [],
                        "info": { "panel": { "locked": true, "columns": [1.5, 2.25] } },
                    }],
                    "info": { "tabs": { "active_index": 0 } },
                }],
                "info": { "stack": { "sizes": [480.0], "axis": 1 } },
            },
        })
    }

    #[test]
    fn the_add_row_hides_with_no_renderer_installed() {
        assert!(target_choices(&[]).is_empty());

        let bars = rox_services::renderers::Renderer {
            id: "bars".into(),
            name: "Bars Deluxe".into(),
            label: "Bars".into(),
            approved: false,
            enabled: false,
            signals: Vec::new(),
            rests: false,
        };
        let unlabelled = rox_services::renderers::Renderer {
            id: "waves".into(),
            name: "Waves".into(),
            label: " ".into(),
            ..bars.clone()
        };

        assert_eq!(
            target_choices(&[bars, unlabelled]),
            [
                ("plugin:bars".to_string(), "Bars".to_string()),
                ("plugin:waves".to_string(), "Waves".to_string()),
            ],
            "listed whether or not it's switched on, under the owner a target saves"
        );
    }

    #[test]
    fn converting_wraps_the_layout_and_loses_nothing() {
        let wrapped = serde_json::to_value(wrap_layout(layout(), 1)).expect("serializes");

        assert_eq!(wrapped["version"], 1);
        assert!(wrapped["center"]["info"]["stack"].is_object());

        let tabs = &wrapped["center"]["children"];
        assert_eq!(tabs.as_array().map(Vec::len), Some(1));
        let canvas = &tabs[0]["children"];
        assert_eq!(canvas.as_array().map(Vec::len), Some(1));
        assert_eq!(canvas[0]["panel_name"], PANEL_NAME);

        // Read back the way the registry hands it to the panel.
        let info: PanelInfo =
            serde_json::from_value(canvas[0]["info"].clone()).expect("info parses");
        let config: CanvasConfig = panel::config_from_info(&info);
        assert_eq!(config.canvas.items.len(), 1);

        let item = &config.canvas.items[0];
        assert_eq!(item.place.anchor, Anchor::Center);
        assert_eq!(item.place.size, [Length::Stage(1.), Length::Stage(1.)]);
        // Everything but the lock.
        let mut unlocked = layout();
        unlocked["center"]["children"][0]["children"][0]["info"]["panel"]
            .as_object_mut()
            .expect("a config")
            .remove("locked");
        match &item.content {
            Content::Grid { dock } => assert_eq!(dock, &unlocked),
            _ => panic!("the one item is a grid"),
        }
    }

    #[test]
    fn converting_unlocks_the_layouts_panels_and_nothing_inside_them() {
        let mut dump = layout();
        let library = &mut dump["center"]["children"][0]["children"][0]["info"]["panel"];
        library["max_height"] = json!(24.0);
        library["items"] = json!([{ "id": "logo", "locked": true }]);

        let wrapped = serde_json::to_value(wrap_layout(dump, 1)).expect("serializes");
        let info: PanelInfo =
            serde_json::from_value(wrapped["center"]["children"][0]["children"][0]["info"].clone())
                .expect("info parses");
        let config: CanvasConfig = panel::config_from_info(&info);
        let Content::Grid { dock } = &config.canvas.items[0].content else {
            panic!("the one item is a grid");
        };

        let library = &dock["center"]["children"][0]["children"][0]["info"]["panel"];
        assert!(library.get("locked").is_none());
        assert_eq!(library["max_height"], 24.0);
        assert_eq!(library["items"][0]["locked"], true);
    }

    #[test]
    fn a_canvas_dumps_no_children() {
        let state = wrap_layout(layout(), 1).center.children[0].children[0].clone();
        assert_eq!(state.panel_name, PANEL_NAME);
        assert!(state.children.is_empty());

        let round = state_of(&panel::config_from_info(&state.info));
        assert!(round.children.is_empty());
        assert_eq!(
            serde_json::to_value(&round.info).ok(),
            serde_json::to_value(&state.info).ok()
        );
    }

    #[test]
    fn a_grid_item_with_a_stale_dump_is_kept_as_it_was() {
        let stale = [
            json!({ "version": 0, "center": layout()["center"].clone() }),
            json!({ "version": 1, "center": { "panel_name": "TabPanel", "children": [], "info": { "tabs": { "active_index": 0 } } } }),
            json!("not a dump"),
        ];

        for dock in stale {
            assert!(grid::usable(&dock).is_none(), "{dock}");

            let item =
                json!({ "id": "old", "name": "Kept", "content": { "kind": "grid", "dock": dock } });
            let mut state: CanvasState =
                serde_json::from_value(json!({ "items": [item.clone()] })).expect("parses");
            let bodies = canvas::build_bodies(&state.items, 0, &mut |_, dock, _| {
                grid::usable(dock).map(|_| ())
            });
            assert!(matches!(bodies[0], Body::Unknown));

            canvas::fold_dumps(&mut state.items, &bodies, &|_| Some(json!("fresh")));
            let saved = serde_json::to_value(&state.items[0]).expect("serializes");
            assert_eq!(saved["content"], item["content"]);
            assert_eq!(saved["name"], "Kept");
        }

        assert!(grid::usable(&layout()).is_some());
    }

    #[test]
    fn an_empty_grid_dump_still_builds_a_grid() {
        // The dump of a grid whose last tab left, and a bare root with no
        // group at all.
        let kept = json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [{
                    "panel_name": "TabPanel",
                    "children": [],
                    "info": { "tabs": { "active_index": 0 } },
                }],
                "info": { "stack": { "sizes": [480.0], "axis": 1 } },
            },
        });
        let bare = json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [],
                "info": { "stack": { "sizes": [], "axis": 1 } },
            },
        });

        for dock in [kept, bare] {
            assert!(grid::usable(&dock).is_some(), "{dock}");

            let item = json!({ "id": "empty", "content": { "kind": "grid", "dock": dock } });
            let state: CanvasState =
                serde_json::from_value(json!({ "items": [item] })).expect("parses");
            let bodies = canvas::build_bodies(&state.items, 0, &mut |_, dock, _| {
                grid::usable(dock).map(|_| ())
            });
            assert!(matches!(bodies[0], Body::Grid(())), "{dock}");
        }
    }

    fn square(x: f32, y: f32, side: f32) -> [[f32; 2]; 4] {
        [[x, y], [x + side, y], [x + side, y + side], [x, y + side]]
    }

    fn shape(id: &str, groups: &[&str], corners: [[f32; 2]; 4], kind: ShapeKind) -> Shape {
        Shape {
            id: id.to_string(),
            groups: groups.iter().map(|g| g.to_string()).collect(),
            corners,
            label: SharedString::from(id.to_string()),
            kind,
            locked: false,
            hidden: false,
        }
    }

    fn picture(id: &str, action: &str) -> Value {
        json!({ "id": id, "action": action, "content": { "kind": "image", "asset": "a" } })
    }

    fn canvas_of(items: Value) -> CanvasState {
        serde_json::from_value(json!({ "items": items })).expect("a canvas parses")
    }

    fn command(id: &str, action: &str) -> Hit {
        Hit::Command {
            id: id.to_string(),
            action: action.to_string(),
        }
    }

    #[test]
    fn a_click_goes_to_the_front_item() {
        let canvas = canvas_of(json!([
            picture("back", "stop_playback"),
            picture("front", "toggle_playback")
        ]));
        let shapes = [
            shape("back", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape("front", &[], square(50., 50., 100.), ShapeKind::Perspective),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [75., 75.]),
            command("front", "toggle_playback")
        );
        assert_eq!(
            hit(&shapes, &canvas, [10., 10.]),
            command("back", "stop_playback")
        );
        assert_eq!(hit(&shapes, &canvas, [300., 300.]), Hit::Nothing);
    }

    #[test]
    fn a_grid_stops_the_walk() {
        let canvas = canvas_of(json!([
            picture("back", "toggle_playback"),
            { "id": "grid", "action": "stop_playback", "content": { "kind": "grid", "dock": null } },
        ]));
        let shapes = [
            shape("back", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape("grid", &[], square(20., 20., 40.), ShapeKind::Grid),
        ];

        assert_eq!(hit(&shapes, &canvas, [30., 30.]), Hit::Grid);
        assert_eq!(
            hit(&shapes, &canvas, [90., 90.]),
            command("back", "toggle_playback")
        );
        assert!(!has_commands(&canvas_of(json!([
            { "id": "grid", "action": "stop_playback", "content": { "kind": "grid", "dock": null } },
        ]))));
    }

    #[test]
    fn a_groups_command_covers_a_child_without_one() {
        let canvas = canvas_of(json!([{
            "id": "outer",
            "action": "next_track",
            "content": { "kind": "group", "items": [{
                "id": "inner",
                "content": { "kind": "group", "items": [picture("a", "")] },
            }] },
        }]));
        let shapes = [
            shape(
                "a",
                &["outer", "inner"],
                square(0., 0., 50.),
                ShapeKind::Perspective,
            ),
            shape("inner", &["outer"], square(0., 0., 50.), ShapeKind::Group),
            shape("outer", &[], square(0., 0., 50.), ShapeKind::Group),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [10., 10.]),
            command("outer", "next_track")
        );
    }

    #[test]
    fn a_childs_own_command_wins() {
        let canvas = canvas_of(json!([{
            "id": "group",
            "action": "next_track",
            "content": { "kind": "group", "items": [picture("a", "stop_playback"), picture("b", "")] },
        }]));
        let shapes = [
            shape("a", &["group"], square(0., 0., 50.), ShapeKind::Perspective),
            shape("b", &["group"], square(60., 0., 50.), ShapeKind::Flat),
            shape("group", &[], square(0., 0., 110.), ShapeKind::Group),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [10., 10.]),
            command("a", "stop_playback")
        );
        assert_eq!(
            hit(&shapes, &canvas, [70., 10.]),
            command("group", "next_track")
        );
        // Inside the group's box but on none of its children.
        assert_eq!(hit(&shapes, &canvas, [55., 80.]), Hit::Nothing);
    }

    #[test]
    fn an_invisible_item_is_passed_through() {
        let mut canvas = canvas_of(json!([
            picture("back", "stop_playback"),
            picture("front", "toggle_playback")
        ]));
        if let Some(front) = canvas.find_mut("front") {
            front.visible = false;
        }
        let shapes = [
            shape("back", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape("front", &[], square(0., 0., 100.), ShapeKind::Perspective),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [50., 50.]),
            command("back", "stop_playback")
        );
    }

    #[test]
    fn an_item_hidden_by_its_case_is_passed_through() {
        let canvas = canvas_of(json!([
            picture("back", "stop_playback"),
            {
                "id": "group",
                "action": "next_track",
                "state": "player.mute",
                "cases": [{ "when": "unmuted", "visible": false }],
                "content": { "kind": "group", "items": [picture("child", "toggle_playback")] },
            },
        ]));
        let mut shapes = [
            shape("back", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape("child", &["group"], square(0., 0., 100.), ShapeKind::Flat),
            shape("group", &[], square(0., 0., 100.), ShapeKind::Group),
        ];

        // Shown, the group's child takes the click with its own command.
        assert_eq!(
            hit(&shapes, &canvas, [50., 50.]),
            command("child", "toggle_playback")
        );

        // Hidden by the group's case, the ghosts pass the click through.
        for shape in &mut shapes[1..] {
            shape.hidden = true;
        }
        assert_eq!(
            hit(&shapes, &canvas, [50., 50.]),
            command("back", "stop_playback")
        );
    }

    fn render_target(id: &str, action: &str) -> Value {
        json!({
            "id": id,
            "action": action,
            "content": { "kind": "target", "owner": "plugin:bars", "width": 64, "height": 64 },
        })
    }

    fn on_target(id: &str, at: [f32; 2]) -> Hit {
        Hit::Target {
            id: id.to_string(),
            at,
        }
    }

    #[test]
    fn a_target_with_a_command_keeps_the_click() {
        let canvas = canvas_of(json!([
            render_target("plain", ""),
            render_target("commanded", "toggle_playback"),
            {
                "id": "group",
                "action": "next_track",
                "content": { "kind": "group", "items": [render_target("held", "")] },
            },
        ]));
        let shapes = [
            shape("plain", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape(
                "commanded",
                &[],
                square(200., 0., 100.),
                ShapeKind::Perspective,
            ),
            shape(
                "held",
                &["group"],
                square(400., 0., 100.),
                ShapeKind::Perspective,
            ),
            shape("group", &[], square(400., 0., 100.), ShapeKind::Group),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [25., 75.]),
            on_target("plain", [0.25, 0.75])
        );
        assert_eq!(
            hit(&shapes, &canvas, [250., 50.]),
            command("commanded", "toggle_playback")
        );
        assert_eq!(
            hit(&shapes, &canvas, [450., 50.]),
            command("group", "next_track"),
            "a group's command covers the target inside it"
        );

        // A tilted target's point comes back through its quad.
        let tilted = [[0., 0.], [100., 20.], [100., 80.], [0., 100.]];
        let shapes = [shape("plain", &[], tilted, ShapeKind::Perspective)];
        let Hit::Target { at, .. } = hit(&shapes, &canvas, [1., 50.]) else {
            panic!("the target is under the point");
        };
        assert!(at[0] < 0.05 && (at[1] - 0.5).abs() < 0.01, "{at:?}");
    }

    #[test]
    fn an_unknown_command_claims_no_click() {
        let canvas = canvas_of(json!([
            render_target("target", "not_a_command"),
            picture("picture", "not_a_command"),
            {
                "id": "group",
                "action": "not_a_command",
                "content": { "kind": "group", "items": [picture("inner", "next_track")] },
            },
        ]));
        let shapes = [
            shape("target", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape(
                "picture",
                &[],
                square(200., 0., 100.),
                ShapeKind::Perspective,
            ),
            shape(
                "inner",
                &["group"],
                square(400., 0., 100.),
                ShapeKind::Perspective,
            ),
            shape("group", &[], square(400., 0., 100.), ShapeKind::Group),
        ];

        assert_eq!(
            hit(&shapes, &canvas, [50., 50.]),
            on_target("target", [0.5, 0.5]),
            "the target gets the pointer"
        );
        assert_eq!(hit(&shapes, &canvas, [250., 50.]), Hit::Nothing);
        assert_eq!(
            hit(&shapes, &canvas, [450., 50.]),
            command("inner", "next_track"),
        );
        assert!(!has_commands(&canvas_of(json!([picture(
            "a",
            "not_a_command"
        )]))));

        // A command handled on a panel can't run from a click either.
        assert!(!has_commands(&canvas_of(json!([picture(
            "a",
            "canvas_undo"
        )]))));
    }

    #[test]
    fn a_target_behind_another_item_gets_no_pointer() {
        let canvas = canvas_of(json!([render_target("back", ""), picture("front", "")]));
        let shapes = [
            shape("back", &[], square(0., 0., 100.), ShapeKind::Perspective),
            shape("front", &[], square(50., 50., 100.), ShapeKind::Perspective),
        ];

        assert_eq!(hit(&shapes, &canvas, [75., 75.]), Hit::Nothing);
        assert_eq!(
            hit(&shapes, &canvas, [10., 20.]),
            on_target("back", [0.1, 0.2])
        );
        assert_eq!(hit(&shapes, &canvas, [300., 300.]), Hit::Nothing);

        // Hidden, the front item passes the pointer to the target behind.
        let mut shapes = shapes;
        shapes[1].hidden = true;
        assert_eq!(
            hit(&shapes, &canvas, [75., 75.]),
            on_target("back", [0.75, 0.75])
        );
    }

    #[test]
    fn the_config_reads_routes_and_chrome_beside_the_canvas() {
        let config: CanvasConfig = serde_json::from_value(json!({
            "title": "Stage",
            "camera": { "fov": 0, "parallax": 12 },
            "routes": [{ "signal": 3, "target": "grid.opacity" }],
            "items": [],
        }))
        .expect("parses");

        assert_eq!(config.chrome.title.as_deref(), Some("Stage"));
        assert_eq!(config.canvas.camera.parallax, 12.);
        assert_eq!(config.routes[0].target, "grid.opacity");

        let empty = serde_json::to_value(CanvasConfig::default()).expect("serializes");
        assert!(empty.get("routes").is_none());
    }
}
