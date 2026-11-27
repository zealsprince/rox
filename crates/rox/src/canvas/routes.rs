//! Signals on canvas items (ADR 33): the canvas's route targets and what a
//! route's factor does to an item this frame. A target is
//! `<item id>.<property>`. Opacity and scale take a share of the item's own
//! setting, the way every route knob does. Position and rotation take a
//! share of the item's sway, so a sway of zero leaves that axis still.
//! Every route on a target counts: sway shares add and setting shares
//! multiply.
//!
//! Nothing here touches the saved items. The offsets live in one [`Live`]
//! per routed item, built fresh each frame and handed to the projection.
//! While playback isn't running a canvas's [`Idle`] setting decides what
//! they are: none at all (Rest), or the last ones drawn (Hold). A time
//! route follows the canvas's own clock instead and ignores playback, so a
//! canvas with one keeps drawing while it shows, paced by
//! [`crate::canvas::pace`]. Reduce Motion holds time routes at rest.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use rox_canvas::{CanvasItem, CanvasState, Idle, Live};
use rox_panel_api::signal_ui::{self, RouteTargets};
use rox_viz::signal::{Route, SignalHub};

/// The properties a route can drive on every item, in display order.
pub(crate) const PROPERTIES: [&str; 8] = ["opacity", "scale", "x", "y", "z", "rx", "ry", "rz"];

/// How long routed items take to ease to their own settings when playback
/// stops, and back when it starts. Close to how long a band signal at the
/// default smoothing takes to fall most of the way once the feed goes quiet.
const REST_EASE: Duration = Duration::from_millis(600);

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
            Property::Opacity => "canvas-item-opacity",
            Property::Scale => "canvas-item-scale",
            Property::Position(axis) => [
                "canvas-item-position-x",
                "canvas-item-position-y",
                "canvas-item-position-z",
            ][axis],
            Property::Rotation(axis) => [
                "canvas-item-rotation-x",
                "canvas-item-rotation-y",
                "canvas-item-rotation-z",
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
    canvas: &'a CanvasState,
    pub live: HashMap<String, Live>,
}

impl<'a> Lives<'a> {
    pub(crate) fn new(canvas: &'a CanvasState) -> Self {
        Self {
            canvas,
            live: HashMap::new(),
        }
    }
}

impl RouteTargets for Lives<'_> {
    fn targets(&self) -> Vec<(String, String)> {
        let mut targets = Vec::new();
        self.canvas.walk(&mut |item, _| {
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
                        "canvas-route-target",
                        item = name.to_string(),
                        property = parsed.label()
                    )
                    .to_string(),
                ));
            }
        });
        targets
    }

    /// Every route on a field counts. A sway's shares add, so a time route
    /// and a music route on one axis move it by both. Opacity and scale
    /// shares multiply, the way a case's opacity composes with a route's,
    /// and hold at zero, since a share below it has no meaning there.
    fn apply(&mut self, id: &str, value: f32) {
        let Some((item_id, property)) = split(id) else {
            return;
        };
        let Some(item) = self.canvas.find(item_id) else {
            return;
        };

        let live = self.live.entry(item_id.to_string()).or_default();
        match property {
            Property::Opacity => live.opacity *= value.max(0.),
            Property::Scale => live.scale *= value.max(0.),
            Property::Position(axis) => live.position[axis] += value * item.sway.position[axis],
            Property::Rotation(axis) => live.rotation[axis] += value * item.sway.rotation[axis],
        }
    }
}

impl Lives<'_> {
    /// Past full an item can't get more opaque, so its opacity share stops
    /// where the product reaches one. Run once every route has applied, so
    /// the order the routes are listed in doesn't change the picture.
    pub(crate) fn finish(&mut self) {
        for (id, live) in self.live.iter_mut() {
            if let Some(item) = self.canvas.find(id)
                && item.opacity > 0.
            {
                live.opacity = live.opacity.min(1. / item.opacity);
            }
        }
    }
}

/// Whether a target's span is a share of its item's sway: a position or
/// rotation axis. Its span is signed, so a sway can swing both ways round
/// home.
pub(crate) fn sways(target: &str) -> bool {
    split(target).is_some_and(|(_, property)| {
        matches!(property, Property::Position(_) | Property::Rotation(_))
    })
}

/// Whether any route is switched on, which is what keeps a canvas listening.
pub(crate) fn running(routes: &[Route]) -> bool {
    routes.iter().any(|route| route.enabled)
}

/// Whether playback counts as running for a canvas's routes. Paused,
/// stopped and nothing loaded all read as not playing. A played-out queue
/// can still read as playing, so the end of the queue is checked too.
pub(crate) fn playback_running(playing: bool, ended: bool) -> bool {
    playing && !ended
}

/// Every routed item's offsets from these route outputs, eased `weight` of
/// the way from its own settings.
fn lives_from(
    canvas: &CanvasState,
    factors: &[(String, f32)],
    weight: f32,
) -> HashMap<String, Live> {
    if weight <= 0. {
        return HashMap::new();
    }

    let mut lives = Lives::new(canvas);
    for (target, factor) in factors {
        lives.apply(target, *factor);
    }
    lives.finish();

    if weight < 1. {
        for live in lives.live.values_mut() {
            *live = blend(&Live::default(), live, weight);
        }
    }
    lives.live
}

/// `from` carried `weight` of the way to `to`.
fn blend(from: &Live, to: &Live, weight: f32) -> Live {
    let mix = |a: f32, b: f32| a + (b - a) * weight;

    Live {
        position: std::array::from_fn(|axis| mix(from.position[axis], to.position[axis])),
        rotation: std::array::from_fn(|axis| mix(from.rotation[axis], to.rotation[axis])),
        scale: mix(from.scale, to.scale),
        opacity: mix(from.opacity, to.opacity),
    }
}

/// Every pair of offsets eased `weight` of the way from `from` to `to`. An
/// item missing from either side sits at its own settings there.
fn blend_all(
    from: &HashMap<String, Live>,
    to: HashMap<String, Live>,
    weight: f32,
) -> HashMap<String, Live> {
    let mut out = to;
    for (id, live) in out.iter_mut() {
        *live = blend(from.get(id).unwrap_or(&Live::default()), live, weight);
    }
    for (id, live) in from {
        out.entry(id.clone())
            .or_insert_with(|| blend(live, &Live::default(), weight));
    }
    out
}

/// What each route puts out this frame, by target. Reading them advances
/// the hub.
fn factors(routes: &[Route], hub: &SignalHub) -> Vec<(String, f32)> {
    struct Collect(Vec<(String, f32)>);

    impl RouteTargets for Collect {
        fn targets(&self) -> Vec<(String, String)> {
            Vec::new()
        }

        fn apply(&mut self, id: &str, value: f32) {
            self.0.push((id.to_string(), value));
        }
    }

    let mut out = Collect(Vec::new());
    signal_ui::apply_routes(routes, hub, &mut out);
    out.0
}

/// A canvas's routes against playback across frames: the ease between
/// routed and at rest, and what Hold keeps.
#[derive(Default)]
pub(crate) struct Motion {
    /// Playback as the last frame saw it, none before the first.
    running: Option<bool>,
    /// When playback last started or stopped, and how much of the routes
    /// showed at that moment.
    edge: Option<(Instant, f32)>,
    /// The offsets last drawn while playing. Empty until something plays,
    /// so a held canvas starts at rest.
    held: HashMap<String, Live>,
    /// When the canvas first posed: the zero of the clock time routes read.
    born: Option<Instant>,
}

/// One frame's offsets, and whether the canvas needs another frame.
pub(crate) struct Pose {
    pub lives: HashMap<String, Live>,
    /// A music route moves or eases: the next frame, as soon as it can.
    pub more: bool,
    /// A time route moves: a frame once the motion has gone far enough to
    /// show.
    pub drifts: bool,
}

impl Motion {
    /// How much of the routes shows at `now`: 0 at rest, 1 playing.
    fn weight(&self, now: Instant) -> f32 {
        let target = if self.running == Some(true) { 1. } else { 0. };
        let Some((at, from)) = self.edge else {
            return target;
        };

        let t = now.saturating_duration_since(at).as_secs_f32() / REST_EASE.as_secs_f32();
        if t >= 1. {
            return target;
        }

        let eased = t * t * (3. - 2. * t);
        from + (target - from) * eased
    }

    /// Starts an ease from wherever the last one stood when playback flips.
    fn note(&mut self, running: bool, now: Instant) {
        match self.running {
            Some(was) if was != running => {
                let weight = self.weight(now);
                self.running = Some(running);
                self.edge = Some((now, weight));
            }
            Some(_) => {}
            None => self.running = Some(running),
        }
    }

    /// Whether a player tick has anything to show: playback runs, or it
    /// flipped since the last frame.
    pub(crate) fn wants(&self, running: bool) -> bool {
        running || self.running != Some(running)
    }

    /// Every routed item's offsets this frame. `live` gives the route
    /// outputs and whether the signals are still moving, and is only called
    /// when the frame needs them, since reading the hub advances it.
    pub(crate) fn pose(
        &mut self,
        canvas: &CanvasState,
        running: bool,
        now: Instant,
        live: impl FnOnce() -> (Vec<(String, f32)>, bool),
    ) -> Pose {
        self.note(running, now);

        // Hold stops at once: the next start eases out of the held pose.
        if !running && canvas.idle == Idle::Hold {
            self.edge = None;
            return Pose {
                lives: self.held.clone(),
                more: false,
                drifts: false,
            };
        }

        let weight = self.weight(now);
        let easing = self.edge.is_some() && weight != if running { 1. } else { 0. };
        if !running && weight <= 0. {
            return Pose {
                lives: HashMap::new(),
                more: false,
                drifts: false,
            };
        }

        let (factors, moving) = live();
        let lives = match canvas.idle {
            Idle::Rest => lives_from(canvas, &factors, weight),
            Idle::Hold => blend_all(&self.held, lives_from(canvas, &factors, 1.), weight),
        };

        if running {
            self.held = lives.clone();
        }

        Pose {
            more: easing || (running && moving),
            drifts: false,
            lives,
        }
    }
}

/// [`Motion::pose`] against the hub, with the time routes laid over it.
/// `still` holds them at rest, for Reduce Motion.
pub(crate) fn pose(
    motion: &mut Motion,
    canvas: &CanvasState,
    routes: &[Route],
    running: bool,
    still: bool,
    hub: &SignalHub,
) -> Pose {
    let now = Instant::now();

    // A canvas with only time routes never reads the hub.
    let music = routes
        .iter()
        .any(|route| route.enabled && route.time.is_none());
    let pose = if music {
        motion.pose(canvas, running, now, || {
            let factors = factors(routes, hub);
            (factors, hub.live() || hub.settling())
        })
    } else {
        Pose {
            lives: HashMap::new(),
            more: false,
            drifts: false,
        }
    };

    let seconds = (!still).then(|| motion.clock(now));
    over_time(canvas, routes, seconds, pose)
}

thread_local! {
    /// `debug.canvas_clock`: every canvas's clock held at these seconds, so
    /// two builds can be compared on the same frame.
    static CLOCK_HELD: Cell<Option<f64>> = const { Cell::new(None) };
}

/// Holds every canvas's clock at `seconds`, or lets it run again.
pub(crate) fn hold_clock(seconds: Option<f64>) {
    CLOCK_HELD.set(seconds);
}

impl Motion {
    /// Seconds on this canvas's own clock, which starts at its first pose.
    fn clock(&mut self, now: Instant) -> f64 {
        let born = *self.born.get_or_insert(now);
        if let Some(held) = CLOCK_HELD.get() {
            return held;
        }
        now.saturating_duration_since(born).as_secs_f64()
    }
}

/// `pose` with every time route applied at `seconds`, at full weight
/// whatever playback does. On a field a music route also drives, the time
/// route composes with the music's eased offsets as any two routes do. One
/// switched on keeps the frames coming. No `seconds` holds them at rest and
/// asks for no frames.
fn over_time(canvas: &CanvasState, routes: &[Route], seconds: Option<f64>, pose: Pose) -> Pose {
    if !signal_ui::timed(routes) {
        return pose;
    }

    let mut lives = Lives::new(canvas);
    lives.live = pose.lives;
    match seconds {
        Some(seconds) => signal_ui::apply_time_routes(routes, seconds, &mut lives),
        None => at_rest(routes, &mut lives),
    }
    lives.finish();

    Pose {
        lives: lives.live,
        more: pose.more,
        drifts: seconds.is_some(),
    }
}

/// Every time route held still. Each wave sits where its cycle starts, at
/// its own phase, so the picture is the one the canvas opens on, staggered
/// as its author set the phases. An opacity takes the brighter of that and
/// its wave's middle, so nothing a fade carries is held invisible: Aero's
/// bubbles pair a rising saw with a triangle fade, and a bubble whose phase
/// sits near the start of its cycle would otherwise rest unseen.
fn at_rest(routes: &[Route], lives: &mut Lives) {
    for route in routes {
        let Some(wave) = route.time.filter(|_| route.enabled) else {
            continue;
        };

        let mut value = signal_ui::span(route, wave.at(0.));
        if fades(&route.target) {
            value = value.max(signal_ui::span(route, 0.5));
        }
        lives.apply(&route.target, value);
    }
}

fn fades(target: &str) -> bool {
    split(target).is_some_and(|(_, property)| property == Property::Opacity)
}

/// The offsets for one item, none when no route reaches it.
pub(crate) fn live_for(lives: &HashMap<String, Live>, item: &CanvasItem) -> Live {
    lives.get(&item.id).copied().unwrap_or_default()
}

/// Drops the routes whose item is gone. Every edit runs this, so deleting an
/// item takes its routes and its children's in the same undo step.
pub(crate) fn prune(routes: &mut Vec<Route>, canvas: &CanvasState) {
    routes.retain(|route| match route.target.rsplit_once('.') {
        Some((id, _)) => canvas.find(id).is_some(),
        None => true,
    });
}

/// Gives each position or rotation route new since `before` a sway on its
/// axis, where that axis has none.
pub(crate) fn sway_new(before: &[Route], after: &[Route], canvas: &mut CanvasState) {
    for route in after {
        if before.iter().any(|old| old.target == route.target) {
            continue;
        }
        let Some((id, property)) = split(&route.target) else {
            continue;
        };
        let Some(item) = canvas.find_mut(id) else {
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

/// Whether two route lists say the same thing.
pub(crate) fn same(a: &[Route], b: &[Route]) -> bool {
    a == b
}

/// One undo step's worth of a canvas: the items and the routes aimed at them.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub canvas: CanvasState,
    pub routes: Vec<Route>,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.canvas == other.canvas && same(&self.routes, &other.routes)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rox_canvas::{Content, Sway};
    use rox_viz::signal::{Curve, Shape, Wave};

    use super::*;
    use crate::canvas::edit::{self, History};

    fn item(id: &str) -> CanvasItem {
        CanvasItem {
            id: id.to_string(),
            ..CanvasItem::default()
        }
    }

    fn route(target: &str) -> Route {
        Route {
            target: target.to_string(),
            ..Route::default()
        }
    }

    fn canvas(items: Vec<CanvasItem>) -> CanvasState {
        CanvasState {
            items,
            ..CanvasState::default()
        }
    }

    #[test]
    fn a_route_scales_opacity_by_its_share() {
        let canvas = canvas(vec![CanvasItem {
            opacity: 0.8,
            ..item("logo")
        }]);
        let mut lives = Lives::new(&canvas);

        lives.apply("logo.opacity", 0.5);
        lives.finish();
        assert_eq!(lives.live["logo"].opacity, 0.5);

        // Overshoot stops where the item is fully opaque.
        let mut lives = Lives::new(&canvas);
        lives.apply("logo.opacity", 3.);
        lives.finish();
        assert_eq!(lives.live["logo"].opacity, 1.25);
    }

    #[test]
    fn a_position_route_with_no_sway_moves_nothing() {
        let canvas = canvas(vec![item("logo")]);
        let mut lives = Lives::new(&canvas);

        lives.apply("logo.x", 1.);
        lives.apply("logo.rz", 1.);
        assert_eq!(lives.live["logo"].position, [0.; 3]);
        assert_eq!(lives.live["logo"].rotation, [0.; 3]);
    }

    #[test]
    fn a_position_route_moves_by_its_share_of_the_sway() {
        let canvas = canvas(vec![CanvasItem {
            sway: Sway {
                position: [0., 40., 0.],
                rotation: [0., 0., 30.],
            },
            ..item("logo")
        }]);
        let mut lives = Lives::new(&canvas);

        lives.apply("logo.y", 0.5);
        lives.apply("logo.rz", 0.25);
        assert_eq!(lives.live["logo"].position, [0., 20., 0.]);
        assert_eq!(lives.live["logo"].rotation, [0., 0., 7.5]);
        assert_eq!(lives.live["logo"].scale, 1.);
    }

    #[test]
    fn an_unknown_target_does_nothing() {
        let canvas = canvas(vec![item("logo")]);
        let mut lives = Lives::new(&canvas);

        lives.apply("ghost.opacity", 0.2);
        lives.apply("logo.wobble", 0.2);
        lives.apply("logo", 0.2);
        assert!(lives.live.is_empty());
    }

    #[test]
    fn routes_on_one_field_add_their_sways_and_multiply_their_shares() {
        let canvas = canvas(vec![CanvasItem {
            opacity: 0.5,
            sway: Sway {
                position: [0., 40., 0.],
                rotation: [0., 0., 30.],
            },
            ..item("logo")
        }]);
        let mut lives = Lives::new(&canvas);

        lives.apply("logo.scale", 0.5);
        lives.apply("logo.scale", 1.5);
        lives.apply("logo.y", 0.5);
        lives.apply("logo.y", -0.25);
        lives.apply("logo.rz", 1.);
        lives.apply("logo.rz", 1.);
        lives.apply("logo.opacity", 4.);
        lives.apply("logo.opacity", 0.25);
        lives.finish();

        let live = lives.live["logo"];
        assert_eq!(live.scale, 0.75);
        assert_eq!(live.position[1], 10.);
        assert_eq!(live.rotation[2], 60.);
        assert_eq!(live.opacity, 1., "the product is capped, not each share");

        // Listed the other way round, the picture is the same.
        let mut lives = Lives::new(&canvas);
        lives.apply("logo.opacity", 0.25);
        lives.apply("logo.opacity", 4.);
        lives.finish();
        assert_eq!(lives.live["logo"].opacity, 1.);
    }

    #[test]
    fn a_share_below_zero_holds_opacity_and_scale_at_zero_and_swings_a_sway() {
        let canvas = canvas(vec![CanvasItem {
            sway: Sway {
                position: [40., 0., 0.],
                ..Sway::default()
            },
            ..item("logo")
        }]);
        let mut lives = Lives::new(&canvas);

        lives.apply("logo.opacity", -0.5);
        lives.apply("logo.scale", -2.);
        lives.apply("logo.x", -1.);
        let live = lives.live["logo"];
        assert_eq!(live.opacity, 0.);
        assert_eq!(live.scale, 0.);
        assert_eq!(live.position[0], -40.);
    }

    #[test]
    fn targets_list_every_item_in_tree_order() {
        let canvas = canvas(vec![
            item("a"),
            CanvasItem {
                content: Content::Group {
                    items: vec![item("b.1"), item("c")],
                },
                ..item("group")
            },
            item("d"),
        ]);
        let targets = Lives::new(&canvas).targets();

        let ids: Vec<&str> = targets
            .iter()
            .step_by(PROPERTIES.len())
            .map(|(id, _)| id.rsplit_once('.').map_or("", |(item, _)| item))
            .collect();
        assert_eq!(ids, ["a", "group", "b.1", "c", "d"]);
        assert_eq!(targets.len(), 5 * PROPERTIES.len());
        assert_eq!(targets[1].0, "a.scale");

        // A dotted id still resolves to its item.
        let mut lives = Lives::new(&canvas);
        lives.apply("b.1.opacity", 0.5);
        assert_eq!(lives.live["b.1"].opacity, 0.5);
    }

    #[test]
    fn deleting_an_item_takes_its_routes_and_undo_returns_them() {
        let mut now = Snapshot {
            canvas: canvas(vec![
                item("keep"),
                CanvasItem {
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
        edit::remove(&mut now.canvas, "group").expect("present");
        prune(&mut now.routes, &now.canvas);
        history.record(before.clone(), now.clone(), None, Instant::now());

        let targets: Vec<&str> = now.routes.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(targets, ["keep.opacity"]);

        let back = history.undo().expect("one step");
        assert!(back == before);
        assert_eq!(back.routes.len(), 3);
        assert!(back.canvas.find("inner").is_some());
    }

    #[test]
    fn a_new_axis_route_gets_a_sway_where_it_has_none() {
        let mut canvas = canvas(vec![
            item("a"),
            CanvasItem {
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

        sway_new(&[], &after, &mut canvas);
        let a = canvas.find("a").expect("present").sway;
        assert_eq!(a.position, [NEW_SWAY_UNITS, 0., 0.]);
        assert_eq!(a.rotation, [0., NEW_SWAY_DEGREES, 0.]);
        assert_eq!(canvas.find("b").expect("present").sway.position[0], 5.);
    }

    /// A logo with an opacity route at the default Quiet, silent, and a
    /// position route halfway up its sway.
    fn routed(idle: Idle) -> (CanvasState, Vec<(String, f32)>) {
        let canvas = CanvasState {
            idle,
            ..canvas(vec![CanvasItem {
                sway: Sway {
                    position: [40., 0., 0.],
                    ..Sway::default()
                },
                ..item("logo")
            }])
        };
        let factors = vec![
            ("logo.opacity".to_string(), 0.),
            ("logo.x".to_string(), 0.5),
        ];
        (canvas, factors)
    }

    #[test]
    fn at_rest_an_item_has_no_offsets_once_the_ease_is_done() {
        let (canvas, factors) = routed(Idle::Rest);
        let mut motion = Motion::default();
        let start = Instant::now();

        let playing = motion.pose(&canvas, true, start, || (factors.clone(), true));
        assert_eq!(playing.lives["logo"].opacity, 0.);
        assert_eq!(playing.lives["logo"].position[0], 20.);

        // Partway: on its way back to its own settings, and asking for more.
        let paused = start + Duration::from_millis(10);
        motion.pose(&canvas, false, paused, || (factors.clone(), false));
        let easing = motion.pose(&canvas, false, paused + REST_EASE / 2, || {
            (factors.clone(), false)
        });
        let live = easing.lives["logo"];
        assert!(live.opacity > 0. && live.opacity < 1.);
        assert!(live.position[0] > 0. && live.position[0] < 20.);
        assert!(easing.more);

        let rested = motion.pose(&canvas, false, paused + REST_EASE, || {
            panic!("a canvas at rest doesn't read the hub")
        });
        assert!(rested.lives.is_empty());
        assert!(!rested.more);
    }

    #[test]
    fn resuming_from_rest_eases_back_in() {
        let (canvas, factors) = routed(Idle::Rest);
        let mut motion = Motion::default();
        let start = Instant::now();

        motion.pose(&canvas, false, start, || (factors.clone(), false));
        let resumed = start + Duration::from_millis(10);
        motion.pose(&canvas, true, resumed, || (factors.clone(), true));

        let halfway = motion.pose(&canvas, true, resumed + REST_EASE / 2, || {
            (factors.clone(), true)
        });
        assert!(halfway.lives["logo"].position[0] < 20.);

        let back = motion.pose(&canvas, true, resumed + REST_EASE, || {
            (factors.clone(), true)
        });
        assert_eq!(back.lives["logo"].position[0], 20.);
    }

    #[test]
    fn held_an_item_keeps_its_last_offsets_and_asks_for_no_frames() {
        let (canvas, factors) = routed(Idle::Hold);
        let mut motion = Motion::default();
        let start = Instant::now();

        let playing = motion.pose(&canvas, true, start, || (factors.clone(), true));
        let held = motion.pose(&canvas, false, start + Duration::from_millis(10), || {
            panic!("a held canvas doesn't read the hub")
        });
        assert_eq!(held.lives["logo"], playing.lives["logo"]);
        assert!(!held.more);

        let later = motion.pose(&canvas, false, start + REST_EASE * 10, || {
            panic!("a held canvas doesn't read the hub")
        });
        assert_eq!(later.lives["logo"], playing.lives["logo"]);
    }

    #[test]
    fn a_held_canvas_with_nothing_played_yet_starts_at_rest() {
        let (canvas, _) = routed(Idle::Hold);
        let held = Motion::default().pose(&canvas, false, Instant::now(), || {
            panic!("a held canvas doesn't read the hub")
        });
        assert!(held.lives.is_empty());
        assert!(!held.more);
    }

    #[test]
    fn a_canvas_with_no_routes_is_unaffected() {
        let canvas = canvas(vec![item("logo")]);
        assert!(!running(&[]));

        for idle in [Idle::Rest, Idle::Hold] {
            let canvas = CanvasState {
                idle,
                ..canvas.clone()
            };
            let mut motion = Motion::default();
            let start = Instant::now();

            let playing = motion.pose(&canvas, true, start, || (Vec::new(), false));
            assert!(playing.lives.is_empty());

            let paused = start + Duration::from_millis(10);
            let stopped = motion.pose(&canvas, false, paused, || (Vec::new(), false));
            assert!(stopped.lives.is_empty());

            let settled = motion.pose(&canvas, false, paused + REST_EASE, || (Vec::new(), false));
            assert!(settled.lives.is_empty());
            assert!(!settled.more);
        }
    }

    #[test]
    fn the_default_is_rest_and_isnt_written_to_the_saved_config() {
        use crate::panels::canvas::CanvasConfig;

        let config = CanvasConfig::default();
        assert_eq!(config.canvas.idle, Idle::Rest);
        let saved = serde_json::to_value(&config).expect("serializes");
        assert!(saved.get("idle").is_none());

        let read: CanvasConfig = serde_json::from_value(serde_json::json!({})).expect("parses");
        assert_eq!(read.canvas.idle, Idle::Rest);

        let mut held = config;
        held.canvas.idle = Idle::Hold;
        let saved = serde_json::to_value(&held).expect("serializes");
        assert_eq!(saved["idle"], "hold");
    }

    #[test]
    fn a_player_tick_redraws_only_while_playing_or_on_a_flip() {
        let (canvas, factors) = routed(Idle::Rest);
        let mut motion = Motion::default();
        assert!(motion.wants(false));

        motion.pose(&canvas, true, Instant::now(), || (factors.clone(), true));
        assert!(motion.wants(true));
        assert!(motion.wants(false));

        motion.pose(&canvas, false, Instant::now(), || (factors.clone(), false));
        assert!(!motion.wants(false));
    }

    fn drifting(enabled: bool) -> (CanvasState, Vec<Route>) {
        let canvas = canvas(vec![CanvasItem {
            sway: Sway {
                position: [0., 40., 0.],
                ..Sway::default()
            },
            ..item("logo")
        }]);
        let routes = vec![Route {
            enabled,
            time: Some(Wave {
                period: 2.,
                shape: Shape::Saw,
                phase: 0.,
            }),
            ..route("logo.y")
        }];
        (canvas, routes)
    }

    fn still() -> Pose {
        Pose {
            lives: HashMap::new(),
            more: false,
            drifts: false,
        }
    }

    #[test]
    fn a_time_route_moves_its_item_on_the_canvas_clock() {
        let (canvas, mut routes) = drifting(true);

        let quarter = over_time(&canvas, &routes, Some(0.5), still());
        assert_eq!(quarter.lives["logo"].position[1], 10.);
        assert!(quarter.drifts, "a time route keeps the canvas drawing");
        assert!(!quarter.more, "and paces its own frames");
        let half = over_time(&canvas, &routes, Some(1.), still());
        assert_eq!(half.lives["logo"].position[1], 20.);

        // A curve bends a time route as it does a music one.
        routes[0].curve = Curve::EaseIn;
        let bent = over_time(&canvas, &routes, Some(1.), still());
        assert_eq!(bent.lives["logo"].position[1], 10.);
    }

    #[test]
    fn a_time_route_adds_to_a_music_route_on_the_same_field() {
        let (canvas, routes) = drifting(true);
        let music = || Pose {
            lives: HashMap::from([(
                "logo".to_string(),
                Live {
                    position: [0., 8., 0.],
                    opacity: 0.5,
                    ..Live::default()
                },
            )]),
            more: true,
            drifts: false,
        };

        // A saw a quarter in, 10 of the 40 unit sway, on top of the music's 8.
        let both = over_time(&canvas, &routes, Some(0.5), music());
        assert_eq!(both.lives["logo"].position[1], 18.);
        assert_eq!(both.lives["logo"].opacity, 0.5, "the time route leaves it");
        assert!(
            both.more && both.drifts,
            "music asks every frame, time paces"
        );

        // Held still, the time route rests at its phase and the music's
        // offset is still there under it.
        let rested = over_time(&canvas, &routes, None, music());
        assert_eq!(rested.lives["logo"].position[1], 8.);
        assert!(rested.more && !rested.drifts);

        // A fade multiplies the music's share.
        let fade = Route {
            from: 0.5,
            time: Some(Wave {
                period: 2.,
                shape: Shape::Saw,
                phase: 0.,
            }),
            ..route("logo.opacity")
        };
        let faded = over_time(&canvas, &[fade], Some(1.), music());
        assert!((faded.lives["logo"].opacity - 0.375).abs() < 1e-6);
    }

    #[test]
    fn a_span_from_minus_one_swings_round_home() {
        let (canvas, mut routes) = drifting(true);
        routes[0].from = -1.;
        routes[0].time = Some(Wave {
            period: 4.,
            shape: Shape::Sine,
            phase: 0.,
        });

        // A sine over a 40 unit sway: -40 at the start, home a quarter in,
        // 40 halfway, home again three quarters in.
        let at = |seconds: f64| over_time(&canvas, &routes, Some(seconds), still()).lives["logo"];
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(at(0.).position[1], -40.));
        assert!(near(at(1.).position[1], 0.));
        assert!(near(at(2.).position[1], 40.));
        assert!(near(at(3.).position[1], 0.));
    }

    #[test]
    fn a_time_route_runs_whatever_playback_does_and_reads_no_hub() {
        let (canvas, routes) = drifting(true);
        let hub = SignalHub::unfed(Vec::new());

        for idle in [Idle::Rest, Idle::Hold] {
            let canvas = CanvasState {
                idle,
                ..canvas.clone()
            };
            let mut motion = Motion::default();
            for running in [false, true] {
                let posed = pose(&mut motion, &canvas, &routes, running, false, &hub);
                assert!(posed.drifts);
                assert!(posed.lives.contains_key("logo"));
            }
        }

        // Switched off, it asks for nothing, so the canvas rests again.
        let (canvas, routes) = drifting(false);
        let posed = pose(&mut Motion::default(), &canvas, &routes, false, false, &hub);
        assert!(!posed.more && !posed.drifts);
        assert!(posed.lives.is_empty());
    }

    #[test]
    fn held_still_a_time_route_rests_at_its_phase_and_asks_for_no_frames() {
        let (canvas, mut routes) = drifting(true);
        routes[0].time = Some(Wave {
            phase: 0.25,
            ..routes[0].time.unwrap()
        });
        let hub = SignalHub::unfed(Vec::new());

        let mut motion = Motion::default();
        for _ in 0..2 {
            let posed = pose(&mut motion, &canvas, &routes, false, true, &hub);
            assert!(!posed.more && !posed.drifts);
            // A saw a quarter in, of a 40 unit sway, wherever the clock is.
            assert_eq!(posed.lives["logo"].position[1], 10.);
        }
    }

    #[test]
    fn held_still_a_fade_never_rests_invisible() {
        let canvas = canvas(vec![item("bubble")]);
        let fade = |phase: f32| Route {
            time: Some(Wave {
                period: 40.,
                shape: Shape::Triangle,
                phase,
            }),
            ..route("bubble.opacity")
        };

        // At the start of its cycle a triangle reads 0, which would hide it.
        let rested = over_time(&canvas, &[fade(0.)], None, still());
        assert_eq!(rested.lives["bubble"].opacity, 0.5);
        assert!(!rested.drifts);

        // Brighter than the middle already, it keeps its own reading.
        let lit = over_time(&canvas, &[fade(0.4)], None, still());
        assert!((lit.lives["bubble"].opacity - 0.8).abs() < 1e-6);

        // A position is never pulled to the middle: it rests at its phase.
        let (canvas, routes) = drifting(true);
        let rested = over_time(&canvas, &routes, None, still());
        assert_eq!(rested.lives["logo"].position[1], 0.);
    }

    #[test]
    fn a_change_to_a_wave_or_a_curve_is_an_edit() {
        let (_, routes) = drifting(true);
        let mut changed = routes.clone();
        assert!(same(&routes, &changed));

        changed[0].time = Some(Wave::default());
        assert!(!same(&routes, &changed));

        let mut bent = routes.clone();
        bent[0].curve = Curve::Power(3.);
        assert!(!same(&routes, &bent));
    }

    #[test]
    fn paused_stopped_and_a_played_out_queue_all_count_as_not_running() {
        assert!(playback_running(true, false));
        assert!(!playback_running(false, false));
        assert!(!playback_running(true, true));
        assert!(!playback_running(false, true));
    }
}
