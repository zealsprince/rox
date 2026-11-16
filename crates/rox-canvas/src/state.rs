//! A canvas's saved shape. Every struct reads with defaults, so a canvas saved
//! before a field existed still loads, and every item list reads leniently,
//! so one broken item costs only itself.

use std::cell::Cell;
use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub const CANVAS_VERSION: u32 = 1;

/// Canvases inside canvases and groups inside groups, counted together.
/// Placeholder.
pub const MAX_DEPTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasState {
    pub version: u32,
    pub stage: Stage,
    pub camera: Camera,
    /// Lenient: a broken item costs only itself.
    #[serde(deserialize_with = "lenient_items")]
    pub items: Vec<CanvasItem>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            version: CANVAS_VERSION,
            stage: Stage::default(),
            camera: Camera::default(),
            items: Vec::new(),
        }
    }
}

/// The size the canvas was laid out at.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stage {
    pub width: f32,
    pub height: f32,
    pub fit: Fit,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            width: 1920.,
            height: 1080.,
            fit: Fit::Anchor,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// One unit is one pixel. Items hold to their anchors as the panel
    /// resizes.
    #[default]
    Anchor,
    /// The stage scales to sit inside the panel.
    Contain,
    /// The stage scales to fill the panel.
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    /// Vertical, degrees. 0 is orthographic.
    pub fov: f32,
    /// How far the view shifts with the pointer, stage units. 0 is off.
    pub parallax: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov: 35.,
            parallax: 0.,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasItem {
    /// Unique in the whole canvas, groups included. Stable across saves.
    pub id: String,
    pub name: String,
    pub place: Place,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    /// How far a full signal carries the item. See Routes.
    #[serde(skip_serializing_if = "Sway::is_still")]
    pub sway: Sway,
    /// A command id run on click, empty for none. Ignored on a Grid.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
    pub content: Content,
}

impl Default for CanvasItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            place: Place::default(),
            visible: true,
            locked: false,
            opacity: 1.,
            sway: Sway::default(),
            action: String::new(),
            content: Content::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Place {
    /// Measured on the parent's box. The item's own matching point sits
    /// there. It only places the item, which turns and scales about its
    /// centre.
    pub anchor: Anchor,
    /// Units, in the parent's space, from the anchor's point.
    pub position: [f32; 3],
    pub size: [Length; 2],
    /// Degrees about x, y, z.
    pub rotation: [f32; 3],
    pub scale: f32,
}

impl Default for Place {
    fn default() -> Self {
        Self {
            anchor: Anchor::Center,
            position: [0.; 3],
            size: [Length::Units(480.), Length::Units(320.)],
            rotation: [0.; 3],
            scale: 1.,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sway {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
}

impl Sway {
    fn is_still(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Length {
    Units(f32),
    /// A fraction of the parent's box: the stage, or the group.
    Stage(f32),
}

impl Length {
    /// This length against the parent box's matching axis.
    pub fn resolve(self, parent_axis: f32) -> f32 {
        match self {
            Length::Units(n) => n,
            Length::Stage(f) => f * parent_axis,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// The point as a fraction of a box, x then y, y down.
    pub fn fraction(self) -> [f32; 2] {
        match self {
            Anchor::TopLeft => [0., 0.],
            Anchor::Top => [0.5, 0.],
            Anchor::TopRight => [1., 0.],
            Anchor::Left => [0., 0.5],
            Anchor::Center => [0.5, 0.5],
            Anchor::Right => [1., 0.5],
            Anchor::BottomLeft => [0., 1.],
            Anchor::Bottom => [0.5, 1.],
            Anchor::BottomRight => [1., 1.],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    #[default]
    Contain,
    Cover,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

// `remote = "Self"` turns the derives into inherent `Content::serialize` and
// `Content::deserialize`, which the trait impls below wrap to keep
// `Unknown` as raw JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Grid {
        dock: Value,
    },
    Group {
        #[serde(default, deserialize_with = "lenient_items")]
        items: Vec<CanvasItem>,
    },
    Image {
        asset: String,
        #[serde(default)]
        fit: ImageFit,
    },
    Text {
        text: String,
        #[serde(default)]
        font: Option<String>,
        size: f32,
        color: String,
        #[serde(default)]
        align: Align,
    },
    Shader {
        shader: Value,
    },
    /// A kind this build doesn't know, or a known kind whose body didn't
    /// parse. Serializes back to the value it was read from.
    #[serde(skip)]
    Unknown(Value),
}

const KINDS: [&str; 5] = ["grid", "group", "image", "text", "shader"];

impl Default for Content {
    fn default() -> Self {
        Content::Unknown(Value::Null)
    }
}

impl Content {
    /// The asset this content names, if it names one.
    pub fn asset(&self) -> Option<&str> {
        match self {
            Content::Image { asset, .. } => Some(asset.as_str()).filter(|a| !a.is_empty()),
            _ => None,
        }
    }
}

impl Serialize for Content {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Content::Unknown(value) => value.serialize(serializer),
            known => Content::serialize(known, serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let kind = value.get("kind").and_then(Value::as_str);

        // The cap is checked while reading, so a hand-edited canvas can't
        // nest without end, and the over-deep group stays exactly as saved.
        let _level = match kind {
            Some("group") => match GroupLevel::enter() {
                Some(level) => Some(level),
                None => {
                    log::warn!(
                        "canvas: a group nests past {MAX_DEPTH} levels, keeping it as saved"
                    );
                    return Ok(Content::Unknown(value));
                }
            },
            _ => None,
        };

        match Content::deserialize(&value) {
            Ok(content) => Ok(content),

            Err(e) => {
                match kind {
                    Some(kind) if KINDS.contains(&kind) => {
                        log::warn!("canvas: keeping a {kind} item whose body doesn't parse: {e}")
                    }
                    _ => log::debug!("canvas: keeping an item of a kind this build doesn't know"),
                }
                Ok(Content::Unknown(value))
            }
        }
    }
}

thread_local! {
    static GROUP_LEVEL: Cell<usize> = const { Cell::new(0) };
}

/// How many groups the reader is inside on this thread. Held for the length
/// of one group's parse and released on drop, a panic included.
struct GroupLevel;

impl GroupLevel {
    fn enter() -> Option<GroupLevel> {
        GROUP_LEVEL.with(|level| {
            if level.get() >= MAX_DEPTH {
                return None;
            }

            level.set(level.get() + 1);
            Some(GroupLevel)
        })
    }
}

impl Drop for GroupLevel {
    fn drop(&mut self) {
        GROUP_LEVEL.with(|level| level.set(level.get() - 1));
    }
}

fn lenient_items<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<CanvasItem>, D::Error> {
    let raw = Vec::<Value>::deserialize(deserializer)?;

    Ok(raw
        .into_iter()
        .filter_map(|value| match CanvasItem::deserialize(value) {
            Ok(item) => Some(item),
            Err(e) => {
                log::warn!("canvas: dropping an item that doesn't parse: {e}");
                None
            }
        })
        .collect())
}

/// What a canvas asks of the renderer, counted without building it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub items: usize,
    pub grids: usize,
    pub shaders: usize,
    /// The deepest group nesting: 0 with no groups, 1 for a group at the
    /// top level.
    pub depth: usize,
}

impl CanvasState {
    /// Every item, parents before children, with its nesting level.
    pub fn walk(&self, f: &mut dyn FnMut(&CanvasItem, usize)) {
        walk_items(&self.items, 0, f);
    }

    pub fn find(&self, id: &str) -> Option<&CanvasItem> {
        find_in(&self.items, id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut CanvasItem> {
        find_in_mut(&mut self.items, id)
    }

    /// Each grid item's dock dump, groups included.
    pub fn grid_dumps(&self) -> Vec<&Value> {
        let mut dumps = Vec::new();
        collect_dumps(&self.items, &mut dumps);
        dumps
    }

    /// `grid`, then `grid-2`. Unique across the whole tree.
    pub fn fresh_id(&self, stem: &str) -> String {
        let mut taken = HashSet::new();
        self.walk(&mut |item, _| {
            taken.insert(item.id.clone());
        });

        if !taken.contains(stem) {
            return stem.to_owned();
        }

        (2..)
            .map(|n| format!("{stem}-{n}"))
            .find(|id| !taken.contains(id))
            .expect("an unbounded range finds a free id")
    }

    pub fn cost(&self) -> Cost {
        let mut cost = Cost::default();

        self.walk(&mut |item, level| {
            cost.items += 1;

            match item.content {
                Content::Grid { .. } => cost.grids += 1,
                Content::Shader { .. } => cost.shaders += 1,
                Content::Group { .. } => cost.depth = cost.depth.max(level + 1),
                _ => {}
            }
        });

        cost
    }
}

fn walk_items(items: &[CanvasItem], level: usize, f: &mut dyn FnMut(&CanvasItem, usize)) {
    for item in items {
        f(item, level);

        if let Content::Group { items } = &item.content {
            walk_items(items, level + 1, f);
        }
    }
}

fn find_in<'a>(items: &'a [CanvasItem], id: &str) -> Option<&'a CanvasItem> {
    items.iter().find_map(|item| {
        if item.id == id {
            return Some(item);
        }

        match &item.content {
            Content::Group { items } => find_in(items, id),
            _ => None,
        }
    })
}

fn find_in_mut<'a>(items: &'a mut [CanvasItem], id: &str) -> Option<&'a mut CanvasItem> {
    for item in items {
        if item.id == id {
            return Some(item);
        }

        if let Content::Group { items } = &mut item.content
            && let Some(found) = find_in_mut(items, id)
        {
            return Some(found);
        }
    }

    None
}

fn collect_dumps<'a>(items: &'a [CanvasItem], dumps: &mut Vec<&'a Value>) {
    for item in items {
        match &item.content {
            Content::Grid { dock } => dumps.push(dock),
            Content::Group { items } => collect_dumps(items, dumps),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn grid(id: &str) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            content: Content::Grid {
                dock: json!({ "root": id }),
            },
            ..CanvasItem::default()
        }
    }

    fn group(id: &str, items: Vec<CanvasItem>) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            content: Content::Group { items },
            ..CanvasItem::default()
        }
    }

    /// Groups nested `levels` deep, a grid at the bottom.
    fn nested(levels: usize) -> CanvasItem {
        (0..levels).fold(grid("leaf"), |inner, n| {
            group(&format!("g{n}"), vec![inner])
        })
    }

    #[test]
    fn a_canvas_round_trips() {
        let canvas = CanvasState {
            stage: Stage {
                width: 1280.,
                height: 720.,
                fit: Fit::Cover,
            },
            camera: Camera {
                fov: 50.,
                parallax: 12.,
            },
            items: vec![
                grid("grid"),
                group(
                    "group",
                    vec![CanvasItem {
                        id: "cover".into(),
                        name: "Cover".into(),
                        place: Place {
                            anchor: Anchor::TopRight,
                            position: [-20., 20., 40.],
                            size: [Length::Stage(0.5), Length::Units(200.)],
                            rotation: [0., 25., 0.],
                            scale: 1.5,
                        },
                        locked: true,
                        opacity: 0.8,
                        sway: Sway {
                            position: [0., 0., 30.],
                            rotation: [0., 10., 0.],
                        },
                        action: "play_pause".into(),
                        content: Content::Image {
                            asset: "a1b2".into(),
                            fit: ImageFit::Cover,
                        },
                        ..CanvasItem::default()
                    }],
                ),
                CanvasItem {
                    id: "title".into(),
                    content: Content::Text {
                        text: "%artist% - %title%".into(),
                        font: Some("Inter".into()),
                        size: 32.,
                        color: "#ffffff".into(),
                        align: Align::Left,
                    },
                    ..CanvasItem::default()
                },
                CanvasItem {
                    id: "glow".into(),
                    content: Content::Shader {
                        shader: json!({ "preset": "glow" }),
                    },
                    ..CanvasItem::default()
                },
            ],
            ..CanvasState::default()
        };

        let text = serde_json::to_string(&canvas).unwrap();
        let back: CanvasState = serde_json::from_str(&text).unwrap();

        assert_eq!(back, canvas);
        assert!(text.contains(r#""size":[{"stage":0.5},{"units":200.0}]"#));
        assert!(text.contains(r#""fit":"cover""#));
        assert!(text.contains(r#""anchor":"top_right""#));
    }

    #[test]
    fn a_broken_item_costs_only_itself() {
        let canvas: CanvasState = serde_json::from_value(json!({
            "items": [
                { "id": "a", "content": { "kind": "image", "asset": "v" } },
                { "id": 7, "opacity": "loud" },
                { "id": "b", "content": { "kind": "group", "items": [
                    { "id": "c" },
                    { "id": "d", "visible": "maybe" },
                ] } },
            ]
        }))
        .unwrap();

        let mut ids = Vec::new();
        canvas.walk(&mut |item, _| ids.push(item.id.clone()));

        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn an_unknown_kind_comes_back_byte_for_byte() {
        // Keys sorted, so the bytes hold whichever map serde_json was built with.
        let raw = r#"{"extra":{"deep":[1,2.5,"x",null]},"kind":"hologram","z":true}"#;
        let item: CanvasItem =
            serde_json::from_str(&format!(r#"{{"id":"h","content":{raw}}}"#)).unwrap();

        assert!(matches!(item.content, Content::Unknown(_)));
        assert_eq!(serde_json::to_string(&item.content).unwrap(), raw);
    }

    #[test]
    fn a_video_or_model_item_is_kept_as_saved_like_any_unknown_kind() {
        for raw in [
            r#"{"asset":"clip","kind":"video"}"#,
            r#"{"asset":"ship","kind":"model"}"#,
        ] {
            let item: CanvasItem =
                serde_json::from_str(&format!(r#"{{"id":"x","content":{raw}}}"#)).unwrap();

            assert!(matches!(item.content, Content::Unknown(_)), "{raw}");
            assert_eq!(item.content.asset(), None);
            assert_eq!(serde_json::to_string(&item.content).unwrap(), raw);
        }
    }

    #[test]
    fn a_known_kind_with_a_bad_body_is_kept_not_dropped() {
        let body = json!({ "kind": "image", "asset": 42 });
        let canvas: CanvasState =
            serde_json::from_value(json!({ "items": [{ "id": "i", "content": body.clone() }] }))
                .unwrap();

        assert_eq!(canvas.items.len(), 1);
        assert_eq!(canvas.items[0].content, Content::Unknown(body.clone()));

        let again = serde_json::to_value(&canvas).unwrap();
        assert_eq!(again["items"][0]["content"], body);
    }

    #[test]
    fn an_old_canvas_reads_fields_it_never_had() {
        let canvas: CanvasState = serde_json::from_value(json!({
            "items": [{ "id": "a", "place": { "position": [10, 0, 0] }, "content": { "kind": "image", "asset": "x" } }]
        }))
        .unwrap();

        assert_eq!(canvas.version, CANVAS_VERSION);
        assert_eq!(canvas.stage, Stage::default());
        assert_eq!(canvas.camera.fov, 35.);

        let item = &canvas.items[0];
        assert_eq!(item.place.position, [10., 0., 0.]);
        assert_eq!(item.place.size, [Length::Units(480.), Length::Units(320.)]);
        assert_eq!(item.place.anchor, Anchor::Center);
        assert_eq!(item.place.scale, 1.);
        assert!(item.visible && !item.locked);
        assert_eq!(item.opacity, 1.);
        assert_eq!(
            item.content,
            Content::Image {
                asset: "x".into(),
                fit: ImageFit::Contain
            }
        );
    }

    #[test]
    fn a_plain_item_writes_no_sway_and_no_action() {
        let value = serde_json::to_value(grid("g")).unwrap();
        let object = value.as_object().unwrap();

        assert!(!object.contains_key("sway"));
        assert!(!object.contains_key("action"));
        assert!(object.contains_key("place"));
    }

    #[test]
    fn fresh_ids_count_up_and_see_inside_groups() {
        let mut canvas = CanvasState::default();
        assert_eq!(canvas.fresh_id("grid"), "grid");

        canvas
            .items
            .push(group("group", vec![grid("grid"), grid("grid-2")]));
        assert_eq!(canvas.fresh_id("grid"), "grid-3");
        assert_eq!(canvas.fresh_id("group"), "group-2");
        assert_eq!(canvas.fresh_id("image"), "image");
    }

    #[test]
    fn find_reaches_a_grandchild() {
        let mut canvas = CanvasState {
            items: vec![
                grid("top"),
                group("outer", vec![group("inner", vec![grid("deep")])]),
            ],
            ..CanvasState::default()
        };

        assert_eq!(canvas.find("deep").map(|i| i.id.as_str()), Some("deep"));
        assert!(canvas.find("missing").is_none());

        canvas.find_mut("deep").unwrap().name = "Found".into();
        assert_eq!(canvas.find("deep").unwrap().name, "Found");

        let dumps = canvas.grid_dumps();
        assert_eq!(
            dumps,
            [&json!({ "root": "top" }), &json!({ "root": "deep" })]
        );
    }

    #[test]
    fn a_group_past_the_depth_cap_is_kept_as_saved() {
        let saved = serde_json::to_value(CanvasState {
            items: vec![nested(MAX_DEPTH + 1)],
            ..CanvasState::default()
        })
        .unwrap();

        let canvas: CanvasState = serde_json::from_value(saved.clone()).unwrap();

        let mut levels = Vec::new();
        canvas.walk(&mut |item, level| {
            levels.push((level, matches!(item.content, Content::Unknown(_))))
        });

        // The four groups that fit are read, and the fifth is held whole.
        let expected: Vec<_> = (0..=MAX_DEPTH)
            .map(|level| (level, level == MAX_DEPTH))
            .collect();
        assert_eq!(levels, expected);
        assert_eq!(canvas.cost().depth, MAX_DEPTH);
        assert_eq!(serde_json::to_value(&canvas).unwrap(), saved);

        let fits: CanvasState = serde_json::from_value(
            serde_json::to_value(CanvasState {
                items: vec![nested(MAX_DEPTH)],
                ..CanvasState::default()
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(fits.find("leaf").map(|i| i.id.as_str()), Some("leaf"));
    }

    #[test]
    fn cost_counts_every_level() {
        let shader = CanvasItem {
            content: Content::Shader { shader: json!({}) },
            ..CanvasItem::default()
        };

        let canvas = CanvasState {
            items: vec![
                grid("a"),
                shader.clone(),
                group("g", vec![grid("b"), group("h", vec![shader])]),
            ],
            ..CanvasState::default()
        };

        assert_eq!(
            canvas.cost(),
            Cost {
                items: 6,
                grids: 2,
                shaders: 2,
                depth: 2,
            }
        );
    }

    #[test]
    fn asset_names_only_what_holds_one() {
        let image = Content::Image {
            asset: "a".into(),
            fit: ImageFit::Stretch,
        };

        assert_eq!(image.asset(), Some("a"));
        assert_eq!(
            Content::Image {
                asset: String::new(),
                fit: ImageFit::Contain,
            }
            .asset(),
            None
        );
        assert_eq!(Content::Shader { shader: json!({}) }.asset(), None);
    }
}
