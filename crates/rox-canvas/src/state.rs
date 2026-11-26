//! A canvas's saved shape. Every struct reads with defaults, so a canvas saved
//! before a field existed still loads, and every item list reads leniently,
//! so one broken item costs only itself.

use std::cell::Cell;
use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::place::{Position, no_limits};

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
    #[serde(skip_serializing_if = "Idle::is_rest")]
    pub idle: Idle,
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
            idle: Idle::Rest,
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

/// What routed items do while playback isn't running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Idle {
    /// Each item at its own settings, as if no route reached it.
    #[default]
    Rest,
    /// Each item where its routes last put it.
    Hold,
}

impl Idle {
    pub fn is_rest(&self) -> bool {
        *self == Idle::Rest
    }
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
    /// A `buttons::STATES` id. Empty means the item follows nothing, and an id
    /// the app doesn't have reads the same.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    /// What the item does in each case of `state`. A case with no entry draws
    /// the item as it is. Lenient: a broken case costs only itself.
    #[serde(
        deserialize_with = "lenient_cases",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub cases: Vec<Case>,
    /// The look while the pointer is over the item, off design mode. Only
    /// an item with a click command takes it. Lenient, like a case.
    #[serde(
        deserialize_with = "lenient_look",
        skip_serializing_if = "Option::is_none"
    )]
    pub hover: Option<Look>,
    /// The look while it's pressed, over the hover look.
    #[serde(
        deserialize_with = "lenient_look",
        skip_serializing_if = "Option::is_none"
    )]
    pub pressed: Option<Look>,
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
            state: String::new(),
            cases: Vec::new(),
            hover: None,
            pressed: None,
            content: Content::default(),
        }
    }
}

impl CanvasItem {
    /// Every asset this item names, its cases' and looks' included, and not its
    /// children's. Anything that collects or rewrites an item's assets goes
    /// through here and [`assets_mut`](Self::assets_mut), so a case's image
    /// is never left behind.
    pub fn assets(&self) -> impl Iterator<Item = &str> {
        let looks = [&self.hover, &self.pressed].into_iter().flatten();
        self.content
            .asset()
            .into_iter()
            .chain(self.cases.iter().map(|case| case.asset.as_str()))
            .chain(looks.map(|look| look.asset.as_str()))
            .filter(|asset| !asset.is_empty())
    }

    /// Each asset id [`assets`](Self::assets) yields, to rewrite in place.
    pub fn assets_mut(&mut self, f: &mut dyn FnMut(&mut String)) {
        if let Content::Image { asset, .. } = &mut self.content
            && !asset.is_empty()
        {
            f(asset);
        }

        for case in &mut self.cases {
            if !case.asset.is_empty() {
                f(&mut case.asset);
            }
        }

        for look in [&mut self.hover, &mut self.pressed].into_iter().flatten() {
            if !look.asset.is_empty() {
                f(&mut look.asset);
            }
        }
    }
}

/// One case of the state an item follows: whether it shows, and its look.
/// The app reads `when` against the state's own list, so an id it doesn't
/// have is never drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Case {
    /// The `StateCase` id this applies in.
    pub when: String,
    /// False hides the item, and everything inside a group, in this case.
    pub visible: bool,
    /// Multiplies the item's opacity in this case.
    pub opacity: f32,
    /// Image items: the asset shown in this case. Empty keeps the item's own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub asset: String,
    /// Text items: the color in this case. Empty keeps the item's own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color: String,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            when: String::new(),
            visible: true,
            opacity: 1.,
            asset: String::new(),
            color: String::new(),
        }
    }
}

impl Case {
    /// Shown, at full opacity, with the item's own image and color: a case
    /// that changes nothing.
    pub fn is_plain(&self) -> bool {
        *self
            == Case {
                when: self.when.clone(),
                ..Case::default()
            }
    }
}

/// How a click item looks while hovered or pressed: what a case can change
/// but whether it shows, since a hover that hid its item would lose the
/// pointer it came from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// Multiplies the item's opacity, its case's included.
    pub opacity: f32,
    /// Image items: the asset shown. Empty keeps the one the case shows.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub asset: String,
    /// Text items: the color. Empty keeps the one the case gives.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color: String,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            opacity: 1.,
            asset: String::new(),
            color: String::new(),
        }
    }
}

impl Look {
    /// A look that changes nothing.
    pub fn is_plain(&self) -> bool {
        *self == Look::default()
    }
}

/// A look that doesn't parse reads as none, so it costs the look and not
/// the item.
fn lenient_look<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Look>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }

    match Look::deserialize(value) {
        Ok(look) => Ok(Some(look)),
        Err(e) => {
            log::warn!("canvas: dropping a hover or pressed look that doesn't parse: {e}");
            Ok(None)
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
    /// From the anchor's point: x and y each in units or as a fraction of
    /// the parent's box, and z in units.
    pub position: Position,
    pub size: [Length; 2],
    /// The least each side resolves to, in units. Null for no limit.
    #[serde(skip_serializing_if = "no_limits")]
    pub min_size: [Option<f32>; 2],
    /// The most each side resolves to, in units. Null for no limit.
    #[serde(skip_serializing_if = "no_limits")]
    pub max_size: [Option<f32>; 2],
    /// Degrees about x, y, z.
    pub rotation: [f32; 3],
    pub scale: f32,
}

impl Default for Place {
    fn default() -> Self {
        Self {
            anchor: Anchor::Center,
            position: Position::default(),
            size: [Length::Units(480.), Length::Units(320.)],
            min_size: [None; 2],
            max_size: [None; 2],
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
        /// Nine-slice insets in the image's own pixels: left, top, right,
        /// bottom. Set, the image fills its box whatever its fit.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slice: Option<[u32; 4]>,
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
    /// A picture a renderer draws (ADR 37).
    Target {
        /// The renderer, as `plugin:<package id>`. Named `owner` so the
        /// dump walkers that already read that key list it in `requires`
        /// with no change.
        owner: String,
        /// Pixels the renderer draws at. Clamped to 16..=3840 by 16..=2160
        /// where a session starts; saved as written.
        width: u32,
        height: u32,
        /// Pauses the renderer while the music is paused or stopped, holding
        /// its last frame. Left out reads as running on.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rests: bool,
        /// The renderer's own settings, opaque to rox. Reserved: nothing
        /// reads or writes it yet.
        #[serde(default)]
        config: Value,
    },
    /// A kind this build doesn't know, or a known kind whose body didn't
    /// parse. Serializes back to the value it was read from.
    #[serde(skip)]
    Unknown(Value),
}

const KINDS: [&str; 6] = ["grid", "group", "image", "text", "shader", "target"];

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

/// A `cases` that isn't a list reads as none, so it costs the cases and not
/// the item.
fn lenient_cases<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Case>, D::Error> {
    let Value::Array(raw) = Value::deserialize(deserializer)? else {
        log::warn!("canvas: dropping cases that aren't a list");
        return Ok(Vec::new());
    };

    Ok(raw
        .into_iter()
        .filter_map(|value| match Case::deserialize(value) {
            Ok(case) => Some(case),
            Err(e) => {
                log::warn!("canvas: dropping a case that doesn't parse: {e}");
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
    pub targets: usize,
    /// Every target's width times height, as saved.
    pub target_pixels: u64,
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
                Content::Target { width, height, .. } => {
                    cost.targets += 1;
                    // Two hand-written u32::MAX squares pass a u64.
                    cost.target_pixels = cost
                        .target_pixels
                        .saturating_add(u64::from(width) * u64::from(height));
                }
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
                            position: Position::units([-20., 20., 40.]),
                            size: [Length::Stage(0.5), Length::Units(200.)],
                            rotation: [0., 25., 0.],
                            scale: 1.5,
                            ..Place::default()
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
                            slice: None,
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
        assert_eq!(item.place.position, Position::units([10., 0., 0.]));
        assert_eq!(item.place.size, [Length::Units(480.), Length::Units(320.)]);
        assert_eq!(item.place.anchor, Anchor::Center);
        assert_eq!(item.place.scale, 1.);
        assert!(item.visible && !item.locked);
        assert_eq!(item.opacity, 1.);
        assert_eq!(
            item.content,
            Content::Image {
                asset: "x".into(),
                fit: ImageFit::Contain,
                slice: None,
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
                ..Cost::default()
            }
        );
    }

    fn target(id: &str, owner: &str, width: u32, height: u32) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            content: Content::Target {
                owner: owner.into(),
                width,
                height,
                rests: false,
                config: Value::Null,
            },
            ..CanvasItem::default()
        }
    }

    #[test]
    fn a_target_round_trips() {
        let mut bars = target("bars", "plugin:bars", 1280, 720);
        if let Content::Target { config, .. } = &mut bars.content {
            *config = json!({ "palette": "ember", "count": 32 });
        }
        let canvas = CanvasState {
            items: vec![bars, target("blank", "", 16, 16)],
            ..CanvasState::default()
        };

        let text = serde_json::to_string(&canvas).unwrap();
        let back: CanvasState = serde_json::from_str(&text).unwrap();

        assert_eq!(back, canvas);
        assert!(text.contains(r#""kind":"target""#));

        // No config reads as none, and an empty owner still parses.
        let bare: CanvasItem = serde_json::from_value(json!({
            "id": "t",
            "content": { "kind": "target", "owner": "", "width": 64, "height": 32 },
        }))
        .unwrap();
        assert_eq!(bare.content, target("t", "", 64, 32).content);
        assert_eq!(bare.assets().count(), 0);

        // Resting with the music is written only when it's on.
        assert!(!text.contains("rests"), "{text}");
        let mut calm = target("calm", "plugin:bars", 64, 32);
        if let Content::Target { rests, .. } = &mut calm.content {
            *rests = true;
        }
        let written = serde_json::to_value(&calm).unwrap();
        assert_eq!(written["content"]["rests"], json!(true));
        assert_eq!(serde_json::from_value::<CanvasItem>(written).unwrap(), calm);
    }

    #[test]
    fn a_target_counts_its_pixels() {
        let canvas = CanvasState {
            items: vec![
                target("a", "plugin:bars", 1920, 1080),
                group("g", vec![target("b", "plugin:bars", 1280, 720)]),
            ],
            ..CanvasState::default()
        };

        let cost = canvas.cost();
        assert_eq!(cost.targets, 2);
        assert_eq!(cost.target_pixels, 1920 * 1080 + 1280 * 720);

        // Sizes as large as a saved one can claim saturate the sum.
        let huge = CanvasState {
            items: vec![
                target("x", "plugin:bars", u32::MAX, u32::MAX),
                target("y", "plugin:bars", u32::MAX, u32::MAX),
            ],
            ..CanvasState::default()
        };
        assert_eq!(huge.cost().target_pixels, u64::MAX);
    }

    #[test]
    fn a_target_names_its_renderer_as_owner() {
        let value = serde_json::to_value(target("t", "plugin:bars", 640, 360)).unwrap();

        assert_eq!(value["content"]["owner"], "plugin:bars");
        assert!(value["content"].get("renderer").is_none());

        // A target missing its size is kept as saved, not dropped.
        let body = json!({ "kind": "target", "owner": "plugin:bars" });
        let item: CanvasItem =
            serde_json::from_value(json!({ "id": "t", "content": body.clone() })).unwrap();
        assert_eq!(item.content, Content::Unknown(body));
    }

    #[test]
    fn an_item_with_no_cases_saves_as_before() {
        let value = serde_json::to_value(grid("g")).unwrap();
        let object = value.as_object().unwrap();

        assert!(!object.contains_key("state"));
        assert!(!object.contains_key("cases"));

        // And one saved before the fields existed reads with neither.
        let old: CanvasItem = serde_json::from_value(json!({ "id": "o" })).unwrap();
        assert!(old.state.is_empty() && old.cases.is_empty());
    }

    #[test]
    fn a_broken_case_costs_only_itself() {
        let item: CanvasItem = serde_json::from_value(json!({
            "id": "play",
            "state": "player.playback",
            "cases": [
                { "when": "playing", "asset": "pause" },
                { "when": 7 },
                { "when": "paused", "visible": false, "opacity": 0.5 },
                "loud",
            ],
            "content": { "kind": "image", "asset": "play" },
        }))
        .unwrap();

        assert_eq!(item.id, "play");
        assert_eq!(item.cases.len(), 2);
        assert_eq!(item.cases[0].when, "playing");
        assert!(item.cases[0].visible, "a missing field takes its default");
        assert_eq!(item.cases[0].opacity, 1.);
        assert!(!item.cases[1].visible);

        // Cases that aren't a list at all cost the cases, not the item.
        let item: CanvasItem =
            serde_json::from_value(json!({ "id": "x", "state": "player.mute", "cases": "no" }))
                .unwrap();
        assert_eq!(item.state, "player.mute");
        assert!(item.cases.is_empty());
    }

    #[test]
    fn unknown_ids_in_a_state_and_its_cases_survive_a_save() {
        let saved = json!({
            "id": "x",
            "state": "player.from_a_newer_build",
            "cases": [{ "when": "sideways", "visible": false, "opacity": 1.0 }],
            "content": { "kind": "image", "asset": "a" },
        });
        let item: CanvasItem = serde_json::from_value(saved.clone()).unwrap();
        let again = serde_json::to_value(&item).unwrap();

        assert_eq!(again["state"], saved["state"]);
        assert_eq!(again["cases"], saved["cases"]);
    }

    #[test]
    fn every_asset_an_item_names_includes_its_cases() {
        let mut item = CanvasItem {
            content: Content::Image {
                asset: "play".into(),
                fit: ImageFit::Contain,
                slice: None,
            },
            cases: vec![
                Case {
                    when: "playing".into(),
                    asset: "pause".into(),
                    ..Case::default()
                },
                Case {
                    when: "paused".into(),
                    ..Case::default()
                },
            ],
            ..CanvasItem::default()
        };
        assert_eq!(item.assets().collect::<Vec<_>>(), ["play", "pause"]);

        item.assets_mut(&mut |asset| asset.push_str("-2"));
        assert_eq!(item.assets().collect::<Vec<_>>(), ["play-2", "pause-2"]);
        assert!(item.cases[1].asset.is_empty(), "an empty case stays empty");

        // A text item names no asset of its own, and a case's still counts.
        let text = CanvasItem {
            content: Content::Text {
                text: "%title%".into(),
                font: None,
                size: 12.,
                color: "text".into(),
                align: Align::Center,
            },
            cases: vec![Case {
                when: "on".into(),
                asset: "stray".into(),
                ..Case::default()
            }],
            ..CanvasItem::default()
        };
        assert_eq!(text.assets().collect::<Vec<_>>(), ["stray"]);
    }

    #[test]
    fn a_hover_and_pressed_look_round_trip_and_a_plain_item_writes_neither() {
        let item: CanvasItem = serde_json::from_value(json!({
            "id": "play",
            "action": "play_pause",
            "hover": { "opacity": 0.8, "asset": "play-lit" },
            "pressed": { "asset": "play-down" },
            "content": { "kind": "image", "asset": "play" },
        }))
        .unwrap();

        let hover = item.hover.as_ref().expect("a hover look");
        assert_eq!(hover.opacity, 0.8);
        assert_eq!(item.pressed.as_ref().map(|look| look.opacity), Some(1.));
        assert_eq!(
            serde_json::from_value::<CanvasItem>(serde_json::to_value(&item).unwrap()).unwrap(),
            item
        );

        let plain = serde_json::to_value(grid("g")).unwrap();
        assert!(plain.get("hover").is_none() && plain.get("pressed").is_none());

        // A look that doesn't parse costs the look, not the item.
        let odd: CanvasItem = serde_json::from_value(json!({
            "id": "x",
            "hover": "glow",
            "pressed": { "opacity": "half" },
        }))
        .unwrap();
        assert_eq!(odd.id, "x");
        assert!(odd.hover.is_none() && odd.pressed.is_none());
    }

    #[test]
    fn every_asset_an_item_names_includes_its_looks() {
        let mut item = CanvasItem {
            content: Content::Image {
                asset: "play".into(),
                fit: ImageFit::Contain,
            },
            hover: Some(Look {
                asset: "lit".into(),
                ..Look::default()
            }),
            pressed: Some(Look {
                opacity: 0.5,
                ..Look::default()
            }),
            ..CanvasItem::default()
        };
        assert_eq!(item.assets().collect::<Vec<_>>(), ["play", "lit"]);

        item.assets_mut(&mut |asset| asset.push_str("-2"));
        assert_eq!(item.assets().collect::<Vec<_>>(), ["play-2", "lit-2"]);
        assert!(item.pressed.as_ref().unwrap().asset.is_empty());
        assert!(!item.pressed.as_ref().unwrap().is_plain());
        assert!(Look::default().is_plain());
    }

    #[test]
    fn a_plain_case_changes_nothing() {
        let plain = Case {
            when: "on".into(),
            ..Case::default()
        };
        assert!(plain.is_plain());

        for changed in [
            Case {
                visible: false,
                ..plain.clone()
            },
            Case {
                opacity: 0.5,
                ..plain.clone()
            },
            Case {
                asset: "a".into(),
                ..plain.clone()
            },
            Case {
                color: "accent".into(),
                ..plain.clone()
            },
        ] {
            assert!(!changed.is_plain(), "{changed:?}");
        }
    }

    #[test]
    fn asset_names_only_what_holds_one() {
        let image = Content::Image {
            asset: "a".into(),
            fit: ImageFit::Stretch,
            slice: None,
        };

        assert_eq!(image.asset(), Some("a"));
        assert_eq!(
            Content::Image {
                asset: String::new(),
                fit: ImageFit::Contain,
                slice: None,
            }
            .asset(),
            None
        );
        assert_eq!(Content::Shader { shader: json!({}) }.asset(), None);
    }
}
