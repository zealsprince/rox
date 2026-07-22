use gpui::{App, AppContext, Axis, Bounds, Entity, Pixels, WeakEntity, Window, point, px, size};
use itertools::Itertools as _;
use serde::{Deserialize, Serialize};

use super::{Dock, DockArea, DockItem, DockPlacement, Panel, PanelRegistry};

/// Used to serialize and deserialize the DockArea
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct DockAreaState {
    /// The version is used to mark this persisted state is compatible with the current version
    /// For example, some times we many totally changed the structure of the Panel,
    /// then we can compare the version to decide whether we can use the state or ignore.
    #[serde(default)]
    pub version: Option<usize>,
    pub center: PanelState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_dock: Option<DockState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_dock: Option<DockState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom_dock: Option<DockState>,
}

/// Used to serialize and deserialize the Dock
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DockState {
    panel: PanelState,
    placement: DockPlacement,
    size: Pixels,
    open: bool,
}

impl DockState {
    pub fn new(dock: Entity<Dock>, cx: &App) -> Self {
        let dock = dock.read(cx);

        Self {
            placement: dock.placement,
            size: dock.size,
            open: dock.open,
            panel: dock.panel.view().dump(cx),
        }
    }

    /// Convert the DockState to Dock
    pub fn to_dock(
        &self,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Dock> {
        let item = self.panel.to_item(dock_area.clone(), window, cx);
        cx.new(|cx| {
            Dock::from_state(
                dock_area.clone(),
                self.placement,
                self.size,
                item,
                self.open,
                window,
                cx,
            )
        })
    }
}

/// Used to serialize and deserialize the DockerItem
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelState {
    pub panel_name: String,
    pub children: Vec<PanelState>,
    pub info: PanelInfo,
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileMeta {
    pub bounds: Bounds<Pixels>,
    pub z_index: usize,
}

impl Default for TileMeta {
    fn default() -> Self {
        Self {
            bounds: Bounds {
                origin: point(px(10.), px(10.)),
                size: size(px(200.), px(200.)),
            },
            z_index: 0,
        }
    }
}

impl From<Bounds<Pixels>> for TileMeta {
    fn from(bounds: Bounds<Pixels>) -> Self {
        Self { bounds, z_index: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PanelInfo {
    #[serde(rename = "stack")]
    Stack {
        sizes: Vec<Pixels>,
        axis: usize, // 0 for horizontal, 1 for vertical
    },
    #[serde(rename = "tabs")]
    Tabs { active_index: usize },
    #[serde(rename = "panel")]
    Panel(serde_json::Value),
    #[serde(rename = "tiles")]
    Tiles { metas: Vec<TileMeta> },
}

impl PanelInfo {
    pub fn stack(sizes: Vec<Pixels>, axis: Axis) -> Self {
        Self::Stack {
            sizes,
            axis: if axis == Axis::Horizontal { 0 } else { 1 },
        }
    }

    pub fn tabs(active_index: usize) -> Self {
        Self::Tabs { active_index }
    }

    pub fn panel(info: serde_json::Value) -> Self {
        Self::Panel(info)
    }

    pub fn tiles(metas: Vec<TileMeta>) -> Self {
        Self::Tiles { metas }
    }

    pub fn axis(&self) -> Option<Axis> {
        match self {
            Self::Stack { axis, .. } => Some(if *axis == 0 {
                Axis::Horizontal
            } else {
                Axis::Vertical
            }),
            _ => None,
        }
    }

    pub fn sizes(&self) -> Option<&Vec<Pixels>> {
        match self {
            Self::Stack { sizes, .. } => Some(sizes),
            _ => None,
        }
    }

    pub fn active_index(&self) -> Option<usize> {
        match self {
            Self::Tabs { active_index } => Some(*active_index),
            _ => None,
        }
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            panel_name: "".to_string(),
            children: Vec::new(),
            info: PanelInfo::Panel(serde_json::Value::Null),
        }
    }
}

impl PanelState {
    pub fn new<P: Panel>(panel: &P) -> Self {
        Self {
            panel_name: panel.panel_name().to_string(),
            ..Default::default()
        }
    }

    pub fn add_child(&mut self, panel: PanelState) {
        self.children.push(panel);
    }

    pub fn to_item(
        &self,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> DockItem {
        let info = self.info.clone();

        let items: Vec<DockItem> = self
            .children
            .iter()
            .map(|child| child.to_item(dock_area.clone(), window, cx))
            .collect();

        match info {
            PanelInfo::Stack { sizes, axis } => {
                let axis = if axis == 0 {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                let sizes = sizes.iter().map(|s| Some(*s)).collect_vec();
                DockItem::split_with_sizes(axis, items, sizes, &dock_area, window, cx)
            }
            PanelInfo::Tabs { active_index } => {
                if items.len() == 1 {
                    return items[0].clone();
                }

                let items = items
                    .iter()
                    .flat_map(|item| match item {
                        DockItem::Tabs { items, .. } => items.clone(),
                        _ => {
                            // ignore invalid panels in tabs
                            vec![]
                        }
                    })
                    .collect_vec();

                DockItem::tabs(items, &dock_area, window, cx).active_index(active_index, cx)
            }
            // An empty container that an older build dumped with the default
            // `Panel(Null)` info instead of its own tabs/stack info (the bug
            // only hit empty containers, so there are no children to carry).
            // Rebuild it as the empty container it was, rather than routing the
            // container name through the registry and coming back as an
            // InvalidPanel.
            PanelInfo::Panel(_) if self.panel_name == "TabPanel" => {
                DockItem::tabs(Vec::new(), &dock_area, window, cx)
            }
            PanelInfo::Panel(_) if self.panel_name == "StackPanel" => {
                DockItem::split_with_sizes(
                    Axis::Horizontal,
                    Vec::new(),
                    Vec::new(),
                    &dock_area,
                    window,
                    cx,
                )
            }
            PanelInfo::Panel(_) => {
                let view = PanelRegistry::build_panel(
                    &self.panel_name,
                    dock_area.clone(),
                    self,
                    &info,
                    window,
                    cx,
                );
                DockItem::tabs(vec![view.into()], &dock_area, window, cx)
            }
            PanelInfo::Tiles { metas } => DockItem::tiles(items, metas, &dock_area, window, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;
    #[test]
    fn test_deserialize_item_state() {
        // One level up from upstream: state.rs sits at src/ here, not src/dock/.
        let json = include_str!("../tests/fixtures/layout.json");
        let state: DockAreaState = serde_json::from_str(json).unwrap();
        assert_eq!(state.version, None);
        assert_eq!(state.center.panel_name, "StackPanel");
        assert_eq!(state.center.children.len(), 2);
        assert_eq!(state.center.children[0].panel_name, "TabPanel");
        assert_eq!(state.center.children[1].children.len(), 1);
        assert_eq!(
            state.center.children[1].children[0].panel_name,
            "StoryContainer"
        );
        assert_eq!(state.center.children[1].panel_name, "TabPanel");

        let left_dock = state.left_dock.unwrap();
        assert_eq!(left_dock.open, true);
        assert_eq!(left_dock.size, px(350.0));
        assert_eq!(left_dock.placement, DockPlacement::Left);
        assert_eq!(left_dock.panel.panel_name, "TabPanel");
        assert_eq!(left_dock.panel.children.len(), 1);
        assert_eq!(left_dock.panel.children[0].panel_name, "StoryContainer");

        let bottom_dock = state.bottom_dock.unwrap();
        assert_eq!(bottom_dock.open, true);
        assert_eq!(bottom_dock.size, px(200.0));
        assert_eq!(bottom_dock.panel.panel_name, "TabPanel");
        assert_eq!(bottom_dock.panel.children.len(), 2);
        assert_eq!(bottom_dock.panel.children[0].panel_name, "StoryContainer");

        let right_dock = state.right_dock.unwrap();
        assert_eq!(right_dock.open, true);
        assert_eq!(right_dock.size, px(320.0));
        assert_eq!(right_dock.panel.panel_name, "TabPanel");
        assert_eq!(right_dock.panel.children.len(), 1);
        assert_eq!(right_dock.panel.children[0].panel_name, "StoryContainer");
    }

    // Build a nested layout in code, serialize it, read it back, and require
    // it survives byte-for-byte. This guards settings.json layout persistence:
    // if a field stops round-tripping, a saved dock layout silently changes on
    // the next load.
    #[test]
    fn dock_area_state_round_trips() {
        let leaf = PanelState {
            panel_name: "Spectrum".to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::json!({ "gain": 1.5 })),
        };
        let tabs = PanelState {
            panel_name: "TabPanel".to_string(),
            children: vec![leaf.clone(), leaf.clone()],
            info: PanelInfo::tabs(1),
        };
        let center = PanelState {
            panel_name: "StackPanel".to_string(),
            children: vec![tabs],
            info: PanelInfo::stack(vec![px(300.0), px(500.0)], Axis::Horizontal),
        };

        let state = DockAreaState {
            version: Some(2),
            center,
            left_dock: Some(DockState {
                panel: PanelState::default(),
                placement: DockPlacement::Left,
                size: px(280.0),
                open: true,
            }),
            right_dock: None,
            bottom_dock: None,
        };

        let json = serde_json::to_string(&state).unwrap();
        let back: DockAreaState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    // The empty-container repair path: an older build dumped an empty TabPanel
    // with the default Panel(Null) info instead of its own tabs info. That must
    // still deserialize as the same PanelState so to_item can rebuild it as an
    // empty tabs container rather than routing "TabPanel" through the registry.
    #[test]
    fn empty_tab_container_default_info_round_trips() {
        let empty = PanelState {
            panel_name: "TabPanel".to_string(),
            children: Vec::new(),
            info: PanelInfo::Panel(serde_json::Value::Null),
        };
        let json = serde_json::to_string(&empty).unwrap();
        let back: PanelState = serde_json::from_str(&json).unwrap();
        assert_eq!(empty, back);
        // The name/info combo the repair path keys on is preserved.
        assert_eq!(back.panel_name, "TabPanel");
        assert!(matches!(back.info, PanelInfo::Panel(serde_json::Value::Null)));
    }

    // Tiles carry per-tile bounds and z-index; those must survive a save/load
    // so a tiled layout doesn't reflow on restart.
    #[test]
    fn tiles_info_round_trips() {
        let metas = vec![
            TileMeta {
                bounds: Bounds {
                    origin: point(px(5.), px(15.)),
                    size: size(px(320.), px(240.)),
                },
                z_index: 3,
            },
            TileMeta::default(),
        ];
        let info = PanelInfo::tiles(metas.clone());
        let json = serde_json::to_string(&info).unwrap();
        let back: PanelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, back);
        match back {
            PanelInfo::Tiles { metas: got } => {
                assert_eq!(got.len(), 2);
                assert_eq!(got[0].z_index, 3);
                assert_eq!(got[1], TileMeta::default());
            }
            _ => panic!("tiles info deserialized as the wrong variant"),
        }
    }

    #[test]
    fn panel_info_accessors_match_variant() {
        let stack = PanelInfo::stack(vec![px(1.), px(2.)], Axis::Vertical);
        assert_eq!(stack.axis(), Some(Axis::Vertical));
        assert_eq!(stack.sizes().map(|s| s.len()), Some(2));
        assert_eq!(stack.active_index(), None);

        let tabs = PanelInfo::tabs(4);
        assert_eq!(tabs.active_index(), Some(4));
        assert_eq!(tabs.axis(), None);
        assert_eq!(tabs.sizes(), None);

        // Horizontal maps to axis 0, vertical to 1; the accessor inverts it.
        assert_eq!(
            PanelInfo::stack(vec![], Axis::Horizontal).axis(),
            Some(Axis::Horizontal)
        );
    }

    // PanelState::default is the seed for a freshly-dumped panel, and its
    // Panel(Null) info is exactly what the empty-container repair path keys on.
    #[test]
    fn panel_state_default_is_empty_null_panel() {
        let d = PanelState::default();
        assert_eq!(d.panel_name, "");
        assert!(d.children.is_empty());
        assert!(matches!(d.info, PanelInfo::Panel(serde_json::Value::Null)));
    }
}
