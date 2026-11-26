//! The inspector's rows for an axis that can be a fraction of the parent:
//! the unit beside a position's x or y, and the limits under a fractional
//! size. A fraction reads as a percentage, as a size's does.

use std::rc::Rc;

use gpui::{AnyElement, Context, Div, MouseButton, SharedString, div, prelude::*, px};
use rox_canvas::{CanvasState, Length, Viewport};
use rox_design::{palette, tokens};
use rox_panel_api::panel;

use super::{begin_typing, number, trimmed, typing_field, typing_input};
use crate::canvas::edit;
use crate::panels::canvas::CanvasPanel;

/// The value a length shows: units as they are, a fraction as a percentage.
fn shown(length: Length) -> f32 {
    match length {
        Length::Units(n) => n,
        Length::Stage(f) => f * 100.,
    }
}

/// A position's x or y: its unit and its number, side by side, for the
/// bindable row the axis sits in.
pub(super) fn position_control(
    panel: &CanvasPanel,
    id: &str,
    axis: usize,
    length: Length,
    viewport: Viewport,
    cx: &mut Context<CanvasPanel>,
) -> Div {
    let property = ["x", "y"][axis];
    let setter = id.to_string();
    let value = number(
        panel,
        &format!("{property}:{id}"),
        shown(length),
        false,
        move |canvas, v| {
            if let Some(item) = canvas.find_mut(&setter) {
                item.place.position.xy[axis] = match item.place.position.xy[axis] {
                    Length::Stage(_) => Length::Stage(v / 100.),
                    Length::Units(_) => Length::Units(v),
                };
            }
        },
        cx,
    );

    let unit_id = id.to_string();
    let unit = unit_toggle(length.is_fraction(), "position.unit", cx, move |canvas| {
        let Some(parent) = edit::parent_frame(canvas, &unit_id, viewport) else {
            return;
        };
        if let Some(item) = canvas.find_mut(&unit_id) {
            item.place.position.xy[axis] =
                edit::other_unit(item.place.position.xy[axis], parent.size[axis]);
        }
    });

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(unit)
        .child(value)
}

/// Units or Percent. Picking the other one re-expresses the axis at the same
/// place, as one undo step under `key`.
fn unit_toggle(
    fraction: bool,
    key: &'static str,
    cx: &mut Context<CanvasPanel>,
    switch: impl Fn(&mut CanvasState) + Clone + 'static,
) -> Div {
    let units = [
        (rox_i18n::t!("canvas-length-units"), false),
        (rox_i18n::t!("canvas-length-percent"), true),
    ];

    panel::choices_shared(
        &units,
        fraction,
        move |this: &mut CanvasPanel, want: bool, cx| {
            if want != fraction {
                let switch = switch.clone();
                this.change_later(key, move |canvas| switch(canvas), cx);
            }
        },
        cx,
    )
}

/// Under a fractional width or height: the least and the most it resolves
/// to, in units. Empty is no limit.
pub(super) fn limit_rows(
    panel: &CanvasPanel,
    id: &str,
    axis: usize,
    limits: [Option<f32>; 2],
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let field = |most: bool, cx: &mut Context<CanvasPanel>| {
        let label = match most {
            false => rox_i18n::t!("canvas-length-min"),
            true => rox_i18n::t!("canvas-length-max"),
        };
        let control = limit_field(panel, id, axis, most, limits[usize::from(most)], cx);
        rox_panel_kit::ui::nested(panel::setting_row(label, None, control))
    };

    let fields = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(field(false, cx))
        .child(field(true, cx));

    // Width and height share the label, so the id keeps their controls apart.
    div()
        .id(SharedString::from(format!("canvas-limits{axis}:{id}")))
        .child(panel::setting_block(
            rox_i18n::t!("canvas-length-limits"),
            Some(rox_i18n::t!("canvas-length-limits.description")),
            None,
            fields,
        ))
        .into_any_element()
}

/// One limit as a number that turns into a field on a press. An empty field
/// clears it, and a negative number reads as zero.
fn limit_field(
    panel: &CanvasPanel,
    id: &str,
    axis: usize,
    most: bool,
    limit: Option<f32>,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let field = format!("limit{axis}{}:{id}", if most { "max" } else { "min" });
    if let Some(input) = typing_input(panel, &field) {
        return div()
            .w(px(64.))
            .child(typing_field(&input))
            .into_any_element();
    }

    let (text, typed) = match limit {
        Some(units) => (SharedString::from(trimmed(units)), trimmed(units)),
        None => (rox_i18n::t!("canvas-length-no-limit"), String::new()),
    };
    let setter = id.to_string();
    let commit = Rc::new(move |canvas: &mut CanvasState, typed: &str| {
        let value = match typed.trim() {
            "" => None,
            typed => match rox_panel_kit::parse_number(typed) {
                Some(v) if v.is_finite() => Some(v.max(0.)),
                _ => return,
            },
        };
        if let Some(item) = canvas.find_mut(&setter) {
            match most {
                false => item.place.min_size[axis] = value,
                true => item.place.max_size[axis] = value,
            }
        }
    });

    div()
        .id(SharedString::from(format!("canvas-number-{field}")))
        .min_w(px(48.))
        .px(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .text_right()
        .whitespace_nowrap()
        .cursor_text()
        .hover(|d| d.bg(palette::bg_control_hover()))
        .when(limit.is_none(), |d| d.text_color(palette::text_muted()))
        .child(text)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                begin_typing(
                    this,
                    field.clone(),
                    typed.clone(),
                    commit.clone(),
                    window,
                    cx,
                );
            }),
        )
        .into_any_element()
}
