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

use std::collections::HashMap;

use rox_canvas::{CanvasItem, CanvasState, Case, Content, Live};
use rox_panel_api::buttons::{self, StateSpec};

use crate::panels::controls;

/// The opacity an item its case hides draws at in design mode, so it can
/// still be found and picked.
pub(crate) const GHOST: f32 = 0.25;

/// The case each followed state is in, by state id.
pub(crate) type Cases = HashMap<&'static str, &'static str>;

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

    controls::state_spec(&item.state)
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
pub(crate) fn read(canvas: &CanvasState, live: &buttons::Live) -> Cases {
    let mut cases = Cases::new();
    canvas.walk(&mut |item, _| {
        let Some(spec) = followed(item) else {
            return;
        };

        if !cases.contains_key(spec.id)
            && let Some(case) = buttons::read_state(spec.id, live)
        {
            cases.insert(spec.id, case);
        }
    });
    cases
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
    let Some(spec) = followed(item) else {
        return Applied::default();
    };

    let known = |id: &&str| spec.cases.iter().any(|case| case.id == *id);
    let current = preview
        .map(|preview| preview.split_once(':').map_or(preview, |(case, _)| case))
        .filter(known)
        .or_else(|| cases.get(spec.id).copied());
    let Some(current) = current else {
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
    let spec = controls::state_spec(&state);
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
}
