//! Signals on scene items (ADR 33): the scene's route targets and what a
//! route's factor does to an item this frame. A target is
//! `<item id>.<property>`. Opacity and scale take a share of the item's own
//! setting, the way every route knob does. Position and rotation take a
//! share of the item's sway, so a sway of zero leaves that axis still.
//!
//! Nothing here touches the saved items. The offsets live in one [`Live`]
//! per routed item, built fresh each frame and handed to the projection.

use std::collections::HashMap;

use rox_panel_api::signal_ui::{self, RouteTargets};
use rox_scene::{Live, SceneItem, SceneState};
use rox_viz::signal::{Route, SignalHub};

/// The properties a route can drive on every item, in display order.
pub(crate) const PROPERTIES: [&str; 8] = ["opacity", "scale", "x", "y", "z", "rx", "ry", "rz"];

/// What a new position or rotation route starts its axis at when the sway
/// there is zero, so a fresh binding visibly does something.
const NEW_SWAY_UNITS: f32 = 40.;
const NEW_SWAY_DEGREES: f32 = 15.;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Property {
    Opacity,
    Scale,
    Position(usize),
    Rotation(usize),
}

impl Property {
    fn parse(name: &str) -> Option<Property> {
        Some(match name {
            "opacity" => Property::Opacity,
            "scale" => Property::Scale,
            "x" => Property::Position(0),
            "y" => Property::Position(1),
            "z" => Property::Position(2),
            "rx" => Property::Rotation(0),
            "ry" => Property::Rotation(1),
            "rz" => Property::Rotation(2),
            _ => return None,
        })
    }

    fn label(self) -> String {
        let key = match self {
            Property::Opacity => "scene-item-opacity",
            Property::Scale => "scene-item-scale",
            Property::Position(axis) => [
                "scene-item-position-x",
                "scene-item-position-y",
                "scene-item-position-z",
            ][axis],
            Property::Rotation(axis) => [
                "scene-item-rotation-x",
                "scene-item-rotation-y",
                "scene-item-rotation-z",
            ][axis],
        };
        rox_i18n::t!(key).to_string()
    }
}

/// The item id and property a target names. Ids may hold dots, so the
/// property is whatever follows the last one.
fn split(target: &str) -> Option<(&str, Property)> {
    let (id, property) = target.rsplit_once('.')?;
    Some((id, Property::parse(property)?))
}

/// This frame's offsets, by item id. Items no route reaches have no entry.
pub(crate) struct Lives<'a> {
    scene: &'a SceneState,
    pub live: HashMap<String, Live>,
}

impl<'a> Lives<'a> {
    pub(crate) fn new(scene: &'a SceneState) -> Self {
        Self {
            scene,
            live: HashMap::new(),
        }
    }
}

impl RouteTargets for Lives<'_> {
    fn targets(&self) -> Vec<(String, String)> {
        let mut targets = Vec::new();
        self.scene.walk(&mut |item, _| {
            let name = if item.name.is_empty() {
                item.id.as_str()
            } else {
                item.name.as_str()
            };
            for property in PROPERTIES {
                let Some(parsed) = Property::parse(property) else {
                    continue;
                };
                targets.push((
                    format!("{}.{property}", item.id),
                    rox_i18n::t!(
                        "scene-route-target",
                        item = name.to_string(),
                        property = parsed.label()
                    )
                    .to_string(),
                ));
            }
        });
        targets
    }

    fn apply(&mut self, id: &str, value: f32) {
        let Some((item_id, property)) = split(id) else {
            return;
        };
        let Some(item) = self.scene.find(item_id) else {
            return;
        };

        let live = self.live.entry(item_id.to_string()).or_default();
        match property {
            // Past full the item can't get more opaque, so the share stops
            // where the product reaches one.
            Property::Opacity if item.opacity > 0. => live.opacity = value.min(1. / item.opacity),
            Property::Opacity => live.opacity = value,
            Property::Scale => live.scale = value,
            Property::Position(axis) => live.position[axis] = value * item.sway.position[axis],
            Property::Rotation(axis) => live.rotation[axis] = value * item.sway.rotation[axis],
        }
    }
}

/// Whether any route is switched on, which is what keeps a scene listening.
pub(crate) fn running(routes: &[Route]) -> bool {
    routes.iter().any(|route| route.enabled)
}

/// Every routed item's offsets this frame.
pub(crate) fn lives(
    scene: &SceneState,
    routes: &[Route],
    hub: &SignalHub,
) -> HashMap<String, Live> {
    let mut lives = Lives::new(scene);
    signal_ui::apply_routes(routes, hub, &mut lives);
    lives.live
}

/// The offsets for one item, none when no route reaches it.
pub(crate) fn live_for(lives: &HashMap<String, Live>, item: &SceneItem) -> Live {
    lives.get(&item.id).copied().unwrap_or_default()
}

/// Drops the routes whose item is gone. Every edit runs this, so deleting an
/// item takes its routes and its children's in the same undo step.
pub(crate) fn prune(routes: &mut Vec<Route>, scene: &SceneState) {
    routes.retain(|route| match route.target.rsplit_once('.') {
        Some((id, _)) => scene.find(id).is_some(),
        None => true,
    });
}

/// Gives each position or rotation route new since `before` a sway on its
/// axis, where that axis has none.
pub(crate) fn sway_new(before: &[Route], after: &[Route], scene: &mut SceneState) {
    for route in after {
        if before.iter().any(|old| old.target == route.target) {
            continue;
        }
        let Some((id, property)) = split(&route.target) else {
            continue;
        };
        let Some(item) = scene.find_mut(id) else {
            continue;
        };

        match property {
            Property::Position(axis) if item.sway.position[axis] == 0. => {
                item.sway.position[axis] = NEW_SWAY_UNITS;
            }
            Property::Rotation(axis) if item.sway.rotation[axis] == 0. => {
                item.sway.rotation[axis] = NEW_SWAY_DEGREES;
            }
            _ => {}
        }
    }
}

/// Whether two route lists say the same thing. `Route` has no `PartialEq`.
pub(crate) fn same(a: &[Route], b: &[Route]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.enabled == b.enabled
                && a.signal == b.signal
                && a.target == b.target
                && a.from == b.from
                && a.to == b.to
        })
}

/// One undo step's worth of a scene: the items and the routes aimed at them.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub scene: SceneState,
    pub routes: Vec<Route>,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.scene == other.scene && same(&self.routes, &other.routes)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rox_scene::{Content, Sway};

    use super::*;
    use crate::scene::edit::{self, History};

    fn item(id: &str) -> SceneItem {
        SceneItem {
            id: id.to_string(),
            ..SceneItem::default()
        }
    }

    fn route(target: &str) -> Route {
        Route {
            target: target.to_string(),
            ..Route::default()
        }
    }

    fn scene(items: Vec<SceneItem>) -> SceneState {
        SceneState {
            items,
            ..SceneState::default()
        }
    }

    #[test]
    fn a_route_scales_opacity_by_its_share() {
        let scene = scene(vec![SceneItem {
            opacity: 0.8,
            ..item("logo")
        }]);
        let mut lives = Lives::new(&scene);

        lives.apply("logo.opacity", 0.5);
        assert_eq!(lives.live["logo"].opacity, 0.5);

        // Overshoot stops where the item is fully opaque.
        lives.apply("logo.opacity", 3.);
        assert_eq!(lives.live["logo"].opacity, 1.25);
    }

    #[test]
    fn a_position_route_with_no_sway_moves_nothing() {
        let scene = scene(vec![item("logo")]);
        let mut lives = Lives::new(&scene);

        lives.apply("logo.x", 1.);
        lives.apply("logo.rz", 1.);
        assert_eq!(lives.live["logo"].position, [0.; 3]);
        assert_eq!(lives.live["logo"].rotation, [0.; 3]);
    }

    #[test]
    fn a_position_route_moves_by_its_share_of_the_sway() {
        let scene = scene(vec![SceneItem {
            sway: Sway {
                position: [0., 40., 0.],
                rotation: [0., 0., 30.],
            },
            ..item("logo")
        }]);
        let mut lives = Lives::new(&scene);

        lives.apply("logo.y", 0.5);
        lives.apply("logo.rz", 0.25);
        assert_eq!(lives.live["logo"].position, [0., 20., 0.]);
        assert_eq!(lives.live["logo"].rotation, [0., 0., 7.5]);
        assert_eq!(lives.live["logo"].scale, 1.);
    }

    #[test]
    fn an_unknown_target_does_nothing() {
        let scene = scene(vec![item("logo")]);
        let mut lives = Lives::new(&scene);

        lives.apply("ghost.opacity", 0.2);
        lives.apply("logo.wobble", 0.2);
        lives.apply("logo", 0.2);
        assert!(lives.live.is_empty());
    }

    #[test]
    fn a_later_route_to_the_same_target_wins() {
        let scene = scene(vec![item("logo")]);
        let mut lives = Lives::new(&scene);

        lives.apply("logo.scale", 0.3);
        lives.apply("logo.scale", 0.7);
        assert_eq!(lives.live["logo"].scale, 0.7);
    }

    #[test]
    fn targets_list_every_item_in_tree_order() {
        let scene = scene(vec![
            item("a"),
            SceneItem {
                content: Content::Group {
                    items: vec![item("b.1"), item("c")],
                },
                ..item("group")
            },
            item("d"),
        ]);
        let targets = Lives::new(&scene).targets();

        let ids: Vec<&str> = targets
            .iter()
            .step_by(PROPERTIES.len())
            .map(|(id, _)| id.rsplit_once('.').map_or("", |(item, _)| item))
            .collect();
        assert_eq!(ids, ["a", "group", "b.1", "c", "d"]);
        assert_eq!(targets.len(), 5 * PROPERTIES.len());
        assert_eq!(targets[1].0, "a.scale");

        // A dotted id still resolves to its item.
        let mut lives = Lives::new(&scene);
        lives.apply("b.1.opacity", 0.5);
        assert_eq!(lives.live["b.1"].opacity, 0.5);
    }

    #[test]
    fn deleting_an_item_takes_its_routes_and_undo_returns_them() {
        let mut now = Snapshot {
            scene: scene(vec![
                item("keep"),
                SceneItem {
                    content: Content::Group {
                        items: vec![item("inner")],
                    },
                    ..item("group")
                },
            ]),
            routes: vec![route("keep.opacity"), route("group.y"), route("inner.rz")],
        };
        let mut history = History::default();

        // The way the panel's one edit path runs a delete.
        let before = now.clone();
        edit::remove(&mut now.scene, "group").expect("present");
        prune(&mut now.routes, &now.scene);
        history.record(before.clone(), now.clone(), None, Instant::now());

        let targets: Vec<&str> = now.routes.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(targets, ["keep.opacity"]);

        let back = history.undo().expect("one step");
        assert!(back == before);
        assert_eq!(back.routes.len(), 3);
        assert!(back.scene.find("inner").is_some());
    }

    #[test]
    fn a_new_axis_route_gets_a_sway_where_it_has_none() {
        let mut scene = scene(vec![
            item("a"),
            SceneItem {
                sway: Sway {
                    position: [5., 0., 0.],
                    ..Sway::default()
                },
                ..item("b")
            },
        ]);
        let after = [
            route("a.x"),
            route("a.ry"),
            route("a.opacity"),
            route("b.x"),
        ];

        sway_new(&[], &after, &mut scene);
        let a = scene.find("a").expect("present").sway;
        assert_eq!(a.position, [NEW_SWAY_UNITS, 0., 0.]);
        assert_eq!(a.rotation, [0., NEW_SWAY_DEGREES, 0.]);
        assert_eq!(scene.find("b").expect("present").sway.position[0], 5.);
    }
}
