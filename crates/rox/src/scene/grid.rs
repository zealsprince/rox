//! A grid item: a dock tree of its own inside a scene, built and restored the
//! way the window builds its dock. Its tab groups are real tab groups, so a
//! tab drags, zooms and pops out as it does anywhere. Zoom covers only the
//! grid it happens in.
//!
//! The dump lives in the item's `dock` value inside the scene's config,
//! never in the scene's `PanelState.children`, so the dock's restore
//! doesn't build the children a second time.

use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Subscription, Window};
use rox_dock::{DockArea, DockAreaState, DockEvent, PanelInfo, PanelView, StackPanel};
use serde_json::Value;

use crate::panels::scene::ScenePanel;
use crate::scene::Nesting;
use crate::workspace::{LAYOUT_VERSION, empty_center};
use rox_panel_api::panel::AppState;

pub(crate) struct Grid {
    pub area: Entity<DockArea>,
    _changed: Subscription,
}

/// A dump a grid can restore from, held to the gates the window's own
/// layout passes: this build's layout version and a split at the root.
pub(crate) fn usable(dump: &Value) -> Option<DockAreaState> {
    serde_json::from_value::<DockAreaState>(dump.clone())
        .ok()
        .filter(|dump| {
            dump.version == Some(LAYOUT_VERSION)
                && matches!(dump.center.info, PanelInfo::Stack { .. })
        })
}

/// None keeps the item as saved, so a dump this build can't restore is
/// still there on the next save.
pub(crate) fn restore(
    id: &str,
    dump: &Value,
    level: usize,
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> Option<Grid> {
    let Some(dump) = usable(dump) else {
        log::warn!("scene: keeping grid {id} as saved, its dock dump doesn't restore");
        return None;
    };

    let area = new_area(id, state, window, cx);
    let center = if dump.center.children.is_empty() {
        // A root with no tab group has nowhere to take a drop, so it gets
        // the empty group a new grid starts with.
        empty_center(&area.downgrade(), window, cx).0
    } else {
        // A scene panel the registry builds inside this dump reads its
        // level from here.
        let _nesting = Nesting::enter(level);
        dump.center.to_item(area.downgrade(), window, cx)
    };
    area.update(cx, |area, cx| area.set_center(center, window, cx));

    Some(watch(area, window, cx))
}

/// One tab group holding `panel`.
pub(crate) fn holding(
    id: &str,
    panel: Arc<dyn PanelView>,
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> Grid {
    let area = new_area(id, state, window, cx);
    let (center, tabs) = empty_center(&area.downgrade(), window, cx);
    area.update(cx, |area, cx| area.set_center(center, window, cx));
    tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));

    watch(area, window, cx)
}

/// Re-render every split in a grid's tree.
pub(crate) fn refresh_splits(area: &Entity<DockArea>, cx: &mut App) {
    fn walk(node: &Arc<dyn PanelView>, cx: &mut App) {
        let view = node.view();
        if let Ok(stack) = view.downcast::<StackPanel>() {
            let children = stack.read(cx).panels().to_vec();
            cx.notify(stack.entity_id());
            for child in &children {
                walk(child, cx);
            }
        }
    }

    let root = area.read(cx).items().view();
    walk(&root, cx);
}

pub(crate) fn dump(grid: &Grid, cx: &App) -> Option<Value> {
    serde_json::to_value(grid.area.read(cx).dump(cx)).ok()
}

fn new_area(
    id: &str,
    state: &AppState,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> Entity<DockArea> {
    let area = cx.new(|cx| DockArea::new(format!("scene-{id}"), Some(LAYOUT_VERSION), window, cx));
    let scene = cx.entity_id();

    area.update(cx, |area, cx| {
        area.set_toggle_button_visible(false, cx);
        area.set_nested(scene, cx);

        // A middle drag released outside the window pops the panel out, the
        // same as in the window's own dock.
        let state = state.clone();
        area.on_middle_drag_out(move |panel, _, _, cx| {
            rox_panel_api::panel::pop_out_view(panel, state.clone(), cx);
        });
    });

    area
}

/// The scene emits no layout event of its own, so without this an edit
/// inside a grid waits for something else to save it.
fn watch(area: Entity<DockArea>, window: &mut Window, cx: &mut Context<ScenePanel>) -> Grid {
    let changed = cx.subscribe_in(&area, window, |this, _, event: &DockEvent, window, cx| {
        if matches!(event, DockEvent::LayoutChanged) {
            this.save_soon(window, cx);
        }
    });

    Grid {
        area,
        _changed: changed,
    }
}
