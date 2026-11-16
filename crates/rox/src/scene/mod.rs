//! The parts of a scene panel (ADR 33) below the panel itself: the live
//! bodies kept beside a scene's saved items, the nesting cap, and the grid,
//! image, shader and texture items. [`crate::panels::scene`] owns them and
//! draws the result.
//!
//! The saved items stay the source of truth. A body holds only what can't be
//! saved, a grid's live dock area, so an item with no body here (text,
//! video, model, or one that didn't build) is kept exactly as it was read
//! and written back the same.

pub(crate) mod edit;
pub(crate) mod grid;
pub(crate) mod image;
pub(crate) mod overlay;
pub(crate) mod routes;
pub(crate) mod shader;
pub(crate) mod texture;

use std::cell::Cell;
use std::collections::HashMap;

use rox_scene::{Camera, Content, MAX_DEPTH, SceneItem, Viewport};
use serde_json::Value;

/// What one saved item holds live, at the same index as the item.
pub(crate) enum Body<G> {
    Grid(G),
    Group(Vec<Body<G>>),
    Image,
    Shader,
    /// Built into nothing: a kind with no body yet, a grid whose dump
    /// didn't restore, or a container past the nesting cap.
    Unknown,
}

/// What every item in one frame is projected with.
pub(crate) struct Placing {
    pub camera: Camera,
    pub viewport: Viewport,
    /// Panel-relative, and only while the camera has parallax.
    pub pointer: Option<[f32; 2]>,
    pub design: bool,
}

thread_local! {
    /// The level of the grid whose dock is restoring right now, if any.
    static RESTORING: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Held around a grid's restore, so a scene the registry builds inside it
/// knows how deep it sits. Restores the outer value on drop, a panic
/// included.
pub(crate) struct Nesting(Option<usize>);

impl Nesting {
    pub(crate) fn enter(level: usize) -> Nesting {
        Nesting(RESTORING.replace(Some(level)))
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        RESTORING.set(self.0);
    }
}

/// The level a scene built now starts at: 0 on its own, one past the grid
/// restoring it.
pub(crate) fn level_here() -> usize {
    RESTORING.get().map_or(0, |level| level + 1)
}

/// Every item's body. `level` counts the scenes and groups these items sit
/// inside, and nothing past [`MAX_DEPTH`] is built, so a hand-edited layout
/// can't nest without end. `grid` builds one grid at the level its item
/// sits at; None keeps the item as saved.
pub(crate) fn build_bodies<G>(
    items: &[SceneItem],
    level: usize,
    grid: &mut impl FnMut(&SceneItem, &Value, usize) -> Option<G>,
) -> Vec<Body<G>> {
    items
        .iter()
        .map(|item| match &item.content {
            Content::Grid { dock } => {
                // Only a scene inside the grid opens a level, so a grid of
                // plain panels still builds at the cap.
                if level >= MAX_DEPTH && holds_scene(dock) {
                    log::warn!(
                        "scene: grid {} holds a scene past {MAX_DEPTH} levels, keeping it as saved",
                        item.id
                    );
                    return Body::Unknown;
                }

                grid(item, dock, level).map_or(Body::Unknown, Body::Grid)
            }

            Content::Group { items } => {
                if level >= MAX_DEPTH {
                    log::warn!(
                        "scene: group {} nests past {MAX_DEPTH} levels, keeping it as saved",
                        item.id
                    );
                    return Body::Unknown;
                }

                Body::Group(build_bodies(items, level + 1, grid))
            }

            Content::Image { .. } => Body::Image,

            Content::Shader { .. } => Body::Shader,

            _ => Body::Unknown,
        })
        .collect()
}

/// Whether a dock dump holds a scene panel anywhere, a composite's slots
/// included.
pub(crate) fn holds_scene(dump: &Value) -> bool {
    match dump {
        Value::Object(map) => {
            map.get("panel_name").and_then(Value::as_str) == Some(crate::panels::scene::PANEL_NAME)
                || map.values().any(holds_scene)
        }
        Value::Array(items) => items.iter().any(holds_scene),
        _ => false,
    }
}

/// Write each live grid's dump over its saved one. An item with no live
/// grid keeps what it was saved with.
pub(crate) fn fold_dumps<G>(
    items: &mut [SceneItem],
    bodies: &[Body<G>],
    dump: &impl Fn(&G) -> Option<Value>,
) {
    for (item, body) in items.iter_mut().zip(bodies) {
        match (&mut item.content, body) {
            (Content::Grid { dock }, Body::Grid(grid)) => {
                if let Some(fresh) = dump(grid) {
                    *dock = fresh;
                }
            }
            (Content::Group { items }, Body::Group(bodies)) => fold_dumps(items, bodies, dump),
            _ => {}
        }
    }
}

/// Every live grid with its item, groups descended, in item order.
pub(crate) fn each_grid<'a, G>(
    items: &'a [SceneItem],
    bodies: &'a [Body<G>],
    f: &mut impl FnMut(&'a SceneItem, &'a G),
) {
    for (item, body) in items.iter().zip(bodies) {
        match (&item.content, body) {
            (_, Body::Grid(grid)) => f(item, grid),
            (Content::Group { items }, Body::Group(bodies)) => each_grid(items, bodies, f),
            _ => {}
        }
    }
}

/// Pull the live grids out by item id, so a rebuild after an edit keeps
/// them. The first of two items sharing an id keeps the grid.
pub(crate) fn take_grids<G>(
    items: &[SceneItem],
    bodies: Vec<Body<G>>,
    out: &mut HashMap<String, G>,
) {
    for (item, body) in items.iter().zip(bodies) {
        match (&item.content, body) {
            (_, Body::Grid(grid)) => {
                out.entry(item.id.clone()).or_insert(grid);
            }
            (Content::Group { items }, Body::Group(bodies)) => take_grids(items, bodies, out),
            _ => {}
        }
    }
}

/// The screen-aligned box around four corners, as its top-left and size.
pub(crate) fn bbox(corners: &[[f32; 2]; 4]) -> ([f32; 2], [f32; 2]) {
    let (mut min, mut max) = (corners[0], corners[0]);

    for [x, y] in corners {
        min = [min[0].min(*x), min[1].min(*y)];
        max = [max[0].max(*x), max[1].max(*y)];
    }

    (min, [max[0] - min[0], max[1] - min[1]])
}

/// Whether any of the box around `corners` falls inside the viewport.
pub(crate) fn on_stage(corners: &[[f32; 2]; 4], viewport: Viewport) -> bool {
    let (origin, extent) = bbox(corners);

    origin[0] < viewport.width
        && origin[1] < viewport.height
        && origin[0] + extent[0] > 0.
        && origin[1] + extent[1] > 0.
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_scene::SceneState;
    use serde_json::json;

    fn scene(items: Value) -> SceneState {
        serde_json::from_value(json!({ "items": items })).expect("a scene parses")
    }

    fn grid(id: &str, dock: Value) -> Value {
        json!({ "id": id, "content": { "kind": "grid", "dock": dock } })
    }

    fn nested_scene_dump() -> Value {
        json!({
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [{
                    "panel_name": "TabPanel",
                    "children": [{ "panel_name": "scene", "children": [], "info": { "panel": {} } }],
                    "info": { "tabs": { "active_index": 0 } },
                }],
                "info": { "stack": { "sizes": [], "axis": 1 } },
            },
        })
    }

    /// Wraps `inner` in `levels` groups.
    fn grouped(levels: usize, inner: Value) -> Value {
        (0..levels).fold(inner, |inner, ix| {
            json!({ "id": format!("g{ix}"), "content": { "kind": "group", "items": [inner] } })
        })
    }

    #[test]
    fn an_unknown_item_survives_a_dump() {
        let text = json!({ "id": "t", "content": { "kind": "hologram", "beam": [1, 2, 3] } });
        let mut state = scene(json!([text.clone(), grid("a", json!({ "stale": true }))]));
        let bodies = build_bodies(&state.items, 0, &mut |_, _, _| Some(()));

        fold_dumps(&mut state.items, &bodies, &|_| {
            Some(json!({ "fresh": true }))
        });

        let saved = serde_json::to_value(&state).expect("serializes");
        assert_eq!(saved["items"][0]["content"], text["content"]);
        assert!(matches!(bodies[0], Body::Unknown));
        assert_eq!(
            saved["items"][1]["content"]["dock"],
            json!({ "fresh": true })
        );
    }

    #[test]
    fn a_scene_in_a_grid_stops_at_the_cap() {
        let at = |levels| {
            let state = scene(json!([grouped(levels, grid("deep", nested_scene_dump()))]));
            let mut built = 0;
            let bodies = build_bodies(&state.items, 0, &mut |_, _, _| {
                built += 1;
                Some(())
            });
            (bodies, built)
        };

        let (_, built) = at(MAX_DEPTH - 1);
        assert_eq!(built, 1, "one level under the cap still builds");

        let (_, built) = at(MAX_DEPTH);
        assert_eq!(built, 0, "a scene at the cap would open one more level");
    }

    #[test]
    fn a_grid_of_plain_panels_builds_at_the_cap() {
        let plain = json!({ "version": 1, "center": { "panel_name": "StackPanel" } });
        let state = scene(json!([grouped(MAX_DEPTH, grid("deep", plain))]));
        let mut built = 0;

        build_bodies(&state.items, 0, &mut |_, _, _| {
            built += 1;
            Some(())
        });

        assert_eq!(built, 1);
    }

    #[test]
    fn a_nested_scene_starts_its_count_where_the_grid_left_it() {
        let state = scene(json!([grid("a", nested_scene_dump())]));
        let bodies = build_bodies(&state.items, MAX_DEPTH, &mut |_, _, _| Some(()));
        assert!(matches!(bodies[0], Body::Unknown));

        assert_eq!(level_here(), 0);
        {
            let _outer = Nesting::enter(1);
            assert_eq!(level_here(), 2);
            {
                let _inner = Nesting::enter(3);
                assert_eq!(level_here(), 4);
            }
            assert_eq!(level_here(), 2);
        }
        assert_eq!(level_here(), 0);
    }

    #[test]
    fn grids_come_back_by_id_after_an_edit() {
        let state = scene(json!([
            grid("a", Value::Null),
            { "id": "g", "content": { "kind": "group", "items": [grid("b", Value::Null)] } },
        ]));
        let mut n = 0;
        let bodies = build_bodies(&state.items, 0, &mut |item, _, _| {
            n += 1;
            Some((item.id.clone(), n))
        });

        let mut seen = Vec::new();
        each_grid(&state.items, &bodies, &mut |item, grid| {
            seen.push((item.id.clone(), grid.1))
        });
        assert_eq!(seen, [("a".to_string(), 1), ("b".to_string(), 2)]);

        let mut spare = HashMap::new();
        take_grids(&state.items, bodies, &mut spare);
        assert_eq!(spare["b"], ("b".to_string(), 2));
        assert_eq!(spare.len(), 2);
    }

    #[test]
    fn only_a_box_over_the_viewport_is_on_stage() {
        let viewport = Viewport {
            width: 100.,
            height: 50.,
        };
        let square = |x: f32, y: f32| [[x, y], [x + 10., y], [x + 10., y + 10.], [x, y + 10.]];

        assert!(on_stage(&square(0., 0.), viewport));
        assert!(on_stage(&square(-5., 45.), viewport));
        assert!(!on_stage(&square(-10., 0.), viewport));
        assert!(!on_stage(&square(100., 0.), viewport));
        assert!(!on_stage(&square(20., 60.), viewport));
    }

    #[test]
    fn a_box_holds_every_corner() {
        let (origin, size) = bbox(&[[10., 5.], [30., 0.], [25., 40.], [0., 20.]]);
        assert_eq!(origin, [0., 0.]);
        assert_eq!(size, [30., 40.]);
    }
}
