//! Weight (ADR 33): what a workspace, a scene or an object asks of the
//! renderer, counted from its saved JSON without building anything, and the
//! lines a confirm or the inspector shows when a count is past what drew
//! smoothly when it was measured. A warning never blocks anything.
//!
//! The counts are a guess at frame cost, not a reading of it. A count with
//! no measured crossing gets no threshold and says nothing.

use std::collections::HashSet;

use gpui::SharedString;
use rox_core::settings::roxo::ObjectBundle;
use rox_core::settings::{self, PostShaderConfig, WorkspaceBundle};
use rox_scene::{Content, Cost, SceneItem, SceneState};
use rox_viz::signal::Route;
use serde::Deserialize as _;
use serde_json::{Value, json};

use crate::panels::scene::PANEL_NAME;

// Measured 2026-11-16 on a Ryzen 9 5950X with a Radeon RX 6800 XT, Vulkan
// through RADV, release build, a 1920x1080 window in a headless sway, music
// playing and a route keeping every frame drawn. Mean `debug.frames` cost
// per frame in ms, which is main-thread time; the GPU kept up at 60 Hz in
// every case, so its own time doesn't show here. Each grid held the
// transport panel, each panel shader was a Shader panel in its own tab group,
// and each shader item ran Plasma:
//
//   count               4     8     16    32     64     96
//   grid items          1.5   1.9   2.4   4.0    7.4    12.3
//   shader items        0.6   0.7   0.7   0.7    1.0
//   panel shaders                   2.6   4.2    7.9    11.6
//   moving grids        1.2         3.4                        (1: 0.7)
//   animated images                 0.7
//   nesting 1, 2, 3 and 4: 1.0, 1.2, 1.5 and 1.8
//
// A threshold sits where the line crosses 8.3 ms, half a 60 Hz frame, read
// between the two points around it. Shader items, moving grids and nesting
// never got there, so they warn about nothing. A moving grid is a grid too.
const GRIDS: usize = 70;
const SHADER_SURFACES: usize = 68;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Weight {
    /// Summed over every scene in the dump. `depth` is the deepest scene in
    /// a scene plus the groups at that level.
    pub cost: Cost,
    /// Panels, scenes and the window's own passes that wear a shader. A
    /// scene's shader items are `cost.shaders`, not these.
    pub shader_surfaces: usize,
    /// Grid items a route moves, a routed group's grids included.
    pub moving_grids: usize,
}

impl Weight {
    /// Each count at the larger of the two.
    fn max(self, other: Weight) -> Weight {
        Weight {
            cost: Cost {
                items: self.cost.items.max(other.cost.items),
                grids: self.cost.grids.max(other.cost.grids),
                shaders: self.cost.shaders.max(other.cost.shaders),
                textures: self.cost.textures.max(other.cost.textures),
                depth: self.cost.depth.max(other.cost.depth),
            },
            shader_surfaces: self.shader_surfaces.max(other.shader_surfaces),
            moving_grids: self.moving_grids.max(other.moving_grids),
        }
    }
}

/// One layout dump, with the bundle's window passes.
pub(crate) fn weigh(dump: &Value, bundle: &WorkspaceBundle) -> Weight {
    let mut weight = weigh_dump(dump);

    let on = |pass: &Option<PostShaderConfig>| pass.as_ref().is_some_and(|pass| pass.enabled);
    weight.shader_surfaces +=
        usize::from(on(&bundle.post_shader)) + usize::from(on(&bundle.backdrop_shader));

    weight
}

/// Every layout in the bundle, each count at its heaviest. Counts from
/// different layouts can meet here, which only matters to a count that's
/// over on its own anyway.
pub(crate) fn heaviest(bundle: &WorkspaceBundle) -> Weight {
    bundle
        .layouts
        .iter()
        .map(|layout| weigh(&layout.dump, bundle))
        .fold(weigh(&Value::Null, bundle), Weight::max)
}

/// An object's item, weighed as a scene holding only it and its routes.
pub(crate) fn weigh_object(bundle: &ObjectBundle) -> Weight {
    weigh_dump(&scene_node(json!({
        "items": [bundle.item],
        "routes": bundle.routes,
    })))
}

/// A scene panel's own config, nested scenes included.
pub(crate) fn weigh_scene(config: Value) -> Weight {
    weigh_dump(&scene_node(config))
}

fn scene_node(config: Value) -> Value {
    json!({ "panel_name": PANEL_NAME, "info": { "panel": config } })
}

fn weigh_dump(dump: &Value) -> Weight {
    let mut tally = Tally::default();
    find_scenes(dump, 0, &mut tally);

    // The dump walker counts a shader item's source too, and those are
    // counted as shader items.
    let sources = settings::dump_shader_sources(dump).len();
    tally.weight.shader_surfaces = sources.saturating_sub(tally.item_sources);

    tally.weight
}

#[derive(Default)]
struct Tally {
    weight: Weight,
    /// Shader items carrying inline source.
    item_sources: usize,
}

/// Every scene panel in a dump, at the level the dump sits at, the way the
/// dump walkers recurse.
fn find_scenes(value: &Value, level: usize, tally: &mut Tally) {
    match value {
        Value::Object(map) => {
            if map.get("panel_name").and_then(Value::as_str) == Some(PANEL_NAME) {
                if let Some(config) = map.get("info").and_then(|info| info.get("panel")) {
                    scene(config, level, tally);
                }
                return;
            }

            map.values()
                .for_each(|child| find_scenes(child, level, tally));
        }

        Value::Array(items) => items
            .iter()
            .for_each(|item| find_scenes(item, level, tally)),

        _ => {}
    }
}

fn scene(config: &Value, level: usize, tally: &mut Tally) {
    let Ok(state) = SceneState::deserialize(config) else {
        return;
    };
    let routes = config
        .get("routes")
        .and_then(|routes| Vec::<Route>::deserialize(routes).ok())
        .unwrap_or_default();

    let cost = state.cost();
    let sum = &mut tally.weight.cost;
    sum.items += cost.items;
    sum.grids += cost.grids;
    sum.shaders += cost.shaders;
    sum.textures += cost.textures;
    sum.depth = sum.depth.max(level + cost.depth);

    let routed: HashSet<&str> = routes
        .iter()
        .filter(|route| route.enabled)
        .filter_map(|route| route.target.rsplit_once('.').map(|(id, _)| id))
        .collect();

    items(&state.items, &routed, false, level, tally);
}

/// `level` counts the scenes and groups these items sit inside, so a scene
/// in a grid here opens the next one, as it does when built.
fn items(
    items: &[SceneItem],
    routed: &HashSet<&str>,
    moved: bool,
    level: usize,
    tally: &mut Tally,
) {
    for item in items {
        let moved = moved || routed.contains(item.id.as_str());

        match &item.content {
            Content::Grid { dock } => {
                tally.weight.moving_grids += usize::from(moved);
                find_scenes(dock, level + 1, tally);
            }

            Content::Group { items: children } => {
                self::items(children, routed, moved, level + 1, tally);
            }

            Content::Shader { shader } => {
                tally.item_sources +=
                    usize::from(shader.get("source").is_some_and(Value::is_string));
            }

            _ => {}
        }
    }
}

/// The inspector's line: the counts always, over or not.
pub(crate) fn counts(weight: &Weight) -> SharedString {
    rox_i18n::t!(
        "scene-weight-counts",
        items = weight.cost.items as u64,
        grids = weight.cost.grids as u64,
        shaders = weight.cost.shaders as u64,
        surfaces = weight.shader_surfaces as u64,
        moving = weight.moving_grids as u64,
        depth = weight.cost.depth as u64
    )
}

/// The lines to show, empty when nothing is over.
pub(crate) fn warnings(weight: &Weight) -> Vec<SharedString> {
    let over = [
        (weight.cost.grids, GRIDS, "scene-weight-many-grids"),
        (
            weight.shader_surfaces,
            SHADER_SURFACES,
            "scene-weight-many-shaders",
        ),
    ];

    over.into_iter()
        .filter(|(count, threshold, _)| count >= threshold)
        .map(|(count, _, key)| rox_i18n::t!(key, count = count as u64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(id: &str, dock: Value) -> Value {
        json!({ "id": id, "content": { "kind": "grid", "dock": dock } })
    }

    fn plain_dock() -> Value {
        json!({ "version": 1, "center": { "panel_name": "StackPanel", "children": [] } })
    }

    /// A dock holding one scene panel with these items and routes.
    fn dock_with_scene(items: Value, routes: Value) -> Value {
        json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [{
                    "panel_name": "TabPanel",
                    "children": [{
                        "panel_name": PANEL_NAME,
                        "children": [],
                        "info": { "panel": { "items": items, "routes": routes } },
                    }],
                    "info": { "tabs": { "active_index": 0 } },
                }],
                "info": { "stack": { "sizes": [], "axis": 1 } },
            },
        })
    }

    fn route(target: &str, enabled: bool) -> Value {
        json!({ "enabled": enabled, "signal": 1, "target": target, "from": 0.0, "to": 1.0 })
    }

    fn bare() -> WorkspaceBundle {
        WorkspaceBundle::default()
    }

    #[test]
    fn a_layout_with_no_scene_weighs_nothing() {
        let dump = json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [{ "panel_name": "playback", "children": [], "info": { "panel": {} } }],
            },
        });

        assert_eq!(weigh(&dump, &bare()), Weight::default());
        assert_eq!(heaviest(&bare()), Weight::default());
    }

    #[test]
    fn a_scene_inside_a_scene_is_counted_and_deepens() {
        let inner = dock_with_scene(json!([grid("leaf", plain_dock())]), json!([]));
        let dump = dock_with_scene(json!([grid("outer", inner)]), json!([]));

        let weight = weigh(&dump, &bare());
        assert_eq!(weight.cost.items, 2);
        assert_eq!(weight.cost.grids, 2);
        assert_eq!(weight.cost.depth, 1);
    }

    #[test]
    fn a_group_deepens_without_adding_a_grid() {
        let group = json!({
            "id": "g",
            "content": { "kind": "group", "items": [{ "id": "t", "content": {
                "kind": "text", "text": "a", "size": 12.0, "color": "text",
            } }] },
        });
        let weight = weigh(&dock_with_scene(json!([group]), json!([])), &bare());

        assert_eq!(weight.cost.items, 2);
        assert_eq!(weight.cost.grids, 0);
        assert_eq!(weight.cost.depth, 1);
    }

    #[test]
    fn a_grid_on_a_route_is_a_moving_grid() {
        let group = json!({
            "id": "g",
            "content": { "kind": "group", "items": [grid("inside", plain_dock())] },
        });
        let items = json!([grid("a", plain_dock()), grid("b", plain_dock()), group]);
        let routes = json!([route("a.x", true), route("g.opacity", true)]);

        let weight = weigh(&dock_with_scene(items, routes), &bare());
        assert_eq!(weight.cost.grids, 3);
        assert_eq!(
            weight.moving_grids, 2,
            "a, and the grid in the routed group"
        );
    }

    #[test]
    fn a_disabled_route_moves_nothing() {
        let items = json!([grid("a", plain_dock())]);
        let weight = weigh(
            &dock_with_scene(items, json!([route("a.x", false)])),
            &bare(),
        );

        assert_eq!(weight.cost.grids, 1);
        assert_eq!(weight.moving_grids, 0);
    }

    #[test]
    fn nothing_over_gives_no_lines() {
        assert!(warnings(&Weight::default()).is_empty());

        let mut weight = Weight {
            shader_surfaces: SHADER_SURFACES - 1,
            ..Weight::default()
        };
        weight.cost.grids = GRIDS - 1;
        weight.cost.shaders = 500;
        weight.moving_grids = 500;
        assert!(warnings(&weight).is_empty(), "only measured counts warn");

        weight.cost.grids = GRIDS;
        assert_eq!(warnings(&weight).len(), 1);
    }

    #[test]
    fn a_shader_item_is_not_also_a_surface() {
        let shader = json!({
            "id": "s",
            "content": { "kind": "shader", "shader": { "enabled": true, "source": "fn x() {}" } },
        });
        let panel = json!({
            "panel_name": "shader",
            "children": [],
            "info": { "panel": { "source": "fn y() {}" } },
        });
        let inner =
            json!({ "version": 1, "center": { "panel_name": "StackPanel", "children": [panel] } });
        let mut bundle = bare();
        bundle.post_shader = Some(PostShaderConfig {
            enabled: true,
            ..PostShaderConfig::default()
        });

        let weight = weigh(
            &dock_with_scene(json!([shader, grid("p", inner)]), json!([])),
            &bundle,
        );
        assert_eq!(weight.cost.shaders, 1);
        assert_eq!(
            weight.shader_surfaces, 2,
            "the shader panel and the screen pass"
        );
    }

    #[test]
    fn an_object_weighs_as_its_own_scene() {
        let item = json!({
            "id": "g",
            "content": { "kind": "group", "items": [grid("a", plain_dock())] },
        });
        let bundle = ObjectBundle {
            item,
            routes: vec![Route {
                target: "a.y".into(),
                ..Route::default()
            }],
            ..ObjectBundle::default()
        };

        let weight = weigh_object(&bundle);
        assert_eq!(weight.cost.items, 2);
        assert_eq!(weight.moving_grids, 1);
        assert_eq!(weight.cost.depth, 1);
    }

    #[test]
    fn the_heaviest_layout_sets_each_count() {
        let light = dock_with_scene(json!([grid("a", plain_dock())]), json!([]));
        let heavy = dock_with_scene(
            json!([grid("a", plain_dock()), grid("b", plain_dock())]),
            json!([]),
        );
        let mut bundle = bare();
        for (name, dump) in [("light", light), ("heavy", heavy)] {
            bundle.layouts.push(settings::NamedLayout {
                name: name.into(),
                dump,
                size: None,
            });
        }

        assert_eq!(heaviest(&bundle).cost.grids, 2);
    }
}
