//! The inspector's rows for an axis that can be a fraction of the parent:
//! the unit beside a position's x or y or a size's side, and the limits
//! under a fractional size. A fraction reads as a percentage, and a fraction
//! plus units as both numbers. The aspect lock under the sizes lives here
//! too.

use std::rc::Rc;

use gpui::{AnyElement, Context, Div, MouseButton, SharedString, div, prelude::*, px};
use rox_canvas::{CanvasItem, CanvasState, Content, Length, LengthKind, Viewport};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;

use super::{begin_typing, number, trimmed, typing_field, typing_input};
use crate::canvas::edit;
use crate::panels::canvas::CanvasPanel;

/// One of the lengths these rows edit: x or y of the position, or a side
/// of the size.
#[derive(Clone, Copy)]
pub(super) enum Axis {
    Position(usize),
    Size(usize),
}

impl Axis {
    fn of(self, item: &mut CanvasItem) -> &mut Length {
        match self {
            Axis::Position(axis) => &mut item.place.position.xy[axis],
            Axis::Size(axis) => &mut item.place.size[axis],
        }
    }

    fn index(self) -> usize {
        match self {
            Axis::Position(axis) | Axis::Size(axis) => axis,
        }
    }

    /// The field name the number is typed under.
    fn field(self, id: &str) -> String {
        match self {
            Axis::Position(axis) => format!("{}:{id}", ["x", "y"][axis]),
            Axis::Size(axis) => format!("size{axis}:{id}"),
        }
    }

    fn unit_key(self) -> &'static str {
        match self {
            Axis::Position(_) => "position.unit",
            Axis::Size(_) => "size.unit",
        }
    }

    /// A size can't take a negative fraction. Its units can go under zero,
    /// for a box that's its parent less a fixed strip.
    fn held(self, stage: f32) -> f32 {
        match self {
            Axis::Position(_) => stage,
            Axis::Size(_) => stage.max(0.),
        }
    }
}

/// A length's number, or both its numbers for a fraction plus units: the
/// fraction as a percentage, then the units after a plus.
pub(super) fn length_value(
    panel: &CanvasPanel,
    id: &str,
    axis: Axis,
    length: Length,
    cx: &mut Context<CanvasPanel>,
) -> Div {
    let field = axis.field(id);
    let set = move |id: String, change: fn(Length, f32, Axis) -> Length| {
        move |canvas: &mut CanvasState, v: f32| {
            if let Some(item) = canvas.find_mut(&id) {
                let length = axis.of(item);
                *length = change(*length, v, axis);
            }
        }
    };

    let row = div().flex().flex_row().items_center().gap(tokens::SPACE_XS);
    match length {
        Length::Units(n) => row.child(number(
            panel,
            &field,
            n,
            false,
            set(id.to_string(), |_, v, axis| match axis {
                Axis::Size(_) => Length::Units(v.max(0.)),
                Axis::Position(_) => Length::Units(v),
            }),
            cx,
        )),

        Length::Stage(f) => row.child(number(
            panel,
            &field,
            f * 100.,
            false,
            set(id.to_string(), |_, v, axis| {
                Length::Stage(axis.held(v / 100.))
            }),
            cx,
        )),

        Length::Mixed { stage, units } => row
            .child(number(
                panel,
                &field,
                stage * 100.,
                false,
                set(id.to_string(), |length, v, axis| match length {
                    Length::Mixed { units, .. } => Length::Mixed {
                        stage: axis.held(v / 100.),
                        units,
                    },
                    other => other,
                }),
                cx,
            ))
            .child(muted("% +"))
            .child(number(
                panel,
                &format!("{field}+units"),
                units,
                false,
                set(id.to_string(), |length, v, _| match length {
                    Length::Mixed { stage, .. } => Length::Mixed { stage, units: v },
                    other => other,
                }),
                cx,
            )),
    }
}

fn muted(text: &'static str) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .whitespace_nowrap()
        .child(text)
}

/// Units, Percent, or Both. Picking another one re-expresses the length at
/// the same place, as one undo step: a fraction or units gaining the other
/// term starts it at zero.
pub(super) fn length_unit(
    id: &str,
    axis: Axis,
    length: Length,
    viewport: Viewport,
    cx: &mut Context<CanvasPanel>,
) -> Div {
    let kinds = [
        (rox_i18n::t!("canvas-length-units"), LengthKind::Units),
        (rox_i18n::t!("canvas-length-percent"), LengthKind::Stage),
        (rox_i18n::t!("canvas-length-both"), LengthKind::Mixed),
    ];
    let current = length.kind();
    let id = id.to_string();

    panel::choices_shared(
        &kinds,
        current,
        move |this: &mut CanvasPanel, want: LengthKind, cx| {
            if want == current {
                return;
            }

            let id = id.clone();
            this.change_later(
                axis.unit_key(),
                move |canvas| {
                    let Some(parent) = edit::parent_frame(canvas, &id, viewport) else {
                        return;
                    };
                    if let Some(item) = canvas.find_mut(&id) {
                        let length = axis.of(item);
                        *length = length.with_kind(want, parent.size[axis.index()]);
                    }
                },
                cx,
            )
        },
        cx,
    )
}

/// A position's x or y: its unit and its number, for the bindable row the
/// axis sits in.
pub(super) fn position_control(
    panel: &CanvasPanel,
    id: &str,
    axis: usize,
    length: Length,
    viewport: Viewport,
    cx: &mut Context<CanvasPanel>,
) -> Div {
    let axis = Axis::Position(axis);

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_end()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(length_unit(id, axis, length, viewport, cx))
        .child(length_value(panel, id, axis, length, cx))
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

/// Under the width and height: the switch that keeps the box one shape, the
/// width over height it keeps while on, and for a decoded image a press that
/// takes the picture's own. Switching it on keeps the shape the box has in
/// this window, so nothing moves until the window does.
pub(super) fn aspect_row(
    panel: &CanvasPanel,
    item: &CanvasItem,
    viewport: Viewport,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let id = item.id.clone();
    let locked = item.place.aspect();

    let lock_id = id.clone();
    let lock = panel::toggle(
        locked.is_some(),
        move |this: &mut CanvasPanel, on, cx| {
            let id = lock_id.clone();
            this.change_later(
                "place.aspect",
                move |canvas| {
                    let Some(parent) = edit::parent_frame(canvas, &id, viewport) else {
                        return;
                    };
                    if let Some(item) = canvas.find_mut(&id) {
                        item.place.aspect = on.then(|| shape_of(item.place.space_in(parent.size)));
                    }
                },
                cx,
            )
        },
        cx,
    );

    let mut control = div().flex().flex_row().items_center().gap(tokens::SPACE_XS);

    if let Some(aspect) = locked {
        let setter = id.clone();
        control = control.child(number(
            panel,
            &format!("aspect:{id}"),
            aspect,
            false,
            move |canvas, v| {
                if let Some(item) = canvas.find_mut(&setter)
                    && v.is_finite()
                    && v > 0.
                {
                    item.place.aspect = Some(v);
                }
            },
            cx,
        ));
    }

    if let Some(picture) = picture_shape(panel, item) {
        let setter = id.clone();
        control = control.child(rox_panel_kit::ui::small_button(
            rox_i18n::t!("canvas-place-aspect-picture"),
            icons::IMAGE,
            false,
            cx.listener(move |this: &mut CanvasPanel, _, _, cx| {
                let id = setter.clone();
                this.change_later(
                    "place.aspect",
                    move |canvas| {
                        if let Some(item) = canvas.find_mut(&id) {
                            item.place.aspect = Some(picture);
                        }
                    },
                    cx,
                )
            }),
        ));
    }

    div()
        .id(SharedString::from(format!("canvas-aspect:{id}")))
        .child(panel::setting_row_wrap(
            rox_i18n::t!("canvas-place-aspect"),
            Some(rox_i18n::t!("canvas-place-aspect.description")),
            control.child(lock),
        ))
        .into_any_element()
}

/// Width over height, a square for a box with no area.
fn shape_of([width, height]: [f32; 2]) -> f32 {
    if width > 0. && height > 0. {
        width / height
    } else {
        1.
    }
}

/// An image item's picture as width over height, once it has decoded.
fn picture_shape(panel: &CanvasPanel, item: &CanvasItem) -> Option<f32> {
    let Content::Image { asset, .. } = &item.content else {
        return None;
    };

    let pixels = panel.images().ready(asset)?.size(0);
    let (width, height) = (pixels.width.0 as f32, pixels.height.0 as f32);
    (width > 0. && height > 0.).then(|| width / height)
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
