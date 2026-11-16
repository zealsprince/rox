//! The scene panel (ADR 33): a composite that places items in 3D space. Live
//! UI goes in as grid items, each a dock tree of its own, so a scene holds
//! real tab groups that drag, zoom and pop out like any other. Images take
//! real perspective; grids stay flat to the screen and take their place and
//! depth order from the projection.
//!
//! A scene is only ever a panel. The window's root stays the dock, and a
//! full-window scene is a dock holding one scene panel. Its saved state is
//! its config, nested dock dumps included, and `PanelState.children` stays
//! empty.
//!
//! In design mode the panel is also the scene's editor: it picks, moves,
//! resizes and turns items with handles drawn on the projection, keeps the
//! undo history, and hosts the inspector ([`crate::scene::overlay`]). The
//! arithmetic behind every gesture is in [`crate::scene::edit`]. Off design
//! mode none of that shows and a scene is plain content.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, AnyWindowHandle, App, Axis, Bounds, Context, CursorStyle, DismissEvent, Div,
    Entity, EntityId, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, PathPromptOptions, Pixels, Point, Rgba,
    SharedString, Subscription, WeakEntity, Window, actions, anchored, canvas, deferred, div,
    point, prelude::*, px,
};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_core::settings::AssetKind;
use rox_core::settings::assets;
use rox_dock::{
    DockArea, DockAreaState, Panel, PanelEvent, PanelInfo, PanelState, PanelView, TabPanel,
};
use rox_scene::{
    Anchor, Camera, Content, Frame, ImageFit, Length, Live, Place, SceneItem, SceneState, Viewport,
    child_frame, inside, paint_order, project,
};
use rox_viz::signal::Route;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::composite;
use crate::scene::edit::{self, Edges, History};
use crate::scene::grid::{self, Grid};
use crate::scene::image::{self, Images};
use crate::scene::overlay::Inspector;
use crate::scene::{self, Body, Placing};
use crate::workspace::{LAYOUT_VERSION, Workspace};
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette;
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings, Tone};
use rox_panel_api::panel_settings;
use rox_panel_api::toast::Toast;
use rox_panel_api::windows::WorkspaceWindows;

/// The registry name.
pub const PANEL_NAME: &str = "scene";

actions!(
    scene,
    [
        Undo,
        Redo,
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

/// The key context a scene sets while its own focus holds in design mode.
/// Panels inside its grids hold their own focus, so the scene's keys never
/// reach past them.
pub const KEY_CONTEXT: &str = "Scene";

/// Added beside [`KEY_CONTEXT`] while an item is selected.
pub const SELECTED_CONTEXT: &str = "SceneSelected";

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

/// How near the scene's side the pointer comes before the inspector's tab
/// shows, and how tall the band it shows in, both in pixels.
const EDGE_REACH: f32 = 28.;
const EDGE_BAND: f32 = 120.;

thread_local! {
    /// The scene whose selection and inspector show. Only one does at a
    /// time, so a scene nested in another doesn't stack a second inspector
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
        let (origin, extent) = scene::bbox(&self.corners);
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

/// A drag in progress. It edits the scene live and lands in the history as
/// one step on release.
struct Gesture {
    id: String,
    before: SceneState,
    /// The item as it stood when the press landed.
    item: SceneItem,
    parent: Frame,
    camera: Camera,
    viewport: Viewport,
    from: [f32; 2],
    start_box: [f32; 4],
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
}

/// `shader` and `owner` are the dump walkers' keys everywhere, and the
/// chrome already holds both. Nothing else in here may use them.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(flatten)]
    pub scene: SceneState,
    /// Targets are `<item id>.<property>`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
}

/// The dock state a scene saves as: its config, with no children.
pub(crate) fn state_of(config: &SceneConfig) -> PanelState {
    PanelState {
        panel_name: PANEL_NAME.to_string(),
        children: Vec::new(),
        info: PanelInfo::panel(serde_json::to_value(config).unwrap_or(Value::Null)),
    }
}

/// A layout as a scene: one grid holding `dump` across the whole stage, the
/// one panel in the one tab group of a fresh root. `dump` goes in untouched,
/// so the window looks the same once it's applied.
pub(crate) fn wrap_layout(dump: Value, version: usize) -> DockAreaState {
    let grid = SceneItem {
        id: "grid".to_string(),
        place: Place {
            anchor: Anchor::Center,
            size: [Length::Stage(1.), Length::Stage(1.)],
            ..Place::default()
        },
        content: Content::Grid { dock: dump },
        ..SceneItem::default()
    };
    let config = SceneConfig {
        scene: SceneState {
            items: vec![grid],
            ..SceneState::default()
        },
        ..SceneConfig::default()
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

pub struct ScenePanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    pub(crate) config: SceneConfig,
    /// At the same indices as `config.scene.items`.
    bodies: Vec<Body<Grid>>,
    /// Grids built ahead of the edit that names them.
    spare: HashMap<String, Grid>,
    /// Scenes and groups this scene sits inside.
    level: usize,
    images: Images,
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
    history: History,
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
}

impl ScenePanel {
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: SceneConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = ScenePanel {
            state,
            workspace,
            config,
            bodies: Vec::new(),
            spare: HashMap::new(),
            level: scene::level_here(),
            images: Images::default(),
            bounds: Rc::default(),
            settle: Rc::default(),
            pointer: Rc::default(),
            near_edge: Rc::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            selected: None,
            history: History::default(),
            gesture: None,
            last_press: None,
            shapes: Vec::new(),
            design: settings::design_mode(),
            window: None,
            inspector: Inspector::default(),
            menu: None,
        };
        this.build(HashMap::new(), window, cx);
        this
    }

    /// `settings::set_design_mode` refreshes each window through
    /// `window.update`, which can't reach the window the toggle was
    /// dispatched in, and this panel renders cached. So the flip is checked
    /// here.
    /// Every grid item's dock, groups included, in item order.
    pub fn grids(&self) -> Vec<Entity<DockArea>> {
        let mut out = Vec::new();
        scene::each_grid(&self.config.scene.items, &self.bodies, &mut |_, grid| {
            out.push(grid.area.clone())
        });
        out
    }

    /// What the layout tree calls each of [`grids`](Self::grids), in the
    /// same order: the item's name, or its id when it has none.
    pub(crate) fn grid_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        scene::each_grid(&self.config.scene.items, &self.bodies, &mut |item, _| {
            out.push(if item.name.is_empty() {
                item.id.clone()
            } else {
                item.name.clone()
            })
        });
        out
    }

    /// A new grid item holding `panel`, placed at the stage center.
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.config.scene.fresh_id("grid");
        let grid = grid::holding(&id, panel, &self.state, window, cx);
        let dock = grid::dump(&grid, cx).unwrap_or(Value::Null);
        self.spare.insert(id.clone(), grid);

        let item = SceneItem {
            id,
            content: Content::Grid { dock },
            ..SceneItem::default()
        };
        self.change(move |scene| scene.items.push(item), window, cx);
    }

    /// The one path every edit takes: applies it, records undo, asks the
    /// workspace to save. Live grids are kept across it by item id.
    pub(crate) fn change(
        &mut self,
        edit: impl FnOnce(&mut SceneState),
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
        edit: impl FnOnce(&mut SceneState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The edit sees every grid as it stands now, not as it was loaded.
        self.config = self.saved(cx);
        let before = self.config.scene.clone();

        self.rebuild_with(edit, window, cx);
        self.history
            .record(before, self.config.scene.clone(), key, Instant::now());
    }

    /// For controls whose callbacks aren't handed the window: the edit runs
    /// on the next turn, through the same path.
    pub(crate) fn change_later(
        &mut self,
        key: &'static str,
        edit: impl FnOnce(&mut SceneState) + 'static,
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
                        this.change_keyed(Some(key), edit, window, cx)
                    })
                    .ok();
                })
                .ok();
        });
    }

    /// Applies an edit and rebuilds, without touching the history.
    fn rebuild_with(
        &mut self,
        edit: impl FnOnce(&mut SceneState),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut keep = std::mem::take(&mut self.spare);
        scene::take_grids(
            &self.config.scene.items,
            std::mem::take(&mut self.bodies),
            &mut keep,
        );

        edit(&mut self.config.scene);
        self.build(keep, window, cx);

        if let Some(id) = &self.selected
            && self.config.scene.find(id).is_none()
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
            self.rebuild_with(move |scene| *scene = state, window, cx);
        }
    }

    pub(crate) fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gesture = None;
        self.config = self.saved(cx);

        if let Some(state) = self.history.redo() {
            self.rebuild_with(move |scene| *scene = state, window, cx);
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

    /// Ask the workspace for its debounced layout save. Only from the
    /// workspace's own window: a popped-out scene isn't in its layout, and
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
        self.bodies = scene::build_bodies(
            &self.config.scene.items,
            self.level,
            &mut |item, dock, level| match keep.remove(&item.id) {
                Some(grid) => Some(grid),
                None => grid::restore(&item.id, dock, level, &state, window, cx),
            },
        );
    }

    /// The config with every live grid's dump folded in.
    fn saved(&self, cx: &App) -> SceneConfig {
        let mut config = self.config.clone();
        scene::fold_dumps(&mut config.scene.items, &self.bodies, &|grid: &Grid| {
            grid::dump(grid, cx)
        });
        config
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let bounds = self.bounds.get();
        let camera = self.config.scene.camera;
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
        let stage = Frame::stage(&self.config.scene.stage, at.viewport);

        // Before the first bounds land there's nothing to place against, and
        // a grid laid out at no size would lay its panels out at none.
        let mut draw = Draw {
            at: &at,
            images: &mut self.images,
            layers: Vec::new(),
            shapes: Vec::new(),
        };
        if at.viewport.width > 0. && at.viewport.height > 0. {
            place(
                &self.config.scene.items,
                &self.bodies,
                &stage,
                &[],
                &mut draw,
                cx,
            );
        }
        let Draw { layers, shapes, .. } = draw;
        self.shapes = shapes;

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

        let active = design && self.is_active(cx);
        let chrome = design.then(|| self.chrome_layer(active, cx));
        let inspector = active.then(|| crate::scene::overlay::render(self, window, cx));
        let tab = (design && !active && self.near_edge.get()).then(|| self.edge_tab(cx));
        let menu = self.menu_overlay(window);

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
            .child(watch)
            .children(layers)
            .children(chrome)
            .children(inspector)
            .children(tab)
            .children(menu)
    }

    pub(crate) fn viewport(&self) -> Viewport {
        let size = self.bounds.get().size;
        Viewport {
            width: f32::from(size.width),
            height: f32::from(size.height),
        }
    }

    /// How many group levels this scene has left before the cap, counting
    /// the scenes it sits inside.
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
    images: &'a mut Images,
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
    items: &[SceneItem],
    bodies: &[Body<Grid>],
    parent: &Frame,
    groups: &[String],
    draw: &mut Draw,
    cx: &mut Context<ScenePanel>,
) {
    let live = Live::default();
    let at = draw.at;
    let mut layers: Vec<Layer> = Vec::new();

    for (item, body) in items.iter().zip(bodies) {
        if !item.visible {
            continue;
        }

        let flat = edit::is_flat(&item.content);
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
                    images: &mut *draw.images,
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
                let element = image::element(item, parent, at, draw.images, cx);
                let Some(projected) = projected() else {
                    continue;
                };
                layers.push(Layer {
                    depth: projected.depth,
                    elements: element.map(|(_, e)| e).into_iter().collect(),
                    shapes: vec![shape(projected.corners, ShapeKind::Perspective)],
                });
            }

            // Kinds with no body yet still have a place, so the editor can
            // pick and move them.
            _ => {
                let Some(projected) = projected() else {
                    continue;
                };
                let kind = if flat {
                    ShapeKind::Flat
                } else {
                    ShapeKind::Perspective
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
fn grid_element(item: &SceneItem, grid: &Grid, corners: [[f32; 2]; 4], opacity: f32) -> AnyElement {
    let (origin, extent) = scene::bbox(&corners);

    div()
        .id(SharedString::from(format!("scene-grid-{}", item.id)))
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

impl ScenePanel {
    /// Selects `id` and makes this the scene whose chrome and inspector
    /// show. Focus comes here too, so the scene's keys apply.
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
            |_, _, _| (),
            move |bounds, _, window, _| {
                if let Some(shape) = &paint_selected {
                    paint_outline(shape, bounds.origin, window);
                    if shape.turns() {
                        paint_turn_handles(shape, bounds.origin, window);
                    }
                }

                let down = me.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !bounds.contains(&event.position) {
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
            .id("scene-edge-tab")
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
        // panel; inside it for an item flush with the top of the scene.
        let y = if top >= STRIP_H { top - STRIP_H } else { top };
        let accent = palette::accent();
        let chain = shape.chain();

        div()
            .id(SharedString::from(format!("scene-strip-{}", shape.id)))
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
                    .id(("scene-resize", ix))
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
        let scene = &self.config.scene;
        let (Some(item), Some(parent), Some(shape)) = (
            scene.find(id).cloned(),
            edit::parent_frame(scene, id, viewport),
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

        self.gesture = Some(Gesture {
            id: id.to_string(),
            before: self.config.scene.clone(),
            item,
            parent,
            camera: self.config.scene.camera,
            viewport,
            from: local,
            start_box: shape.edges(),
            kind,
        });
    }

    /// The parent box's edges and center, and the edges of every flat
    /// sibling, as they sit on screen.
    fn snap_lines(&self, moving: &Shape, parent: &Frame, viewport: Viewport) -> [Vec<f32>; 2] {
        let mut xs = Vec::new();
        let mut ys = Vec::new();

        let parent_box = SceneItem {
            place: Place {
                anchor: Anchor::TopLeft,
                size: [Length::Units(parent.size[0]), Length::Units(parent.size[1])],
                ..Place::default()
            },
            ..SceneItem::default()
        };
        let camera = self.config.scene.camera;
        if let Some(projected) = project(
            &parent_box,
            parent,
            &camera,
            viewport,
            None,
            false,
            &Live::default(),
        ) {
            let (origin, extent) = scene::bbox(&projected.corners);
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
        scene::each_grid(&self.config.scene.items, &self.bodies, &mut |item, grid| {
            if item.id == id {
                let min = grid.area.read(cx).items().view().min_size(cx);
                floor = [f32::from(min.width), f32::from(min.height)];
            }
        });
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
        if let Some(item) = self.config.scene.find_mut(&id) {
            item.place = place;
        }
        cx.notify();
    }

    fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };

        let after = self.saved(cx).scene;
        self.history
            .record(gesture.before, after, None, Instant::now());

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

    /// Add Grid, Add Group and Add Image at `position`. `at` places the new
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
                    PopupMenuItem::new(rox_i18n::t!("scene-add-file"))
                        .icon(Icon::default().path(icons::FOLDER))
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| this.add_file(at, window, cx)).ok();
                        }),
                )
            })
        };

        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(
                PopupMenuItem::new(rox_i18n::t!("scene-add-grid"))
                    .icon(Icon::default().path(icons::LAYOUT_GRID))
                    .on_click(pick(NewItem::Grid)),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("scene-add-group"))
                    .icon(Icon::default().path(icons::LAYERS))
                    .on_click(pick(NewItem::Group)),
            )
            .item(
                PopupMenuItem::submenu(rox_i18n::t!("scene-add-image"), image_menu)
                    .icon(Icon::default().path(icons::IMAGE)),
            )
        });
        self.open_menu(position, menu, window, cx);
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
        };

        let id = self.config.scene.fresh_id(stem);
        let mut item = SceneItem {
            id: id.clone(),
            content,
            ..SceneItem::default()
        };
        if let Some([w, h]) = size {
            item.place.size = [Length::Units(w), Length::Units(h)];
        }

        // Carried from the stage center to the press.
        let viewport = self.viewport();
        if let Some(at) = at {
            let stage = Frame::stage(&self.config.scene.stage, viewport);
            let camera = self.config.scene.camera;
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
                let (origin, extent) = scene::bbox(&projected.corners);
                let shift = [
                    at[0] - (origin[0] + extent[0] / 2.),
                    at[1] - (origin[1] + extent[1] / 2.),
                ];
                item.place.position = edit::moved(&item, &stage, &camera, viewport, [0.; 3], shift);
            }
        }

        self.change(move |scene| scene.items.push(item), window, cx);
        self.select(Some(id), window, cx);
    }

    /// Imports a picked file into the asset store, then adds it as an image.
    fn add_file(&mut self, at: Option<[f32; 2]>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;

        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

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
                        log::warn!("scene: adding {}: {e}", path.display());
                        Toast::new(Tone::Bad, e)
                            .title(rox_i18n::t!("scene-add-image-failed"))
                            .show(window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.take() else {
            return;
        };

        self.change(move |scene| drop(edit::remove(scene, &id)), window, cx);
    }

    pub(crate) fn group_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        let levels = self.levels_left();
        let mut made = None;
        self.change(|scene| made = edit::group(scene, &id, levels), window, cx);

        if made.is_some() {
            self.select(made, window, cx);
        }
    }

    pub(crate) fn ungroup_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !matches!(
            self.config.scene.find(&id).map(|item| &item.content),
            Some(Content::Group { .. })
        ) {
            return;
        }

        let viewport = self.viewport();
        self.change(
            |scene| {
                edit::ungroup(scene, &id, viewport);
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
            |scene| {
                edit::move_into(scene, id, group, viewport, levels);
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
            |scene| {
                let Some(parent) = edit::parent_frame(scene, &id, viewport) else {
                    return;
                };
                let Some(siblings) = edit::siblings(&scene.items, &id) else {
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

                if let Some(item) = scene.find_mut(&id) {
                    item.place.position[2] = z;
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
        if self.config.scene.find(&id).is_none_or(|item| item.locked) {
            return;
        }

        self.change_keyed(
            Some("nudge"),
            |scene| {
                if let Some(item) = scene.find_mut(&id) {
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
            let mut path = edit::path_to(&self.config.scene.items, id)?;
            path.pop();
            path.pop()
        });

        self.select(parent, window, cx);
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

/// Whether `at` is in the band along the scene's side where the edge tab
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

/// The image assets in the store, id and file, for the Add Image list.
pub(crate) fn image_assets() -> Vec<(String, String)> {
    assets::all()
        .into_iter()
        .filter(|asset| asset.kind == AssetKind::Image)
        .map(|asset| (asset.id, asset.file))
        .collect()
}

/// Copies a picked file into the store's staging folder. Only images, so
/// a font or a clip can't come in through here.
async fn stage_image(path: PathBuf) -> Result<assets::Staged, String> {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if assets::kind_for(&file) != Some(AssetKind::Image) {
        return Err(rox_i18n::t!("scene-add-image-not-image").to_string());
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

impl PanelSettings for ScenePanel {
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

impl EventEmitter<PanelEvent> for ScenePanel {}

impl Focusable for ScenePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ScenePanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        panel_settings::open(cx.entity(), cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("scene-panel-title"),
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
    /// can't see past the scene.
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
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.images.clear(window);
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let state = self.state.clone();
        let workspace = self.workspace.clone();
        let add = PopupMenu::build(window, cx, move |menu, window, cx| {
            let weak = weak.clone();
            composite::pick_items(
                menu,
                state.clone(),
                workspace.clone(),
                window,
                cx,
                move |panel, window, cx| {
                    if let Some(this) = weak.upgrade() {
                        this.update(cx, |this, cx| this.add_panel(panel, window, cx));
                    }
                },
            )
        });

        let menu = menu.item(
            PopupMenuItem::submenu(rox_i18n::t!("composite-add-panel"), add)
                .icon(Icon::default().path(icons::PLUS)),
        );
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

impl Render for ScenePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.images.follow_store(window);
        self.window = Some(window.window_handle());

        // Off design mode a scene is plain content: no drag carries over and
        // no menu stays open.
        let design = !self.config.chrome.controls_hidden();
        if design != self.design {
            self.design = design;
            if !design {
                self.gesture = None;
                self.menu = None;
            }
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
    fn converting_wraps_the_layout_and_loses_nothing() {
        let wrapped = serde_json::to_value(wrap_layout(layout(), 1)).expect("serializes");

        assert_eq!(wrapped["version"], 1);
        assert!(wrapped["center"]["info"]["stack"].is_object());

        let tabs = &wrapped["center"]["children"];
        assert_eq!(tabs.as_array().map(Vec::len), Some(1));
        let scene = &tabs[0]["children"];
        assert_eq!(scene.as_array().map(Vec::len), Some(1));
        assert_eq!(scene[0]["panel_name"], PANEL_NAME);

        // Read back the way the registry hands it to the panel.
        let info: PanelInfo =
            serde_json::from_value(scene[0]["info"].clone()).expect("info parses");
        let config: SceneConfig = panel::config_from_info(&info);
        assert_eq!(config.scene.items.len(), 1);

        let item = &config.scene.items[0];
        assert_eq!(item.place.anchor, Anchor::Center);
        assert_eq!(item.place.size, [Length::Stage(1.), Length::Stage(1.)]);
        match &item.content {
            Content::Grid { dock } => assert_eq!(dock, &layout()),
            _ => panic!("the one item is a grid"),
        }
    }

    #[test]
    fn a_scene_dumps_no_children() {
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
            let mut state: SceneState =
                serde_json::from_value(json!({ "items": [item.clone()] })).expect("parses");
            let bodies = scene::build_bodies(&state.items, 0, &mut |_, dock, _| {
                grid::usable(dock).map(|_| ())
            });
            assert!(matches!(bodies[0], Body::Unknown));

            scene::fold_dumps(&mut state.items, &bodies, &|_| Some(json!("fresh")));
            let saved = serde_json::to_value(&state.items[0]).expect("serializes");
            assert_eq!(saved["content"], item["content"]);
            assert_eq!(saved["name"], "Kept");
        }

        assert!(grid::usable(&layout()).is_some());
    }

    #[test]
    fn the_config_reads_routes_and_chrome_beside_the_scene() {
        let config: SceneConfig = serde_json::from_value(json!({
            "title": "Stage",
            "camera": { "fov": 0, "parallax": 12 },
            "routes": [{ "signal": 3, "target": "grid.opacity" }],
            "items": [],
        }))
        .expect("parses");

        assert_eq!(config.chrome.title.as_deref(), Some("Stage"));
        assert_eq!(config.scene.camera.parallax, 12.);
        assert_eq!(config.routes[0].target, "grid.opacity");

        let empty = serde_json::to_value(SceneConfig::default()).expect("serializes");
        assert!(empty.get("routes").is_none());
    }
}
