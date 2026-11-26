//! Hover and pressed looks on click items (ADR 33). An item with a click
//! command can carry a look for while the pointer is over it and one for
//! while it's pressed, laid over whichever case its state is in, the pressed
//! look on top of the hover one. Off design mode only, and only on the item
//! a click there would run: the front item under the pointer by the same
//! walk, or the group whose command covers it. Nothing but the pointer
//! hovers, so the keyboard never shows a look.
//!
//! Each look eases in and out over [`EASE`]. Opacity blends. An image or a
//! color can't, so it switches halfway through the ease. A look asks for
//! frames only while an ease runs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, ElementId, SharedString, Window, div, prelude::*};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_canvas::{CanvasItem, Content, Look};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, setting_row};
use rox_panel_kit::Tip;
use rox_panel_kit::ui as settings_ui;

use crate::canvas::overlay;
use crate::canvas::states::Applied;
use crate::panels::canvas::{self, CanvasPanel};

/// How long a look takes to come in or go. Short enough that a quick click
/// still shows its press.
pub(crate) const EASE: Duration = Duration::from_millis(140);

/// The preview the inspector names in place of a case, for each look. No
/// state's case id starts with a colon, so the case preview never takes one.
pub(crate) const PREVIEW_HOVER: &str = ":hover";
pub(crate) const PREVIEW_PRESSED: &str = ":pressed";

/// How far each look shows on an item this frame, hover then pressed, 0 to
/// 1. An item missing from it shows neither.
pub(crate) type Shown = HashMap<String, [f32; 2]>;

/// Whether the item carries either look.
pub(crate) fn wears(item: &CanvasItem) -> bool {
    item.hover.is_some() || item.pressed.is_some()
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

/// The inspector's preview of a look on the selected item, at full weight.
pub(crate) fn preview(shown: &mut Shown, id: &str, case: &str) {
    let weights = match case {
        PREVIEW_HOVER => [1., 0.],
        PREVIEW_PRESSED => [1., 1.],
        _ => return,
    };
    shown.insert(id.to_string(), weights);
}

/// `applied` with the item's looks laid over at `weights`. A look's
/// opacity multiplies, so the pressed look's goes on top of the hover
/// look's; its image or color takes over once it's halfway in.
pub(crate) fn over<'a>(
    item: &'a CanvasItem,
    mut applied: Applied<'a>,
    [hover, press]: [f32; 2],
) -> Applied<'a> {
    let named = |text: &'a str| Some(text).filter(|text| !text.is_empty());

    for (look, weight) in [(&item.hover, hover), (&item.pressed, press)] {
        let Some(look) = look.as_ref().filter(|_| weight > 0.) else {
            continue;
        };

        // A hand-edited file can hold any number here.
        let opacity = if look.opacity.is_finite() {
            look.opacity.clamp(0., 1.)
        } else {
            1.
        };
        applied.opacity *= 1. + (opacity - 1.) * weight;

        if weight >= 0.5 {
            applied.asset = named(&look.asset).or(applied.asset);
            applied.color = named(&look.color).or(applied.color);
        }
    }

    applied
}

/// Which look an inspector row edits.
#[derive(Clone, Copy, PartialEq)]
enum Which {
    Hover,
    Pressed,
}

impl Which {
    fn of(self, item: &CanvasItem) -> Option<&Look> {
        match self {
            Which::Hover => item.hover.as_ref(),
            Which::Pressed => item.pressed.as_ref(),
        }
    }

    fn slot(self, item: &mut CanvasItem) -> &mut Option<Look> {
        match self {
            Which::Hover => &mut item.hover,
            Which::Pressed => &mut item.pressed,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Which::Hover => "hover",
            Which::Pressed => "pressed",
        }
    }

    fn preview(self) -> &'static str {
        match self {
            Which::Hover => PREVIEW_HOVER,
            Which::Pressed => PREVIEW_PRESSED,
        }
    }
}

/// Edits one look, making it first if there's none. A look edited back to
/// changing nothing goes, so an untouched one saves as nothing at all.
fn edit_look(item: &mut CanvasItem, which: Which, edit: impl FnOnce(&mut Look)) {
    let slot = which.slot(item);
    let look = slot.get_or_insert_with(Look::default);
    edit(look);

    if look.is_plain() {
        *slot = None;
    }
}

/// Through `change`, so every look edit is an undo step and a save. A run
/// of edits to one field merges into one step.
fn change_look(
    this: &mut CanvasPanel,
    id: &str,
    which: Which,
    field: &'static str,
    edit: impl FnOnce(&mut Look) + 'static,
    cx: &mut Context<CanvasPanel>,
) {
    let id = id.to_string();
    let key = match (which, field) {
        (Which::Hover, "opacity") => "look.hover.opacity",
        (Which::Hover, "color") => "look.hover.color",
        (Which::Pressed, "opacity") => "look.pressed.opacity",
        _ => "look.pressed.color",
    };
    this.change_later(
        key,
        move |canvas| {
            if let Some(item) = canvas.find_mut(&id) {
                edit_look(item, which, edit);
            }
        },
        cx,
    );
}

/// The two looks' blocks, for an item a click runs a command on. None for
/// one without, whose looks would never show.
pub(crate) fn rows(
    panel: &CanvasPanel,
    item: &CanvasItem,
    cx: &mut Context<CanvasPanel>,
) -> Option<AnyElement> {
    if !canvas::commands(item) {
        return None;
    }

    Some(
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(block(panel, item, Which::Hover, cx))
            .child(block(panel, item, Which::Pressed, cx))
            .into_any_element(),
    )
}

/// One look: its opacity, and the image or color its kind takes, the same
/// rows a case has. A look left alone reads as the case under it.
fn block(
    panel: &CanvasPanel,
    item: &CanvasItem,
    which: Which,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let look = which.of(item).cloned().unwrap_or_default();
    let previewing = panel.inspector.preview() == Some((item.id.as_str(), which.preview()));

    let me = cx.entity().downgrade();
    let preview_id = item.id.clone();
    let mut eye = settings_ui::icon_button(icons::EYE, false, move |_, _, cx| {
        let id = preview_id.clone();
        me.update(cx, |this, cx| {
            this.inspector.toggle_preview(id, which.preview());
            cx.notify();
        })
        .ok();
    })
    .keyed(ElementId::Name(
        format!("canvas-look-preview:{}", which.key()).into(),
    ));
    if previewing {
        eye = eye.bg(palette::alpha(palette::accent(), 0x40));
    }
    let eye = Tip::keyed(
        ElementId::Name(format!("canvas-look-preview-tip:{}", which.key()).into()),
        rox_i18n::t!("canvas-look-preview"),
    )
    .apply(div().child(eye));

    let label = match which {
        Which::Hover => rox_i18n::t!("canvas-look-hover"),
        Which::Pressed => rox_i18n::t!("canvas-look-pressed"),
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
        .child(opacity_row(panel, item, which, look.opacity, cx));

    match &item.content {
        Content::Image { .. } => body = body.child(image_row(&item.id, which, &look, cx)),
        Content::Text { .. } => body = body.child(color_row(&item.id, which, &look, cx)),
        _ => {}
    }

    div()
        .id(SharedString::from(format!("canvas-look-{}", which.key())))
        .child(settings_ui::nested(body))
        .into_any_element()
}

fn opacity_row(
    panel: &CanvasPanel,
    item: &CanvasItem,
    which: Which,
    opacity: f32,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let scrub = panel.inspector.scrub(&format!("look:{}", which.key()));

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
                which,
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
fn image_row(id: &str, which: Which, look: &Look, cx: &mut Context<CanvasPanel>) -> AnyElement {
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
                                            edit_look(item, which, |look| look.asset = asset);
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
        format!("canvas-look-image:{}", which.key()).into(),
    ));

    setting_row(rox_i18n::t!("canvas-image-asset"), None, pick).into_any_element()
}

/// The color picker, headed by the color the case gives.
fn color_row(id: &str, which: Which, look: &Look, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let mut roles = vec![(String::new(), rox_i18n::t!("canvas-case-same-color"))];
    roles.extend(overlay::text_roles(&look.color));

    let id = id.to_string();
    let picker = panel::picker(
        match which {
            Which::Hover => "canvas-look-color-hover",
            Which::Pressed => "canvas-look-color-pressed",
        },
        look.color.clone(),
        roles,
        false,
        move |this: &mut CanvasPanel, role: String, cx| {
            change_look(this, &id, which, "color", move |look| look.color = role, cx)
        },
        cx,
    );

    setting_row(rox_i18n::t!("canvas-text-color"), None, picker).into_any_element()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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

    #[test]
    fn the_pressed_look_goes_on_top_of_the_hover_look() {
        let item = button();
        let rest = Applied::default();

        assert_eq!(over(&item, rest, [0., 0.]), rest);

        let hovered = over(&item, rest, [1., 0.]);
        assert_eq!(hovered.opacity, 0.5);
        assert_eq!(hovered.asset, Some("lit"));

        let pressed = over(&item, rest, [1., 1.]);
        assert_eq!(pressed.opacity, 0.25);
        assert_eq!(pressed.asset, Some("down"));

        // Under halfway the image hasn't switched yet, and the opacity is
        // on its way.
        let early = over(&item, rest, [0.25, 0.]);
        assert_eq!(early.asset, None);
        assert!((early.opacity - 0.875).abs() < 1e-6);
    }

    #[test]
    fn a_look_left_alone_keeps_what_the_case_gives() {
        let mut item = button();
        item.pressed = Some(Look {
            opacity: 0.8,
            ..Look::default()
        });
        let case = Applied {
            opacity: 0.5,
            asset: Some("pause"),
            ..Applied::default()
        };

        let pressed = over(&item, case, [1., 1.]);
        assert_eq!(pressed.asset, Some("lit"), "the hover's image stays");
        assert!((pressed.opacity - 0.5 * 0.5 * 0.8).abs() < 1e-6);

        item.hover = None;
        assert_eq!(over(&item, case, [1., 1.]).asset, Some("pause"));

        // A file can say anything; what draws stays in range.
        item.pressed = Some(Look {
            opacity: f32::NAN,
            ..Look::default()
        });
        assert_eq!(over(&item, case, [1., 1.]).opacity, 0.5);
    }

    #[test]
    fn a_look_edited_back_to_plain_saves_as_nothing() {
        let mut item = button();
        item.hover = None;

        edit_look(&mut item, Which::Hover, |look| look.opacity = 0.7);
        assert_eq!(item.hover.as_ref().map(|look| look.opacity), Some(0.7));

        edit_look(&mut item, Which::Hover, |look| look.opacity = 1.);
        assert!(item.hover.is_none());
        assert!(serde_json::to_value(&item).unwrap().get("hover").is_none());
    }

    #[test]
    fn the_inspector_previews_a_look_at_full_weight() {
        let mut shown = Shown::new();
        preview(&mut shown, "play", PREVIEW_PRESSED);
        assert_eq!(shown["play"], [1., 1.]);

        preview(&mut shown, "other", "playing");
        assert!(!shown.contains_key("other"), "a case id isn't a look");
    }
}
