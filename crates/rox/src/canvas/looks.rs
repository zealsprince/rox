//! Hover and pressed looks on click items (ADR 33). An item with a click
//! command can carry a look for while the pointer is over it and one for
//! while it's pressed, laid over whichever case its state is in, the pressed
//! look on top of the hover one. Off design mode only, and only on the item
//! a click there would run: the front item under the pointer by the same
//! walk, or the group whose command covers it. Nothing but the pointer
//! hovers, so the keyboard never shows a look.
//!
//! A group's command covers every item inside it that has no command of its
//! own, and the looks reach them the same way: hovering the group shows each
//! covered item's own look, so a button's body can light while its glyph
//! stays as it is. A case can carry its own hover and pressed looks, which
//! stand in for the item's while that case is drawn.
//!
//! Each look eases in and out over [`EASE`]. Opacity and a text color blend,
//! and an image crossfades to the look's. A look asks for frames only while
//! an ease runs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, ElementId, SharedString, Window, div, prelude::*};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_canvas::{CanvasItem, CanvasState, Case, Content, Look};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, setting_row};
use rox_panel_kit::Tip;
use rox_panel_kit::ui as settings_ui;

use crate::canvas::fades::Faded;
use crate::canvas::overlay;
use crate::canvas::states;
use crate::panels::canvas::{self, CanvasPanel};

/// How long a look takes to come in or go. Short enough that a quick click
/// still shows its press.
pub(crate) const EASE: Duration = Duration::from_millis(140);

/// How far each look shows on an item this frame, hover then pressed, 0 to
/// 1. An item missing from it shows neither.
pub(crate) type Shown = HashMap<String, [f32; 2]>;

/// Whether the item carries a look, its cases' included.
pub(crate) fn wears(item: &CanvasItem) -> bool {
    item.hover.is_some()
        || item.pressed.is_some()
        || item
            .cases
            .iter()
            .any(|case| case.hover.is_some() || case.pressed.is_some())
}

/// Whether hovering `item` shows any look: its own, or one on an item its
/// command covers inside it.
pub(crate) fn worn(item: &CanvasItem) -> bool {
    if wears(item) {
        return true;
    }

    match &item.content {
        Content::Group { items } => items
            .iter()
            .any(|child| !canvas::commands(child) && worn(child)),
        _ => false,
    }
}

/// Whether a command covers the item `id`: its own, or a group's around it.
/// Only then can its looks show.
pub(crate) fn covered(canvas: &CanvasState, id: &str) -> bool {
    fn walk(items: &[CanvasItem], id: &str, above: bool) -> Option<bool> {
        items.iter().find_map(|item| {
            let here = above || canvas::commands(item);
            if item.id == id {
                return Some(here);
            }

            match &item.content {
                Content::Group { items } => walk(items, id, here),
                _ => None,
            }
        })
    }

    walk(&canvas.items, id, false).unwrap_or(false)
}

/// How far the looks show on `item`: its own weights when it's the hovered
/// item or previewed, else the group's around it that its command covers.
pub(crate) fn weights(
    item: &CanvasItem,
    shown: &Shown,
    inherited: Option<[f32; 2]>,
) -> Option<[f32; 2]> {
    // The command list is a scan, so it's asked only under a lit group.
    match shown.get(&item.id) {
        Some(weights) => Some(*weights),
        None => inherited.filter(|_| !canvas::commands(item)),
    }
}

/// One look's ease on one item.
#[derive(Clone, Copy, Debug)]
struct Edge {
    on: bool,
    /// The weight when it last flipped, and when. None once it never moved.
    from: f32,
    at: Option<Instant>,
}

impl Edge {
    const REST: Edge = Edge {
        on: false,
        from: 0.,
        at: None,
    };

    fn weight(&self, now: Instant) -> f32 {
        let target = if self.on { 1. } else { 0. };
        let Some(at) = self.at else {
            return target;
        };

        let t = now.saturating_duration_since(at).as_secs_f32() / EASE.as_secs_f32();
        if t >= 1. {
            return target;
        }

        let eased = t * t * (3. - 2. * t);
        self.from + (target - self.from) * eased
    }

    /// Starts an ease from wherever the last one stood.
    fn set(&mut self, on: bool, now: Instant) {
        if self.on == on {
            return;
        }

        self.from = self.weight(now);
        self.on = on;
        self.at = Some(now);
    }

    fn settled(&self, now: Instant) -> bool {
        self.at
            .is_none_or(|at| now.saturating_duration_since(at) >= EASE)
    }
}

/// The pointer's looks across frames: which item is hovered and pressed,
/// and every ease still running.
#[derive(Default)]
pub(crate) struct Looks {
    hovered: Option<String>,
    pressed: Option<String>,
    eases: HashMap<String, [Edge; 2]>,
}

impl Looks {
    /// Points the looks at `hovered`, the item a click there would run, and
    /// `pressed`, the one a held press landed on. A press shows only while
    /// the pointer is still over its item. True when anything moved, which
    /// is when the canvas needs a frame.
    pub(crate) fn point(
        &mut self,
        hovered: Option<&str>,
        pressed: Option<&str>,
        now: Instant,
    ) -> bool {
        let pressed = pressed.filter(|id| Some(*id) == hovered);
        if self.hovered.as_deref() == hovered && self.pressed.as_deref() == pressed {
            return false;
        }

        self.hovered = hovered.map(str::to_string);
        self.pressed = pressed.map(str::to_string);

        if let Some(id) = hovered {
            self.eases.entry(id.to_string()).or_insert([Edge::REST; 2]);
        }
        for (id, [hover, press]) in &mut self.eases {
            hover.set(Some(id.as_str()) == hovered, now);
            press.set(Some(id.as_str()) == pressed, now);
        }

        true
    }

    /// Nothing hovered or pressed, as when the pointer leaves the window.
    pub(crate) fn clear(&mut self, now: Instant) -> bool {
        self.point(None, None, now)
    }

    /// Each item's weights this frame, and whether an ease still runs. Items
    /// back at rest are dropped. `still` lands every ease at once, for
    /// Reduce Motion.
    pub(crate) fn shown(&mut self, now: Instant, still: bool) -> (Shown, bool) {
        if still {
            for edge in self.eases.values_mut().flatten() {
                edge.at = None;
            }
        }

        self.eases
            .retain(|_, edges| edges.iter().any(|edge| edge.on || !edge.settled(now)));

        let more = self.eases.values().flatten().any(|edge| !edge.settled(now));
        let shown = self
            .eases
            .iter()
            .map(|(id, [hover, press])| (id.clone(), [hover.weight(now), press.weight(now)]))
            .collect();

        (shown, more)
    }
}

/// The looks this frame: the pointer's off design mode, eased unless
/// `still`, or the one the inspector previews in it. Frames are asked for
/// only while an ease runs.
pub(crate) fn this_frame(
    looks: &mut Looks,
    design: bool,
    preview: Option<(&str, &str)>,
    still: bool,
    window: &mut Window,
) -> Shown {
    if design {
        let mut shown = Shown::new();
        if let Some((id, case)) = preview {
            self::preview(&mut shown, id, case);
        }
        return shown;
    }

    let (shown, more) = looks.shown(Instant::now(), still);
    if more {
        window.request_animation_frame();
    }
    shown
}

/// The inspector's preview of a look on the selected item, at full weight,
/// named where a case's would be: `:hover`, or a case's own look as
/// `playing:hover`. No state's case id holds a colon, so a case preview
/// never reads as a look.
pub(crate) fn preview(shown: &mut Shown, id: &str, case: &str) {
    let look = case.split_once(':').map(|(_, look)| look);
    let weights = match look {
        Some("hover") => [1., 0.],
        Some("pressed") => [1., 1.],
        _ => return,
    };
    shown.insert(id.to_string(), weights);
}

/// The looks `item` wears in `case`: the case's own where it has one, else
/// the item's.
fn looks_in<'a>(item: &'a CanvasItem, case: Option<&'a Case>) -> [Option<&'a Look>; 2] {
    [
        case.and_then(|case| case.hover.as_ref())
            .or(item.hover.as_ref()),
        case.and_then(|case| case.pressed.as_ref())
            .or(item.pressed.as_ref()),
    ]
}

/// `faded` with the looks `item` wears in `case` laid over at `weights`. A
/// look's opacity multiplies, so the pressed look's goes on top of the hover
/// look's. Its image crossfades in over what the case draws, and its color
/// blends in.
pub(crate) fn over<'a>(
    item: &'a CanvasItem,
    case: Option<&'a Case>,
    mut faded: Faded<'a>,
    weights: [f32; 2],
) -> Faded<'a> {
    let named = |text: &'a str| Some(text).filter(|text| !text.is_empty());

    for (look, weight) in looks_in(item, case).into_iter().zip(weights) {
        let Some(look) = look.filter(|_| weight > 0.) else {
            continue;
        };

        // A hand-edited file can hold any number here.
        let opacity = if look.opacity.is_finite() {
            look.opacity.clamp(0., 1.)
        } else {
            1.
        };
        faded.opacity *= 1. + (opacity - 1.) * weight;

        if let Some(asset) = named(&look.asset) {
            faded.pictures.blend(Some(asset), weight);
        }
        if let Some(color) = named(&look.color) {
            faded.colors.blend(Some(color), weight);
        }
    }

    faded
}

/// Which look an inspector row edits.
#[derive(Clone, Copy, PartialEq)]
enum Which {
    Hover,
    Pressed,
}

impl Which {
    fn key(self) -> &'static str {
        match self {
            Which::Hover => "hover",
            Which::Pressed => "pressed",
        }
    }
}

/// Where a look an inspector row edits lives: on the item, or on one of its
/// cases.
#[derive(Clone, Copy, PartialEq)]
struct Slot {
    which: Which,
    case: Option<&'static str>,
}

impl Slot {
    fn of(self, item: &CanvasItem) -> Option<&Look> {
        let pick = |hover, pressed| match self.which {
            Which::Hover => hover,
            Which::Pressed => pressed,
        };

        match self.case {
            None => pick(&item.hover, &item.pressed).as_ref(),
            Some(when) => item
                .cases
                .iter()
                .find(|case| case.when == when)
                .and_then(|case| pick(&case.hover, &case.pressed).as_ref()),
        }
    }

    /// Unique among the inspector's rows, for element ids and scrubs.
    fn key(self) -> String {
        match self.case {
            None => self.which.key().to_string(),
            Some(when) => format!("{when}:{}", self.which.key()),
        }
    }

    fn preview(self) -> String {
        format!("{}:{}", self.case.unwrap_or(""), self.which.key())
    }

    /// The undo key a run of edits to `field` merges under.
    fn undo_key(self, field: &str) -> &'static str {
        match (self.case.is_some(), self.which, field) {
            (false, Which::Hover, "opacity") => "look.hover.opacity",
            (false, Which::Hover, _) => "look.hover.color",
            (false, Which::Pressed, "opacity") => "look.pressed.opacity",
            (false, Which::Pressed, _) => "look.pressed.color",
            (true, Which::Hover, "opacity") => "case.look.hover.opacity",
            (true, Which::Hover, _) => "case.look.hover.color",
            (true, Which::Pressed, "opacity") => "case.look.pressed.opacity",
            (true, Which::Pressed, _) => "case.look.pressed.color",
        }
    }
}

/// Edits one look, making it first if there's none. A look edited back to
/// changing nothing goes, so an untouched one saves as nothing at all, and a
/// case left with nothing else goes with it.
fn edit_look(item: &mut CanvasItem, slot: Slot, edit: impl FnOnce(&mut Look)) {
    let apply = |hover: &mut Option<Look>, pressed: &mut Option<Look>| {
        let held = match slot.which {
            Which::Hover => hover,
            Which::Pressed => pressed,
        };
        let look = held.get_or_insert_with(Look::default);
        edit(look);

        if look.is_plain() {
            *held = None;
        }
    };

    match slot.case {
        None => apply(&mut item.hover, &mut item.pressed),
        Some(when) => {
            states::edit_case(item, when, |case| apply(&mut case.hover, &mut case.pressed))
        }
    }
}

/// Through `change`, so every look edit is an undo step and a save. A run
/// of edits to one field merges into one step.
fn change_look(
    this: &mut CanvasPanel,
    id: &str,
    slot: Slot,
    field: &'static str,
    edit: impl FnOnce(&mut Look) + 'static,
    cx: &mut Context<CanvasPanel>,
) {
    let id = id.to_string();
    this.change_later(
        slot.undo_key(field),
        move |canvas| {
            if let Some(item) = canvas.find_mut(&id) {
                edit_look(item, slot, edit);
            }
        },
        cx,
    );
}

/// The two looks' blocks, for an item a command covers: its own, or a
/// group's around it. None for one with neither, whose looks would never
/// show.
pub(crate) fn rows(
    panel: &CanvasPanel,
    item: &CanvasItem,
    cx: &mut Context<CanvasPanel>,
) -> Option<AnyElement> {
    blocks(panel, item, None, cx)
}

/// A case's own two looks, in its block, which stand in for the item's own
/// while that case is drawn. None where no command covers the item.
pub(crate) fn case_rows(
    panel: &CanvasPanel,
    item: &CanvasItem,
    when: &'static str,
    cx: &mut Context<CanvasPanel>,
) -> Option<AnyElement> {
    blocks(panel, item, Some(when), cx)
}

fn blocks(
    panel: &CanvasPanel,
    item: &CanvasItem,
    case: Option<&'static str>,
    cx: &mut Context<CanvasPanel>,
) -> Option<AnyElement> {
    if !covered(&panel.config.canvas, &item.id) {
        return None;
    }

    let hover = Slot {
        which: Which::Hover,
        case,
    };
    let pressed = Slot {
        which: Which::Pressed,
        case,
    };

    Some(
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(block(panel, item, hover, cx))
            .child(block(panel, item, pressed, cx))
            .into_any_element(),
    )
}

/// One look: its opacity, and the image or color its kind takes, the same
/// rows a case has. A look left alone reads as the case under it.
fn block(
    panel: &CanvasPanel,
    item: &CanvasItem,
    slot: Slot,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let look = slot.of(item).cloned().unwrap_or_default();
    let shows = slot.preview();
    let previewing = panel.inspector.preview() == Some((item.id.as_str(), shows.as_str()));

    let me = cx.entity().downgrade();
    let preview_id = item.id.clone();
    let mut eye = settings_ui::icon_button(icons::EYE, false, move |_, _, cx| {
        let id = preview_id.clone();
        me.update(cx, |this, cx| {
            this.inspector.toggle_preview(id, &slot.preview());
            cx.notify();
        })
        .ok();
    })
    .keyed(ElementId::Name(
        format!("canvas-look-preview:{}", slot.key()).into(),
    ));
    if previewing {
        eye = eye.bg(palette::alpha(palette::accent(), 0x40));
    }
    let eye = Tip::keyed(
        ElementId::Name(format!("canvas-look-preview-tip:{}", slot.key()).into()),
        rox_i18n::t!("canvas-look-preview"),
    )
    .apply(div().child(eye));

    let label = match (slot.which, slot.case.is_some()) {
        (Which::Hover, false) => rox_i18n::t!("canvas-look-hover"),
        (Which::Pressed, false) => rox_i18n::t!("canvas-look-pressed"),
        (Which::Hover, true) => rox_i18n::t!("canvas-case-look-hover"),
        (Which::Pressed, true) => rox_i18n::t!("canvas-case-look-pressed"),
    };
    let header = settings_ui::block_header(
        div()
            .text_xs()
            .text_color(if previewing {
                palette::accent()
            } else {
                palette::text_muted()
            })
            .child(label),
        eye,
    );

    let mut body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(header)
        .child(opacity_row(panel, item, slot, look.opacity, cx));

    match &item.content {
        Content::Image { .. } => body = body.child(image_row(&item.id, slot, &look, cx)),
        Content::Text { .. } => body = body.child(color_row(&item.id, slot, &look, cx)),
        _ => {}
    }

    div()
        .id(SharedString::from(format!("canvas-look-{}", slot.key())))
        .child(settings_ui::nested(body))
        .into_any_element()
}

fn opacity_row(
    panel: &CanvasPanel,
    item: &CanvasItem,
    slot: Slot,
    opacity: f32,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let scrub = panel.inspector.scrub(&format!("look:{}", slot.key()));

    let id = item.id.clone();
    let slider = panel::value_slider_edit_sized(
        &scrub,
        panel.inspector.value_edit(),
        opacity,
        rox_i18n::format::format_percent((opacity * 100.).round() as f64),
        format!("{:.0}", opacity * 100.),
        1.,
        panel::SliderWidth::Fill,
        panel::SLIDER_STEP,
        |v| v / 100.,
        move |this: &mut CanvasPanel, f, cx| {
            change_look(
                this,
                &id,
                slot,
                "opacity",
                move |look| look.opacity = f.clamp(0., 1.),
                cx,
            )
        },
        cx,
    );

    // As a case's row: nested a step in, a fixed column runs the readout
    // off the edge in a long language.
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_MD)
        .child(div().flex_none().child(rox_i18n::t!("canvas-item-opacity")))
        .child(div().flex_1().min_w_0().child(slider))
        .into_any_element()
}

/// The image picker, headed by the image the case shows.
fn image_row(id: &str, slot: Slot, look: &Look, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let label = if look.asset.is_empty() {
        rox_i18n::t!("canvas-case-same-image")
    } else {
        SharedString::from(look.asset.clone())
    };

    let me = cx.entity().downgrade();
    let id = id.to_string();
    let pick = settings_ui::small_button(label, icons::IMAGE, false, move |_, window, cx| {
        let position = window.mouse_position();
        let me = me.clone();
        let id = id.clone();
        let menu = PopupMenu::build(window, cx, {
            let me = me.clone();
            move |mut menu, _, _| {
                let mut choices = vec![(
                    String::new(),
                    rox_i18n::t!("canvas-case-same-image").to_string(),
                )];
                choices.extend(canvas::image_assets());

                for (asset, file) in choices {
                    let me = me.clone();
                    let id = id.clone();
                    menu = menu.item(PopupMenuItem::new(SharedString::from(file)).on_click(
                        move |_, window, cx| {
                            let id = id.clone();
                            let asset = asset.clone();
                            me.update(cx, |this, cx| {
                                this.change(
                                    |canvas| {
                                        if let Some(item) = canvas.find_mut(&id) {
                                            edit_look(item, slot, |look| look.asset = asset);
                                        }
                                    },
                                    window,
                                    cx,
                                )
                            })
                            .ok();
                        },
                    ));
                }
                menu
            }
        });
        me.update(cx, |this, cx| this.open_menu(position, menu, window, cx))
            .ok();
    })
    .keyed(ElementId::Name(
        format!("canvas-look-image:{}", slot.key()).into(),
    ));

    setting_row(rox_i18n::t!("canvas-image-asset"), None, pick).into_any_element()
}

/// The color picker, headed by the color the case gives.
fn color_row(id: &str, slot: Slot, look: &Look, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let mut roles = vec![(String::new(), rox_i18n::t!("canvas-case-same-color"))];
    roles.extend(overlay::text_roles(&look.color));

    let id = id.to_string();
    let picker = panel::picker(
        // Each block carries its own element id, so these repeat safely.
        match slot.which {
            Which::Hover => "canvas-look-color-hover",
            Which::Pressed => "canvas-look-color-pressed",
        },
        look.color.clone(),
        roles,
        false,
        move |this: &mut CanvasPanel, role: String, cx| {
            change_look(this, &id, slot, "color", move |look| look.color = role, cx)
        },
        cx,
    );

    setting_row(rox_i18n::t!("canvas-text-color"), None, picker).into_any_element()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::canvas::states::Applied;

    fn button() -> CanvasItem {
        serde_json::from_value(json!({
            "id": "play",
            "action": "toggle_playback",
            "hover": { "opacity": 0.5, "asset": "lit" },
            "pressed": { "opacity": 0.5, "asset": "down" },
            "content": { "kind": "image", "asset": "play" },
        }))
        .expect("an item parses")
    }

    #[test]
    fn a_hover_eases_in_and_out_and_then_asks_for_nothing() {
        let mut looks = Looks::default();
        let start = Instant::now();
        assert_eq!(looks.shown(start, false), (Shown::new(), false));

        assert!(looks.point(Some("play"), None, start));
        assert!(
            !looks.point(Some("play"), None, start),
            "the same item again moves nothing"
        );

        let (shown, more) = looks.shown(start + EASE / 2, false);
        assert!(more);
        let [hover, press] = shown["play"];
        assert!(hover > 0. && hover < 1.);
        assert_eq!(press, 0.);

        let (shown, more) = looks.shown(start + EASE, false);
        assert_eq!(shown["play"], [1., 0.]);
        assert!(!more, "a settled hover draws no frames");

        let left = start + EASE * 2;
        assert!(looks.clear(left));
        let (shown, more) = looks.shown(left + EASE / 2, false);
        assert!(more && shown["play"][0] > 0.);

        let (shown, more) = looks.shown(left + EASE, false);
        assert!(shown.is_empty(), "an item back at rest is dropped");
        assert!(!more);
    }

    #[test]
    fn a_press_shows_only_over_its_own_item() {
        let mut looks = Looks::default();
        let now = Instant::now();

        looks.point(Some("play"), Some("play"), now);
        assert_eq!(looks.shown(now + EASE, false).0["play"], [1., 1.]);

        // Dragged off onto another button, the press lets go and that one
        // hovers.
        let later = now + EASE * 2;
        looks.point(Some("next"), Some("play"), later);
        let (shown, _) = looks.shown(later + EASE, false);
        assert!(!shown.contains_key("play"));
        assert_eq!(shown["next"], [1., 0.]);
    }

    #[test]
    fn leaving_partway_eases_back_from_where_it_stood() {
        let mut looks = Looks::default();
        let start = Instant::now();

        looks.point(Some("play"), None, start);
        let midway = start + EASE / 2;
        let stood = looks.shown(midway, false).0["play"][0];
        looks.clear(midway);

        let (shown, _) = looks.shown(midway + Duration::from_millis(1), false);
        let now = shown["play"][0];
        assert!(now <= stood && now > stood - 0.1, "{stood} then {now}");
    }

    #[test]
    fn held_still_a_look_switches_at_once_and_asks_for_no_frames() {
        let mut looks = Looks::default();
        let start = Instant::now();

        looks.point(Some("play"), Some("play"), start);
        let (shown, more) = looks.shown(start, true);
        assert_eq!(shown["play"], [1., 1.]);
        assert!(!more);

        looks.clear(start);
        let (shown, more) = looks.shown(start, true);
        assert!(shown.is_empty(), "gone the same frame it left");
        assert!(!more);
    }

    fn rest() -> Faded<'static> {
        Faded::at_once(&Applied::default())
    }

    #[test]
    fn the_pressed_look_goes_on_top_of_the_hover_look() {
        let item = button();

        assert_eq!(over(&item, None, rest(), [0., 0.]), rest());

        let hovered = over(&item, None, rest(), [1., 0.]);
        assert_eq!(hovered.opacity, 0.5);
        assert_eq!(hovered.pictures.parts(), &[(Some("lit"), 1.)]);

        let pressed = over(&item, None, rest(), [1., 1.]);
        assert_eq!(pressed.opacity, 0.25);
        assert_eq!(pressed.pictures.top(), Some("down"));

        // A quarter in, the look's image holds a quarter of the mix over the
        // item's own, and the opacity is on its way.
        let early = over(&item, None, rest(), [0.25, 0.]);
        assert_eq!(early.pictures.parts(), &[(None, 0.75), (Some("lit"), 0.25)]);
        assert!((early.opacity - 0.875).abs() < 1e-6);
    }

    #[test]
    fn a_look_left_alone_keeps_what_the_case_gives() {
        let mut item = button();
        item.pressed = Some(Look {
            opacity: 0.8,
            ..Look::default()
        });
        let case = Faded::at_once(&Applied {
            opacity: 0.5,
            asset: Some("pause"),
            ..Applied::default()
        });

        let pressed = over(&item, None, case, [1., 1.]);
        assert_eq!(
            pressed.pictures.top(),
            Some("lit"),
            "the hover's image stays"
        );
        assert!((pressed.opacity - 0.5 * 0.5 * 0.8).abs() < 1e-6);

        item.hover = None;
        assert_eq!(
            over(&item, None, case, [1., 1.]).pictures.top(),
            Some("pause")
        );

        // A file can say anything; what draws stays in range.
        item.pressed = Some(Look {
            opacity: f32::NAN,
            ..Look::default()
        });
        assert_eq!(over(&item, None, case, [1., 1.]).opacity, 0.5);
    }

    #[test]
    fn a_cases_own_look_stands_in_for_the_items() {
        let item: CanvasItem = serde_json::from_value(json!({
            "id": "play-body",
            "state": "player.playback",
            "hover": { "asset": "gel_lit" },
            "cases": [{ "when": "playing", "asset": "gel_green", "hover": { "asset": "gel_green_lit" } }],
            "content": { "kind": "image", "asset": "gel" },
        }))
        .expect("an item parses");
        let playing = &item.cases[0];
        let case = Faded::at_once(&Applied {
            asset: Some("gel_green"),
            case: Some(playing),
            ..Applied::default()
        });

        let lit = over(&item, Some(playing), case, [1., 0.]);
        assert_eq!(lit.pictures.top(), Some("gel_green_lit"));

        // In a case with no look of its own, the item's own shows.
        assert_eq!(
            over(&item, None, rest(), [1., 0.]).pictures.top(),
            Some("gel_lit")
        );

        // Neither the case nor the item has a pressed look, so the hover's
        // image holds.
        assert_eq!(
            over(&item, Some(playing), case, [1., 1.]).pictures.top(),
            Some("gel_green_lit")
        );
        assert!(wears(&item));
    }

    #[test]
    fn a_look_edited_back_to_plain_saves_as_nothing() {
        let mut item = button();
        item.hover = None;
        let hover = Slot {
            which: Which::Hover,
            case: None,
        };

        edit_look(&mut item, hover, |look| look.opacity = 0.7);
        assert_eq!(item.hover.as_ref().map(|look| look.opacity), Some(0.7));

        edit_look(&mut item, hover, |look| look.opacity = 1.);
        assert!(item.hover.is_none());
        assert!(serde_json::to_value(&item).unwrap().get("hover").is_none());

        // A case's look, edited back, takes the case it made with it.
        let in_case = Slot {
            which: Which::Pressed,
            case: Some("playing"),
        };
        edit_look(&mut item, in_case, |look| look.asset = "down".into());
        assert_eq!(item.cases.len(), 1);
        assert_eq!(
            in_case.of(&item).map(|look| look.asset.as_str()),
            Some("down")
        );

        edit_look(&mut item, in_case, |look| look.asset.clear());
        assert!(item.cases.is_empty());
    }

    fn gel_button() -> CanvasState {
        serde_json::from_value(json!({ "items": [{
            "id": "play",
            "action": "toggle_playback",
            "content": { "kind": "group", "items": [
                { "id": "play-body", "hover": { "asset": "gel_lit" },
                  "content": { "kind": "image", "asset": "gel" } },
                { "id": "play-glyph", "hit": false,
                  "content": { "kind": "image", "asset": "glyph" } },
                { "id": "play-menu", "action": "next_track",
                  "hover": { "opacity": 0.5 },
                  "content": { "kind": "image", "asset": "menu" } },
            ] },
        }] }))
        .expect("a canvas parses")
    }

    #[test]
    fn a_groups_hover_reaches_what_its_command_covers() {
        let canvas = gel_button();
        let group = canvas.find("play").unwrap();
        assert!(!wears(group) && worn(group), "the body's look counts");

        assert!(covered(&canvas, "play-body"));
        assert!(covered(&canvas, "play-glyph"));
        assert!(!covered(&canvas, "nowhere"));

        let mut shown = Shown::new();
        shown.insert("play".into(), [1., 0.]);
        let from_group = weights(group, &shown, None);
        assert_eq!(from_group, Some([1., 0.]));

        let body = canvas.find("play-body").unwrap();
        assert_eq!(weights(body, &shown, from_group), Some([1., 0.]));

        // A child with its own command is its own button and lights only
        // when it's the one hovered.
        let menu = canvas.find("play-menu").unwrap();
        assert_eq!(weights(menu, &shown, from_group), None);
        shown.insert("play-menu".into(), [1., 0.]);
        assert_eq!(weights(menu, &shown, None), Some([1., 0.]));
    }

    #[test]
    fn the_inspector_previews_a_look_at_full_weight() {
        let mut shown = Shown::new();
        preview(&mut shown, "play", ":pressed");
        assert_eq!(shown["play"], [1., 1.]);

        preview(&mut shown, "other", "playing");
        assert!(!shown.contains_key("other"), "a case id isn't a look");

        preview(&mut shown, "body", "playing:hover");
        assert_eq!(shown["body"], [1., 0.], "a case's own look previews too");
    }
}
