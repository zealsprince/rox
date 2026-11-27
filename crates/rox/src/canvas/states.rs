//! Canvas items that follow state (ADR 33's amendment of 2026-11-16). An item
//! names a piece of state from the table Custom Controls picks from
//! ([`buttons::STATES`]), and for each of that state's cases it can hide, fade,
//! show another image or take another color. Nothing is an expression: the
//! state id and the case ids are only ever looked up in that fixed table, so
//! an id it doesn't have changes nothing, and a canvas from a newer build or
//! from someone else draws as if that item followed nothing.
//!
//! The state is read once per render into [`Cases`]. The panel's observers
//! read it again and ask for a render only when a case moved.
//!
//! The canvas adds states of its own to that table, ones only it can read
//! ([`CANVAS_STATES`]): whether each of the playing track's fields is empty,
//! whether it has a cover, and the canvas's own shape. A case can also give
//! its item a place of its own and a group a stack of its own, which is how a
//! look rearranges for a tall canvas. Those cut, whatever the ease: a place
//! that glided would trail a window being dragged.

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;

use rox_canvas::{CanvasItem, CanvasState, Case, Content, Live, Viewport};
use rox_design::assets::icons;
use rox_panel_api::buttons::{self, StateCase, StateSpec};

use crate::canvas::fades;
use crate::canvas::text::{self, Values};
use crate::panels::controls;

/// The opacity an item its case hides draws at in design mode, so it can
/// still be found and picked.
pub(crate) const GHOST: f32 = 0.25;

/// The case each followed state is in, by state id.
pub(crate) type Cases = HashMap<&'static str, &'static str>;

/// The canvas's own box, tall or wide.
pub(crate) const SHAPE: &str = "canvas.shape";

const ART: &str = "player.art";

/// The playing track's fields: `track.` and a placeholder's name, so
/// `track.album` is empty exactly when `%album%` renders nothing.
const TRACK: &str = "track.";

/// The width over height where a canvas turns from tall to wide.
const SHAPE_SPLIT: f32 = 1.0;

/// How far past the split, as a factor, a canvas has to go before it turns:
/// tall below 0.943, wide above 1.06, and between them whichever it was. A
/// window dragged slowly across the split turns once, and a few pixels from a
/// scrollbar or a snap never flip it.
const SHAPE_BAND: f32 = 1.06;

const FIELD_CASES: &[StateCase] = &[
    StateCase {
        id: "empty",
        label_key: "canvas-case-field-empty",
        icon: icons::SQUARE_DASHED,
        color: "text_faint",
    },
    StateCase {
        id: "present",
        label_key: "canvas-case-field-present",
        icon: icons::TAG,
        color: "text",
    },
];

const fn field(id: &'static str, label_key: &'static str) -> StateSpec {
    StateSpec {
        id,
        label_key,
        cases: FIELD_CASES,
        action: "",
        read: read_by_canvas,
    }
}

/// The states only a canvas can read, in the order its picker lists them.
/// Custom Controls doesn't offer them: a button has no track text, cover or
/// box of its own to follow. Their ids are saved in layouts forever, like
/// [`buttons::STATES`]'s.
pub(crate) static CANVAS_STATES: &[StateSpec] = &[
    field("track.title", "canvas-state-track-title"),
    field("track.artist", "canvas-state-track-artist"),
    field("track.albumartist", "canvas-state-track-albumartist"),
    field("track.album", "canvas-state-track-album"),
    field("track.track", "canvas-state-track-track"),
    field("track.year", "canvas-state-track-year"),
    field("track.genre", "canvas-state-track-genre"),
    field("track.station", "canvas-state-track-station"),
    field("track.source", "canvas-state-track-source"),
    field("track.format", "canvas-state-track-format"),
    StateSpec {
        id: ART,
        label_key: "canvas-state-art",
        action: "",
        read: read_by_canvas,
        cases: &[
            StateCase {
                id: "present",
                label_key: "canvas-case-art-present",
                icon: icons::IMAGE,
                color: "text",
            },
            StateCase {
                id: "none",
                label_key: "canvas-case-art-none",
                icon: icons::SQUARE_DASHED,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: SHAPE,
        label_key: "canvas-state-shape",
        action: "",
        read: read_by_canvas,
        cases: &[
            StateCase {
                id: "wide",
                label_key: "canvas-case-shape-wide",
                icon: icons::COLUMNS_2,
                color: "text",
            },
            StateCase {
                id: "tall",
                label_key: "canvas-case-shape-tall",
                icon: icons::ROWS_2,
                color: "text",
            },
        ],
    },
];

/// Never called: [`read`] answers the canvas's own states from [`Sense`].
fn read_by_canvas(_: &buttons::Live) -> &'static str {
    ""
}

/// The state `id` names, from either table.
pub(crate) fn spec(id: &str) -> Option<&'static StateSpec> {
    controls::state_spec(id).or_else(|| CANVAS_STATES.iter().find(|spec| spec.id == id))
}

/// What only the canvas can say, for the states it reads itself.
pub(crate) struct Sense<'a> {
    /// The playing track's fields, as its text items fill them.
    pub values: &'a Values,
    /// Whether the playing track has a cover, held while the next loads.
    pub art: bool,
    pub shape: Option<Shape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Wide,
    Tall,
}

impl Shape {
    /// The shape a canvas of `viewport` takes, having held `held`. A box
    /// with no area keeps what it had.
    pub(crate) fn next(held: Option<Shape>, viewport: Viewport) -> Option<Shape> {
        if !(viewport.width > 0. && viewport.height > 0.) {
            return held;
        }

        let aspect = viewport.width / viewport.height;
        Some(match held {
            Some(Shape::Wide) if aspect >= SHAPE_SPLIT / SHAPE_BAND => Shape::Wide,
            Some(Shape::Tall) if aspect <= SHAPE_SPLIT * SHAPE_BAND => Shape::Tall,
            _ if aspect < SHAPE_SPLIT => Shape::Tall,
            _ => Shape::Wide,
        })
    }

    fn case(self) -> &'static str {
        match self {
            Shape::Wide => "wide",
            Shape::Tall => "tall",
        }
    }
}

/// The case a canvas state is in, None for a state that isn't the canvas's.
fn read_canvas(id: &str, sense: &Sense) -> Option<&'static str> {
    if id == ART {
        return Some(if sense.art { "present" } else { "none" });
    }
    if id == SHAPE {
        return Some(sense.shape.unwrap_or(Shape::Wide).case());
    }

    let name = id.strip_prefix(TRACK)?;
    let empty = text::renders_empty(name, sense.values)?;
    Some(if empty { "empty" } else { "present" })
}

/// What an item's case does to it this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Applied<'a> {
    pub visible: bool,
    pub opacity: f32,
    pub asset: Option<&'a str>,
    pub color: Option<&'a str>,
    /// The entry drawn, whose hover and pressed looks stand in for the
    /// item's own.
    pub case: Option<&'a Case>,
}

impl Default for Applied<'_> {
    fn default() -> Self {
        Applied {
            visible: true,
            opacity: 1.,
            asset: None,
            color: None,
            case: None,
        }
    }
}

/// The state `item` follows, when the table has it.
pub(crate) fn followed(item: &CanvasItem) -> Option<&'static StateSpec> {
    // Every item asks every frame, and most follow nothing.
    if item.state.is_empty() {
        return None;
    }

    spec(&item.state)
}

/// Whether any item follows one of the playing track's fields, which the
/// canvas then holds the track's values for, text or no text.
pub(crate) fn needs_values(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= item.state.starts_with(TRACK));
    found
}

/// Whether any item, inside groups too, follows a state the table has. A
/// canvas where none does reads nothing and observes nothing more.
pub(crate) fn follows(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= followed(item).is_some());
    found
}

/// Whether a followed state reads the playing track's marks, which cost
/// queries.
pub(crate) fn needs_marks(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| {
        found |= followed(item).is_some_and(|spec| buttons::needs_marks(spec.id))
    });
    found
}

/// Every followed state's case, each state read once.
pub(crate) fn read(canvas: &CanvasState, live: &buttons::Live, sense: &Sense) -> Cases {
    let mut cases = Cases::new();
    canvas.walk(&mut |item, _| {
        let Some(spec) = followed(item) else {
            return;
        };

        if !cases.contains_key(spec.id)
            && let Some(case) =
                read_canvas(spec.id, sense).or_else(|| buttons::read_state(spec.id, live))
        {
            cases.insert(spec.id, case);
        }
    });
    cases
}

/// The case `item` draws: `preview` when it names one of its state's cases,
/// a look's preview after a colon left off, or else the state's live case.
/// The flag says whether it came from the preview.
fn current_case(
    item: &CanvasItem,
    cases: &Cases,
    preview: Option<&str>,
) -> Option<(&'static str, bool)> {
    let spec = followed(item)?;

    let previewed = preview
        .map(|preview| preview.split_once(':').map_or(preview, |(case, _)| case))
        .and_then(|id| spec.cases.iter().find(|case| case.id == id))
        .map(|case| (case.id, true));
    previewed.or_else(|| cases.get(spec.id).map(|case| (*case, false)))
}

/// What `item` looks like in the case its state is in, or in `preview` when
/// the inspector names one, a look's preview after a colon left off. An
/// empty or unknown state, a case the state doesn't have, and a case with no
/// entry all give the default, which changes nothing.
pub(crate) fn resolve<'a>(
    item: &'a CanvasItem,
    cases: &Cases,
    preview: Option<&str>,
) -> Applied<'a> {
    let Some((current, _)) = current_case(item, cases, preview) else {
        return Applied::default();
    };

    let Some(entry) = item.cases.iter().find(|case| case.when == current) else {
        return Applied::default();
    };

    // A hand-edited file can hold any number here.
    let opacity = if entry.opacity.is_finite() {
        entry.opacity.clamp(0., 1.)
    } else {
        1.
    };
    let named = |text: &'a str| Some(text).filter(|text| !text.is_empty());

    Applied {
        visible: entry.visible,
        opacity,
        asset: named(&entry.asset),
        color: named(&entry.color),
        case: Some(entry),
    }
}

/// The item's live offsets with its case's opacity folded in, eased and with
/// its looks: it rides the same product a route's does, so a group's case
/// reaches its children.
pub(crate) fn fold(mut live: Live, opacity: f32) -> Live {
    live.opacity *= opacity;
    live
}

/// Points `item` at `state`. Cases the new state doesn't have go, so picking
/// None clears them all. An empty On Click takes the state's own command, as
/// a Custom Controls button does, except on a grid, whose panels own the
/// click.
pub(crate) fn set_state(item: &mut CanvasItem, state: String) {
    let spec = spec(&state);
    item.cases.retain(|case| {
        spec.is_some_and(|spec| spec.cases.iter().any(|known| known.id == case.when))
    });
    item.state = state;

    if !matches!(item.content, Content::Grid { .. }) {
        controls::prefill_action(&mut item.action, spec);
    }
}

/// Edits the entry for case `when`, making it first if there's none. An entry
/// edited back to changing nothing goes, so an untouched case saves as no
/// entry at all.
pub(crate) fn edit_case(item: &mut CanvasItem, when: &str, edit: impl FnOnce(&mut Case)) {
    let index = match item.cases.iter().position(|case| case.when == when) {
        Some(index) => index,
        None => {
            item.cases.push(Case {
                when: when.to_string(),
                ..Case::default()
            });
            item.cases.len() - 1
        }
    };

    edit(&mut item.cases[index]);

    if item.cases[index].is_plain() {
        item.cases.remove(index);
    }
}

/// How long a change of case eases on `item`. The shape cuts unless the item
/// asks for an ease, since its cases move things while a window is dragged.
pub(crate) fn ease_length(item: &CanvasItem, still: bool) -> Duration {
    if item.ease.is_none() && item.state == SHAPE {
        return Duration::ZERO;
    }

    fades::length(item, still)
}

/// The picker's headings and the states under each, in order.
pub(crate) fn picker_groups() -> [(&'static str, Vec<&'static StateSpec>); 4] {
    let table = |prefix: &str| {
        buttons::STATES
            .iter()
            .filter(|spec| spec.id.starts_with(prefix))
            .collect::<Vec<_>>()
    };
    let mine = |pick: fn(&str) -> bool| {
        CANVAS_STATES
            .iter()
            .filter(|spec| pick(spec.id))
            .collect::<Vec<_>>()
    };

    let mut player = table("player.");
    player.extend(mine(|id| id == ART));
    [
        ("canvas-state-heading-player", player),
        (
            "canvas-state-heading-track",
            mine(|id| id.starts_with(TRACK)),
        ),
        ("canvas-state-heading-canvas", mine(|id| id == SHAPE)),
        ("canvas-state-heading-app", table("app.")),
    ]
}

// ── A case's place and stack ──

/// Whether any item has a case that moves it or restacks its children.
pub(crate) fn moves_any(canvas: &CanvasState) -> bool {
    let mut found = false;
    canvas.walk(&mut |item, _| found |= item.cases.iter().any(Case::moves));
    found
}

/// `item` as its case places it: a copy with the case's place and stack laid
/// over its own when the case has either, or the item itself. The copy is
/// shallow, a group's children and a grid's dock left behind, so it's for
/// placing and drawing only: read those from the item.
pub(crate) fn placed<'a>(item: &'a CanvasItem, case: Option<&Case>) -> Cow<'a, CanvasItem> {
    let Some(case) = case.filter(|case| case.moves()) else {
        return Cow::Borrowed(item);
    };

    let mut out = shallow(item);
    moved(&mut out, case);
    Cow::Owned(out)
}

fn moved(item: &mut CanvasItem, case: &Case) {
    if let Some(place) = &case.place {
        item.place = place.over(item.place);
    }
    if case.stack.is_some() {
        item.stack = case.stack;
    }
}

/// Everything but what a group holds and a grid's dock, which can run to
/// many items and a whole layout tree.
fn shallow(item: &CanvasItem) -> CanvasItem {
    let content = match &item.content {
        Content::Group { .. } => Content::Group { items: Vec::new() },
        Content::Grid { .. } => Content::Grid {
            dock: serde_json::Value::Null,
        },
        other => other.clone(),
    };

    CanvasItem {
        id: item.id.clone(),
        name: item.name.clone(),
        place: item.place,
        visible: item.visible,
        locked: item.locked,
        opacity: item.opacity,
        sway: item.sway,
        action: item.action.clone(),
        state: item.state.clone(),
        cases: item.cases.clone(),
        hover: item.hover.clone(),
        pressed: item.pressed.clone(),
        hit: item.hit,
        ease: item.ease,
        stack: item.stack,
        content,
    }
}

/// A stack's children as their cases place them, for laying it out. The
/// list itself when none of them moves.
pub(crate) fn placed_children<'a>(
    children: &'a [CanvasItem],
    cases: &Cases,
    preview: Option<(&str, &str)>,
) -> Cow<'a, [CanvasItem]> {
    let entry = |child: &'a CanvasItem| {
        let preview = preview
            .filter(|(id, _)| *id == child.id)
            .map(|(_, case)| case);
        resolve(child, cases, preview).case
    };
    if !children
        .iter()
        .any(|child| entry(child).is_some_and(Case::moves))
    {
        return Cow::Borrowed(children);
    }

    Cow::Owned(
        children
            .iter()
            .map(|child| {
                let mut out = shallow(child);
                if let Some(case) = entry(child) {
                    moved(&mut out, case);
                }
                out
            })
            .collect(),
    )
}

/// What the canvas draws for the editor: the live cases, and what the
/// inspector previews in place of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    /// Every followed state's case, a state previewed across the canvas
    /// already standing in for its live one.
    pub cases: Cases,
    /// The selected item and the case the inspector previews on it.
    pub preview: Option<(String, String)>,
    /// The state previewed on every item that follows it. Only the shape
    /// previews this way, since a layout is the whole canvas's.
    pub across: Option<&'static str>,
}

impl View {
    fn preview_for(&self, item: &CanvasItem) -> Option<&str> {
        self.preview
            .as_ref()
            .filter(|(id, _)| *id == item.id)
            .map(|(_, case)| case.as_str())
    }

    /// The case an edit to `item`'s place or stack lands in: the one it
    /// draws, when that case already moves it or the inspector previews it.
    /// None for the item's own.
    fn routed(&self, item: &CanvasItem) -> Option<&'static str> {
        let (when, previewed) = current_case(item, &self.cases, self.preview_for(item))?;
        let across = self.across.is_some_and(|state| state == item.state);
        let moves = item
            .cases
            .iter()
            .any(|case| case.when == when && case.moves());

        (previewed || across || moves).then_some(when)
    }

    /// Whether any item's edits land in a case.
    fn routes_any(&self, canvas: &CanvasState) -> bool {
        let mut found = false;
        canvas.walk(&mut |item, _| found |= self.routed(item).is_some());
        found
    }

    fn entry<'a>(&self, item: &'a CanvasItem) -> Option<&'a Case> {
        resolve(item, &self.cases, self.preview_for(item)).case
    }
}

/// Every item where its case puts it, whole: what the editor's arithmetic
/// measures and its edits change. The canvas itself when nothing moves.
pub(crate) fn drawn<'a>(canvas: &'a CanvasState, view: &View) -> Cow<'a, CanvasState> {
    let mut moves = false;
    canvas.walk(&mut |item, _| moves |= view.entry(item).is_some_and(Case::moves));
    if !moves {
        return Cow::Borrowed(canvas);
    }

    let mut out = canvas.clone();
    each_mut(&mut out.items, &mut |item| {
        if let Some(case) = view.entry(item).filter(|case| case.moves()).cloned() {
            moved(item, &case);
        }
    });
    Cow::Owned(out)
}

/// Runs `edit` on the canvas as it's drawn, then writes it back: a changed
/// place or stack lands in the case it was drawn in when [`View::routed`]
/// says so, and on the item otherwise. Everything else the edit changed is
/// kept as it made it. With no case to land in, `edit` runs on the canvas
/// straight.
pub(crate) fn edit_drawn(
    canvas: &mut CanvasState,
    view: &View,
    edit: impl FnOnce(&mut CanvasState),
) {
    if !view.routes_any(canvas) {
        edit(canvas);
        return;
    }

    let before = drawn(canvas, view).into_owned();
    let mut after = before.clone();
    edit(&mut after);

    let base = std::mem::take(canvas);
    each_mut(&mut after.items, &mut |item| {
        let (Some(own), Some(was)) = (base.find(&item.id), before.find(&item.id)) else {
            return;
        };

        let (place, stack) = (item.place, item.stack);
        let (moved, restacked) = (place != was.place, stack != was.stack);
        item.place = own.place;
        item.stack = own.stack;

        let Some(when) = view.routed(own) else {
            item.place = place;
            item.stack = stack;
            return;
        };

        if moved || restacked {
            edit_case(item, when, |case| {
                if moved {
                    case.place.get_or_insert_default().take(&was.place, &place);
                }
                if restacked {
                    case.stack = stack;
                }
            });
        }
    });
    *canvas = after;
}

fn each_mut(items: &mut [CanvasItem], f: &mut dyn FnMut(&mut CanvasItem)) {
    for item in items {
        f(item);
        if let Content::Group { items } = &mut item.content {
            each_mut(items, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use rox_canvas::{Camera, Frame, Viewport, project};
    use serde_json::json;

    use super::*;

    fn item(value: serde_json::Value) -> CanvasItem {
        serde_json::from_value(value).expect("an item parses")
    }

    /// An image following playback: a pause glyph while playing, hidden while
    /// paused.
    fn play_button() -> CanvasItem {
        item(json!({
            "id": "play",
            "state": "player.playback",
            "cases": [
                { "when": "playing", "asset": "pause", "opacity": 0.5 },
                { "when": "paused", "visible": false },
            ],
            "content": { "kind": "image", "asset": "play" },
        }))
    }

    fn cases(pairs: &[(&'static str, &'static str)]) -> Cases {
        pairs.iter().copied().collect()
    }

    #[test]
    fn an_item_following_nothing_is_unchanged() {
        let mut plain = play_button();
        plain.state.clear();

        let applied = resolve(&plain, &cases(&[("player.playback", "paused")]), None);
        assert_eq!(applied, Applied::default());
        assert!(!follows(&CanvasState {
            items: vec![plain],
            ..CanvasState::default()
        }));
    }

    #[test]
    fn an_unknown_state_is_unchanged() {
        let mut newer = play_button();
        newer.state = "player.from_a_newer_build".to_string();

        // Whatever the cases say, and whatever the map holds under the id.
        let mut held = Cases::new();
        held.insert("player.from_a_newer_build", "paused");
        assert_eq!(resolve(&newer, &held, None), Applied::default());
        assert_eq!(resolve(&newer, &held, Some("paused")), Applied::default());

        let canvas = CanvasState {
            items: vec![newer],
            ..CanvasState::default()
        };
        assert!(!follows(&canvas), "an unknown state builds no live state");
        assert!(!needs_marks(&canvas));
    }

    #[test]
    fn a_case_with_no_entry_is_unchanged() {
        let mut button = play_button();
        button.cases.retain(|case| case.when == "playing");

        assert_eq!(
            resolve(&button, &cases(&[("player.playback", "paused")]), None),
            Applied::default()
        );

        // A case id the state doesn't have is never the one drawn.
        button.cases[0].when = "sideways".to_string();
        assert_eq!(
            resolve(&button, &cases(&[("player.playback", "playing")]), None),
            Applied::default()
        );
        assert_eq!(
            resolve(&button, &Cases::new(), Some("sideways")),
            Applied::default()
        );
    }

    #[test]
    fn a_hidden_case_hides_the_item() {
        let button = play_button();
        let applied = resolve(&button, &cases(&[("player.playback", "paused")]), None);
        assert!(!applied.visible);
        assert!(resolve(&button, &cases(&[("player.playback", "playing")]), None).visible);
    }

    #[test]
    fn a_case_swaps_an_images_asset() {
        let button = play_button();

        let playing = resolve(&button, &cases(&[("player.playback", "playing")]), None);
        assert_eq!(playing.asset, Some("pause"));
        assert_eq!(playing.color, None);

        // The inspector's preview stands in for the live case.
        let previewed = resolve(
            &button,
            &cases(&[("player.playback", "paused")]),
            Some("playing"),
        );
        assert_eq!(previewed.asset, Some("pause"));
        assert!(previewed.visible);
    }

    #[test]
    fn a_case_looks_preview_draws_its_case() {
        let button = play_button();
        let paused = cases(&[("player.playback", "paused")]);

        let previewed = resolve(&button, &paused, Some("playing:hover"));
        assert_eq!(previewed.asset, Some("pause"));
        assert_eq!(
            previewed.case.map(|case| case.when.as_str()),
            Some("playing")
        );

        // The item's own look names no case, so the live one draws under it.
        assert!(!resolve(&button, &paused, Some(":hover")).visible);
    }

    #[test]
    fn a_cases_opacity_multiplies_the_items() {
        let mut button = play_button();
        button.opacity = 0.8;
        let applied = resolve(&button, &cases(&[("player.playback", "playing")]), None);
        assert_eq!(applied.opacity, 0.5);

        // A route's share and the case's both ride the item's own.
        let routed = Live {
            opacity: 0.5,
            ..Live::default()
        };
        let viewport = Viewport {
            width: 800.,
            height: 600.,
        };
        let stage = Frame::stage(&rox_canvas::Stage::default(), viewport);
        let camera = Camera {
            fov: 0.,
            ..Camera::default()
        };
        let projected = project(
            &button,
            &stage,
            &camera,
            viewport,
            None,
            false,
            &fold(routed, applied.opacity),
        )
        .expect("in front of the camera");
        assert!((projected.opacity - 0.8 * 0.5 * 0.5).abs() < 1e-6);

        // A file can say anything; what draws stays in range.
        button.cases[0].opacity = f32::NAN;
        let odd = resolve(&button, &cases(&[("player.playback", "playing")]), None);
        assert_eq!(odd.opacity, 1.);
        button.cases[0].opacity = 7.;
        let odd = resolve(&button, &cases(&[("player.playback", "playing")]), None);
        assert_eq!(odd.opacity, 1.);
    }

    #[test]
    fn changing_the_state_keeps_only_the_cases_it_has() {
        let mut button = play_button();
        button.cases.push(Case {
            when: "muted".to_string(),
            visible: false,
            ..Case::default()
        });

        set_state(&mut button, "player.mute".to_string());
        let whens: Vec<&str> = button.cases.iter().map(|case| case.when.as_str()).collect();
        assert_eq!(whens, ["muted"]);
        assert_eq!(button.state, "player.mute");

        set_state(&mut button, String::new());
        assert!(button.cases.is_empty(), "None takes every case with it");
        assert!(button.state.is_empty());
    }

    #[test]
    fn picking_a_state_fills_an_empty_on_click_only() {
        let mut button = play_button();
        set_state(&mut button, "player.repeat".to_string());
        assert_eq!(button.action, "cycle_loop");

        button.action = "next_track".to_string();
        set_state(&mut button, "player.playback".to_string());
        assert_eq!(button.action, "next_track");

        let mut grid = item(json!({ "id": "g", "content": { "kind": "grid", "dock": null } }));
        set_state(&mut grid, "player.playback".to_string());
        assert!(grid.action.is_empty(), "a grid's panels own its click");
    }

    #[test]
    fn a_case_edited_back_to_plain_saves_as_no_entry() {
        let mut button = item(json!({
            "id": "b",
            "state": "player.mute",
            "content": { "kind": "image", "asset": "a" },
        }));

        edit_case(&mut button, "muted", |case| case.visible = false);
        assert_eq!(button.cases.len(), 1);
        assert!(!button.cases[0].visible);

        edit_case(&mut button, "muted", |case| case.opacity = 0.3);
        assert_eq!(button.cases.len(), 1, "the same case is edited in place");

        edit_case(&mut button, "muted", |case| {
            case.visible = true;
            case.opacity = 1.;
        });
        assert!(button.cases.is_empty());

        let saved = serde_json::to_value(&button).unwrap();
        assert!(saved.get("cases").is_none());
    }

    #[test]
    fn marks_are_wanted_only_by_the_favourite() {
        let heart = item(json!({
            "id": "h",
            "state": "player.favourite",
            "content": { "kind": "image", "asset": "a" },
        }));
        let canvas = CanvasState {
            items: vec![play_button()],
            ..CanvasState::default()
        };
        assert!(follows(&canvas));
        assert!(!needs_marks(&canvas));

        // Inside a group too.
        let canvas = CanvasState {
            items: vec![item(json!({
                "id": "g",
                "content": { "kind": "group", "items": [heart] },
            }))],
            ..CanvasState::default()
        };
        assert!(needs_marks(&canvas));
    }

    fn viewport(width: f32, height: f32) -> Viewport {
        Viewport { width, height }
    }

    #[test]
    fn the_shape_turns_only_past_the_band_either_side_of_square() {
        let next = |held, width| Shape::next(held, viewport(width, 1000.));

        // A first reading splits at square.
        assert_eq!(next(None, 1001.), Some(Shape::Wide));
        assert_eq!(next(None, 999.), Some(Shape::Tall));

        // Narrowing, wide holds down to 0.943.
        assert_eq!(next(Some(Shape::Wide), 970.), Some(Shape::Wide));
        assert_eq!(next(Some(Shape::Wide), 944.), Some(Shape::Wide));
        assert_eq!(next(Some(Shape::Wide), 940.), Some(Shape::Tall));

        // Widening, tall holds up to 1.06.
        assert_eq!(next(Some(Shape::Tall), 1030.), Some(Shape::Tall));
        assert_eq!(next(Some(Shape::Tall), 1060.), Some(Shape::Tall));
        assert_eq!(next(Some(Shape::Tall), 1061.), Some(Shape::Wide));

        // A box with no area keeps what it had.
        assert_eq!(
            Shape::next(Some(Shape::Tall), viewport(0., 1000.)),
            Some(Shape::Tall)
        );
        assert_eq!(Shape::next(None, viewport(800., 0.)), None);
    }

    fn sense(values: &Values) -> Sense<'_> {
        Sense {
            values,
            art: false,
            shape: None,
        }
    }

    #[test]
    fn a_field_reads_empty_exactly_when_its_placeholder_renders_nothing() {
        use crate::canvas::text::CanvasField;

        let values: Values = vec![
            (CanvasField::Title, "Roygbiv".into()),
            (CanvasField::Album, "   ".into()),
            (CanvasField::Year, "1998".into()),
        ];
        let read = |id| read_canvas(id, &sense(&values));

        assert_eq!(read("track.title"), Some("present"));
        assert_eq!(read("track.year"), Some("present"));
        // A tag of spaces trims away, as a text item's hole does.
        assert_eq!(read("track.album"), Some("empty"));
        assert_eq!(read("track.artist"), Some("empty"));
        assert_eq!(read("track.nonsense"), None);

        // Nothing playing leaves every field empty.
        let none = Values::new();
        assert_eq!(read_canvas("track.title", &sense(&none)), Some("empty"));
    }

    #[test]
    fn art_and_shape_read_what_the_canvas_holds() {
        let values = Values::new();
        let mut held = sense(&values);
        assert_eq!(read_canvas(ART, &held), Some("none"));
        assert_eq!(read_canvas(SHAPE, &held), Some("wide"));

        held.art = true;
        held.shape = Some(Shape::Tall);
        assert_eq!(read_canvas(ART, &held), Some("present"));
        assert_eq!(read_canvas(SHAPE, &held), Some("tall"));

        // Custom Controls' states aren't the canvas's to answer.
        assert_eq!(read_canvas("player.playback", &held), None);
    }

    #[test]
    fn every_filled_placeholder_has_a_field_state_and_every_state_is_listed_once() {
        for placeholder in text::PLACEHOLDERS {
            let id = format!("{TRACK}{}", placeholder.trim_matches('%'));
            assert!(spec(&id).is_some(), "{placeholder} has no {id}");
        }

        for canvas in CANVAS_STATES {
            assert!(
                buttons::STATES.iter().all(|spec| spec.id != canvas.id),
                "{} is in both tables",
                canvas.id
            );
        }

        let listed: Vec<&str> = picker_groups()
            .iter()
            .flat_map(|(_, specs)| specs.iter().map(|spec| spec.id))
            .collect();
        let every = buttons::STATES.iter().chain(CANVAS_STATES);
        assert_eq!(listed.len(), every.clone().count());
        for spec in every {
            assert_eq!(
                listed.iter().filter(|id| **id == spec.id).count(),
                1,
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn the_shape_cuts_unless_the_item_asks_for_an_ease() {
        let mut shaped = item(json!({ "id": "s", "state": SHAPE }));
        assert_eq!(ease_length(&shaped, false), Duration::ZERO);

        shaped.ease = Some(120);
        assert_eq!(ease_length(&shaped, false), Duration::from_millis(120));
        assert_eq!(ease_length(&shaped, true), Duration::ZERO);

        let playing = item(json!({ "id": "p", "state": "player.playback" }));
        assert_eq!(ease_length(&playing, false), fades::EASE);
    }

    /// A deck that sits on the left as a column, and across the top as a
    /// row when tall.
    fn deck() -> CanvasItem {
        item(json!({
            "id": "deck",
            "state": SHAPE,
            "place": { "anchor": "left", "size": [{ "stage": 0.3 }, { "stage": 1.0 }] },
            "stack": { "axis": "y", "gap": 10 },
            "cases": [{
                "when": "tall",
                "place": { "anchor": "top", "size": [{ "stage": 1.0 }, { "stage": 0.4 }] },
                "stack": { "axis": "x", "gap": 20 },
            }],
            "content": { "kind": "group", "items": [
                { "id": "cover", "content": { "kind": "image", "asset": "c" } },
                { "id": "list", "content": { "kind": "grid", "dock": { "big": true } } },
            ] },
        }))
    }

    fn canvas(items: Vec<CanvasItem>) -> CanvasState {
        CanvasState {
            items,
            ..CanvasState::default()
        }
    }

    #[test]
    fn a_case_that_moves_places_a_shallow_copy() {
        let deck = deck();
        let wide = cases(&[(SHAPE, "wide")]);
        let tall = cases(&[(SHAPE, "tall")]);

        let as_wide = placed(&deck, resolve(&deck, &wide, None).case);
        assert!(matches!(as_wide, Cow::Borrowed(_)), "no entry, no copy");

        let as_tall = placed(&deck, resolve(&deck, &tall, None).case);
        assert_eq!(as_tall.place.anchor, rox_canvas::Anchor::Top);
        assert_eq!(as_tall.stack.map(|stack| stack.gap), Some(20.));
        // What the group holds is left behind, to be read from the item.
        assert!(matches!(&as_tall.content, Content::Group { items } if items.is_empty()));
    }

    #[test]
    fn a_stacks_children_are_copied_only_when_one_moves() {
        let Content::Group { items } = deck().content else {
            unreachable!()
        };
        assert!(matches!(
            placed_children(&items, &cases(&[(SHAPE, "tall")]), None),
            Cow::Borrowed(_)
        ));

        let mut moving = items.clone();
        moving[1].state = SHAPE.to_string();
        moving[1].cases = vec![Case {
            when: "tall".into(),
            place: Some(rox_canvas::PlaceOverride {
                size: Some([rox_canvas::Length::Units(5.), rox_canvas::Length::Units(6.)]),
                ..Default::default()
            }),
            ..Case::default()
        }];
        let laid = placed_children(&moving, &cases(&[(SHAPE, "tall")]), None);
        assert_eq!(laid[1].place.size[0], rox_canvas::Length::Units(5.));
        // A grid's dock isn't copied for it.
        assert!(matches!(&laid[1].content, Content::Grid { dock } if dock.is_null()));
    }

    fn view(case: &'static str, preview: Option<&str>) -> View {
        View {
            cases: cases(&[(SHAPE, case)]),
            preview: preview.map(|case| ("deck".to_string(), case.to_string())),
            across: None,
        }
    }

    #[test]
    fn an_edit_under_a_previewed_case_lands_in_that_case_only() {
        let mut held = canvas(vec![deck()]);
        let own = held.items[0].place;

        // Previewing tall from a wide canvas: the drag lands in tall.
        edit_drawn(&mut held, &view("wide", Some("tall")), |drawn| {
            let deck = drawn.find_mut("deck").unwrap();
            assert_eq!(
                deck.place.anchor,
                rox_canvas::Anchor::Top,
                "edits see it drawn"
            );
            deck.place.position = rox_canvas::Position::units([0., 30., 0.]);
        });

        let deck = &held.items[0];
        assert_eq!(deck.place, own, "the item's own place is untouched");
        let tall = deck.cases[0].place.expect("tall keeps its place");
        assert_eq!(
            tall.position,
            Some(rox_canvas::Position::units([0., 30., 0.]))
        );
        assert_eq!(tall.anchor, Some(rox_canvas::Anchor::Top));

        // The children's things are kept as they were: the grid's dock too.
        let Content::Group { items } = &deck.content else {
            unreachable!()
        };
        assert!(matches!(&items[1].content, Content::Grid { dock } if dock["big"] == true));
    }

    #[test]
    fn an_edit_where_no_case_moves_the_item_lands_on_the_item() {
        let mut held = canvas(vec![deck()]);
        let tall = held.items[0].cases[0].clone();

        // Live and wide, no preview: wide has no place, so the item's own.
        edit_drawn(&mut held, &view("wide", None), |drawn| {
            drawn.find_mut("deck").unwrap().place.scale = 2.;
        });
        assert_eq!(held.items[0].place.scale, 2.);
        assert_eq!(held.items[0].cases[0], tall);

        // Live and tall, which moves it: the edit lands in tall.
        edit_drawn(&mut held, &view("tall", None), |drawn| {
            let deck = drawn.find_mut("deck").unwrap();
            deck.stack = deck
                .stack
                .map(|stack| rox_canvas::Stack { gap: 4., ..stack });
        });
        assert_eq!(held.items[0].stack.map(|stack| stack.gap), Some(10.));
        assert_eq!(
            held.items[0].cases[0].stack.map(|stack| stack.gap),
            Some(4.)
        );
    }

    #[test]
    fn a_canvas_where_nothing_moves_is_edited_straight() {
        let mut plain = canvas(vec![play_button()]);
        let before = plain.clone();
        let mut seen = None;
        edit_drawn(&mut plain, &View::default(), |canvas| {
            seen = Some(canvas.clone());
            canvas.items[0].opacity = 0.5;
        });

        assert_eq!(seen, Some(before));
        assert!(matches!(drawn(&plain, &View::default()), Cow::Borrowed(_)));
        assert_eq!(plain.items[0].opacity, 0.5);
    }

    #[test]
    fn a_reset_drops_the_cases_place_and_keeps_the_items() {
        let mut held = canvas(vec![deck()]);
        let own = held.items[0].place;

        edit_drawn(&mut held, &view("tall", None), |drawn| {
            let deck = drawn.find_mut("deck").unwrap();
            edit_case(deck, "tall", |case| case.place = None);
        });

        assert_eq!(held.items[0].place, own);
        assert_eq!(held.items[0].cases[0].place, None);
        assert!(held.items[0].cases[0].stack.is_some());
    }

    #[test]
    fn the_shape_preview_reaches_every_item_following_it() {
        let mut follower = play_button();
        follower.id = "other".into();
        follower.state = SHAPE.to_string();
        follower.cases.clear();
        let mut held = canvas(vec![deck(), follower]);

        let across = View {
            cases: cases(&[(SHAPE, "tall")]),
            preview: None,
            across: Some(SHAPE),
        };
        edit_drawn(&mut held, &across, |drawn| {
            drawn.find_mut("other").unwrap().place.scale = 3.;
        });

        // Previewed with no place yet, it gets one in that case.
        let other = &held.items[1];
        assert_eq!(other.place.scale, 1.);
        assert_eq!(other.cases[0].when, "tall");
        assert_eq!(other.cases[0].place.and_then(|place| place.scale), Some(3.));
    }
}
