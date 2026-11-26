//! Weight (ADR 33): what a workspace, a canvas or an object asks of the
//! renderer, counted from its saved JSON without building anything, and the
//! lines a confirm or the inspector shows when a count is past what drew
//! smoothly when it was measured. A warning never blocks anything.
//!
//! The counts are a guess at frame cost, not a reading of it. A count with
//! no measured crossing gets no threshold and says nothing.

use std::collections::HashSet;

use gpui::SharedString;
use rox_canvas::{CanvasItem, CanvasState, Content, Cost};
use rox_core::settings::roxo::ObjectBundle;
use rox_core::settings::{self, PostShaderConfig, WorkspaceBundle};
use rox_viz::signal::Route;
use serde::Deserialize as _;
use serde_json::{Value, json};

use crate::panels::canvas::PANEL_NAME;

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

// Render targets, measured 2026-11-24 on the same Ryzen 9 5950X and RX 6800 XT,
// RADV (Mesa 26.1.8), Linux 6.18, release build, the same headless sway at
// 1920x1080, design mode off. Each target ran the Rust Plasma example, which
// published 60 fps on 14% of a core at 1920x1080 and 51% at 3840x2160. Mean
// `debug.frames` ms over three runs of 3 s. The first five rows are 1920x1080
// targets, the rest 3840x2160 ones, with a third target at whatever size
// makes up the megapixels:
//
//   megapixels   targets   mean ms   worst ms   fps
//   2.1          1         1.1       2.0        60
//   4.1          2         1.8       3.0        62
//   6.2          3         2.5       6.4        62
//   8.3          4         3.2       5.1        62
//   12.4         6         5.0       8.8        61
//   8.3          1         3.0       4.8        60
//   16.6         2         6.6       11.6       62
//   17.5         3         7.1       11.1       62
//   18.7         3         8.3       12.0       62
//   19.4         3         8.7       13.3       62
//   20.3         3         9.7       16.6       62
//   22.3         3         17.5      29.2       55
//   24.9         3         20.6      28.8       47
//   33.2         4         28.0      75.3       31
//
// At 33.2 the renderers fell short of 60 on nearly a core each, so that row
// is partly theirs. Six more runs at 18.7 and three at 19.4 put the means at
// 8.2 and 8.8, and the threshold is read between them.
const TARGET_PIXELS: u64 = 18_800_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Weight {
    /// Summed over every canvas in the dump. `depth` is the deepest canvas in
    /// a canvas plus the groups at that level.
    pub cost: Cost,
    /// Panels, canvases and the window's own passes that wear a shader. A
    /// canvas's shader items are `cost.shaders`, not these.
    pub shader_surfaces: usize,
    /// Grid items a route moves, a routed group's grids included.
    pub moving_grids: usize,
    /// Switched-on time routes. One is enough to keep a canvas drawing every
    /// frame while it shows.
    pub time_routes: usize,
}

impl Weight {
    /// Each count at the larger of the two.
    fn max(self, other: Weight) -> Weight {
        Weight {
            cost: Cost {
                items: self.cost.items.max(other.cost.items),
                grids: self.cost.grids.max(other.cost.grids),
                shaders: self.cost.shaders.max(other.cost.shaders),
                depth: self.cost.depth.max(other.cost.depth),
                targets: self.cost.targets.max(other.cost.targets),
                target_pixels: self.cost.target_pixels.max(other.cost.target_pixels),
            },
            shader_surfaces: self.shader_surfaces.max(other.shader_surfaces),
            moving_grids: self.moving_grids.max(other.moving_grids),
            time_routes: self.time_routes.max(other.time_routes),
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

/// An object's item, weighed as a canvas holding only it and its routes.
pub(crate) fn weigh_object(bundle: &ObjectBundle) -> Weight {
    weigh_dump(&canvas_node(json!({
        "items": [bundle.item],
        "routes": bundle.routes,
    })))
}

/// A canvas panel's own config, nested canvases included.
pub(crate) fn weigh_canvas(config: Value) -> Weight {
    weigh_dump(&canvas_node(config))
}

fn canvas_node(config: Value) -> Value {
    json!({ "panel_name": PANEL_NAME, "info": { "panel": config } })
}

fn weigh_dump(dump: &Value) -> Weight {
    let mut tally = Tally::default();
    find_canvases(dump, 0, &mut tally);

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

/// Every canvas panel in a dump, at the level the dump sits at, the way the
/// dump walkers recurse.
fn find_canvases(value: &Value, level: usize, tally: &mut Tally) {
    match value {
        Value::Object(map) => {
            if map.get("panel_name").and_then(Value::as_str) == Some(PANEL_NAME) {
                if let Some(config) = map.get("info").and_then(|info| info.get("panel")) {
                    canvas(config, level, tally);
                }
                return;
            }

            map.values()
                .for_each(|child| find_canvases(child, level, tally));
        }

        Value::Array(items) => items
            .iter()
            .for_each(|item| find_canvases(item, level, tally)),

        _ => {}
    }
}

fn canvas(config: &Value, level: usize, tally: &mut Tally) {
    let Ok(state) = CanvasState::deserialize(config) else {
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
    sum.depth = sum.depth.max(level + cost.depth);
    sum.targets += cost.targets;
    sum.target_pixels = sum.target_pixels.saturating_add(cost.target_pixels);

    tally.weight.time_routes += routes
        .iter()
        .filter(|route| route.enabled && route.time.is_some())
        .count();

    let routed: HashSet<&str> = routes
        .iter()
        .filter(|route| route.enabled)
        .filter_map(|route| route.target.rsplit_once('.').map(|(id, _)| id))
        .collect();

    items(&state.items, &routed, false, level, tally);
}

/// `level` counts the canvases and groups these items sit inside, so a canvas
/// in a grid here opens the next one, as it does when built.
fn items(
    items: &[CanvasItem],
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
                find_canvases(dock, level + 1, tally);
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
        "canvas-weight-counts",
        items = weight.cost.items as u64,
        grids = weight.cost.grids as u64,
        shaders = weight.cost.shaders as u64,
        surfaces = weight.shader_surfaces as u64,
        moving = weight.moving_grids as u64,
        depth = weight.cost.depth as u64
    )
}

/// The render targets' line under the counts, for a canvas that has any.
pub(crate) fn target_counts(weight: &Weight) -> Option<SharedString> {
    if weight.cost.targets == 0 {
        return None;
    }

    Some(rox_i18n::t!(
        "canvas-weight-targets",
        count = weight.cost.targets as u64,
        megapixels = megapixels(weight.cost.target_pixels)
    ))
}

/// The line saying a canvas animates for as long as it shows, for one with
/// a time route. Not a warning: nothing is over, it only never rests.
pub(crate) fn animates(weight: &Weight) -> Option<SharedString> {
    (weight.time_routes > 0)
        .then(|| rox_i18n::t!("canvas-weight-animates", count = weight.time_routes as u64))
}

/// The lines to show, empty when nothing is over.
pub(crate) fn warnings(weight: &Weight) -> Vec<SharedString> {
    let over = [
        (weight.cost.grids, GRIDS, "canvas-weight-many-grids"),
        (
            weight.shader_surfaces,
            SHADER_SURFACES,
            "canvas-weight-many-shaders",
        ),
    ];

    let mut lines: Vec<SharedString> = over
        .into_iter()
        .filter(|(count, threshold, _)| count >= threshold)
        .map(|(count, _, key)| rox_i18n::t!(key, count = count as u64))
        .collect();

    if weight.cost.target_pixels >= TARGET_PIXELS {
        lines.push(rox_i18n::t!(
            "canvas-weight-heavy-targets",
            megapixels = megapixels(weight.cost.target_pixels)
        ));
    }

    lines
}

fn megapixels(pixels: u64) -> String {
    rox_i18n::format::format_float(pixels as f64 / 1_000_000., 1)
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

    /// A dock holding one canvas panel with these items and routes.
    fn dock_with_canvas(items: Value, routes: Value) -> Value {
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
    fn a_layout_with_no_canvas_weighs_nothing() {
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
    fn a_canvas_inside_a_canvas_is_counted_and_deepens() {
        let inner = dock_with_canvas(json!([grid("leaf", plain_dock())]), json!([]));
        let dump = dock_with_canvas(json!([grid("outer", inner)]), json!([]));

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
        let weight = weigh(&dock_with_canvas(json!([group]), json!([])), &bare());

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

        let weight = weigh(&dock_with_canvas(items, routes), &bare());
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
            &dock_with_canvas(items, json!([route("a.x", false)])),
            &bare(),
        );

        assert_eq!(weight.cost.grids, 1);
        assert_eq!(weight.moving_grids, 0);
    }

    #[test]
    fn a_time_route_says_the_canvas_animates() {
        let items = json!([{ "id": "logo", "content": { "kind": "image", "asset": "a" } }]);
        let mut timed = route("logo.y", true);
        timed["time"] = json!({ "period": 4.0, "shape": "sine" });
        let mut off = route("logo.x", false);
        off["time"] = json!({});

        let weight = weigh(
            &dock_with_canvas(
                items.clone(),
                json!([timed, off, route("logo.opacity", true)]),
            ),
            &bare(),
        );
        assert_eq!(weight.time_routes, 1, "only the switched-on time route");
        assert!(animates(&weight).is_some());
        assert!(
            warnings(&weight).is_empty(),
            "animating isn't over anything"
        );

        let still = weigh(
            &dock_with_canvas(items, json!([route("logo.y", true)])),
            &bare(),
        );
        assert_eq!(still.time_routes, 0);
        assert!(animates(&still).is_none());
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
            &dock_with_canvas(json!([shader, grid("p", inner)]), json!([])),
            &bundle,
        );
        assert_eq!(weight.cost.shaders, 1);
        assert_eq!(
            weight.shader_surfaces, 2,
            "the shader panel and the screen pass"
        );
    }

    #[test]
    fn an_object_weighs_as_its_own_canvas() {
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

    fn target(id: &str, width: u32, height: u32) -> Value {
        json!({
            "id": id,
            "content": { "kind": "target", "owner": "plugin:bars", "width": width, "height": height },
        })
    }

    #[test]
    fn a_target_adds_its_megapixels() {
        let inner = dock_with_canvas(json!([target("deep", 3840, 2160)]), json!([]));
        let group = json!({
            "id": "g",
            "content": { "kind": "group", "items": [target("b", 1280, 720)] },
        });
        let items = json!([target("a", 1920, 1080), group, grid("nest", inner)]);

        let weight = weigh(&dock_with_canvas(items, json!([])), &bare());
        assert_eq!(
            weight.cost.targets, 3,
            "in a group and in a nested canvas too"
        );
        assert_eq!(
            weight.cost.target_pixels,
            1920 * 1080 + 1280 * 720 + 3840 * 2160
        );

        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let line = target_counts(&weight).expect("a line for a canvas with targets");
        assert!(line.contains("11.3"), "{line}");
        assert_eq!(target_counts(&Weight::default()), None);
        assert!(warnings(&weight).is_empty(), "under the threshold");

        // Two 4K targets and a 1080p one drew under half a frame. With
        // 2240x1260 in place of the 1080p one they didn't.
        let mut heavy = weight;
        heavy.cost.target_pixels = 2 * 3840 * 2160 + 1920 * 1080;
        assert!(warnings(&heavy).is_empty());
        heavy.cost.target_pixels = 2 * 3840 * 2160 + 2240 * 1260;
        assert_eq!(warnings(&heavy).len(), 1);
    }

    #[test]
    fn the_heaviest_layout_sets_each_count() {
        let light = dock_with_canvas(json!([grid("a", plain_dock())]), json!([]));
        let heavy = dock_with_canvas(
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
