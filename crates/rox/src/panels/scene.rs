//! The scene panel (ADR 33): a composite that places items in 3D space. Live
//! UI goes in as grid items, each a dock tree of its own, so a scene holds
//! real tab groups that drag, zoom and pop out like any other. Images take
//! real perspective; grids stay flat to the screen and take their place and
//! depth order from the projection.
//!
//! A scene is only ever a panel. The window's root stays the dock, and a
//! full-window scene is a dock holding one scene panel. Its saved state is
//! its config, nested dock dumps included, and `PanelState.children` stays
//! empty. The editor isn't here: this panel draws, saves and restores.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Axis, Bounds, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    MouseMoveEvent, Pixels, Point, SharedString, WeakEntity, Window, canvas, div, prelude::*, px,
};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{
    DockArea, DockAreaState, Panel, PanelEvent, PanelInfo, PanelState, PanelView, TabPanel,
};
use rox_scene::{
    Anchor, Content, Frame, Length, Live, Place, SceneItem, SceneState, Viewport, child_frame,
    paint_order, project,
};
use rox_viz::signal::Route;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::composite;
use crate::scene::grid::{self, Grid};
use crate::scene::image::{self, Images};
use crate::scene::{self, Body, Placing};
use crate::workspace::Workspace;
use rox_core::settings;
use rox_design::assets::icons;
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_api::windows::WorkspaceWindows;

/// The registry name.
pub const PANEL_NAME: &str = "scene";

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
    config: SceneConfig,
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
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
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
            focus: cx.focus_handle(),
            tab_panel: None,
        };
        this.build(HashMap::new(), window, cx);
        this
    }

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
        // The edit sees every grid as it stands now, not as it was loaded.
        self.config = self.saved(cx);
        Self::remember(&self.config.scene);

        let mut keep = std::mem::take(&mut self.spare);
        scene::take_grids(
            &self.config.scene.items,
            std::mem::take(&mut self.bodies),
            &mut keep,
        );

        edit(&mut self.config.scene);
        self.build(keep, window, cx);

        self.save_soon(window, cx);
        cx.notify();
    }

    /// Where undo records the state an edit replaces. Nothing keeps it yet.
    fn remember(_before: &SceneState) {}

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
            design: settings::design_mode(),
        };
        let stage = Frame::stage(&self.config.scene.stage, at.viewport);

        // Before the first bounds land there's nothing to place against, and
        // a grid laid out at no size would lay its panels out at none.
        let mut layers = Vec::new();
        if at.viewport.width > 0. && at.viewport.height > 0. {
            place(
                &self.config.scene.items,
                &self.bodies,
                &stage,
                &at,
                &mut self.images,
                &mut layers,
                cx,
            );
        }

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
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus)
            .child(watch)
            .children(layers)
    }
}

/// Siblings back to front. A group takes one slot at its own depth and lays
/// its children out inside it.
fn place(
    items: &[SceneItem],
    bodies: &[Body<Grid>],
    parent: &Frame,
    at: &Placing,
    images: &mut Images,
    out: &mut Vec<AnyElement>,
    cx: &mut Context<ScenePanel>,
) {
    let live = Live::default();
    let mut layers: Vec<(f32, Vec<AnyElement>)> = Vec::new();

    for (item, body) in items.iter().zip(bodies) {
        if !item.visible {
            continue;
        }

        match (body, &item.content) {
            (Body::Grid(grid), _) => {
                let flat = true;
                let Some(projected) = project(
                    item,
                    parent,
                    &at.camera,
                    at.viewport,
                    at.pointer,
                    flat,
                    &live,
                ) else {
                    continue;
                };
                layers.push((
                    projected.depth,
                    vec![grid_element(
                        item,
                        grid,
                        projected.corners,
                        projected.opacity,
                    )],
                ));
            }

            (Body::Group(children), Content::Group { items }) => {
                let frame = child_frame(item, parent, &live);
                let center = [frame.size[0] / 2., frame.size[1] / 2., 0.];
                let depth = frame.transform.point(center)[2];

                let mut inner = Vec::new();
                place(items, children, &frame, at, images, &mut inner, cx);
                layers.push((depth, inner));
            }

            (Body::Image, _) => {
                if let Some((depth, element)) = image::element(item, parent, at, images, cx) {
                    layers.push((depth, vec![element]));
                }
            }

            _ => {}
        }
    }

    let depths: Vec<f32> = layers.iter().map(|(depth, _)| *depth).collect();
    let mut layers: Vec<Option<Vec<AnyElement>>> =
        layers.into_iter().map(|(_, layer)| Some(layer)).collect();
    for ix in paint_order(&depths) {
        out.extend(layers[ix].take().unwrap_or_default());
    }
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
