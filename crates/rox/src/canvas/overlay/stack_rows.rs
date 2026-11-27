//! The inspector's rows for a stacking group: the switch, then the axis,
//! the gap, the padding, where the children sit across and along it, and
//! whether it runs from the far end. A run longer than the group says by how
//! much under the rows, since nothing shrinks to fit. And on a stacked
//! child: the note that its x and y are offsets, then its growth, its
//! margins and where it sits across.

use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div};
use rox_canvas::{CanvasItem, Stack, StackAlign, StackAxis, StackJustify, Viewport, stack};
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, setting_row};

use super::number;
use crate::canvas::edit;
use crate::panels::canvas::CanvasPanel;

/// A stack's run longer than its group by under this many units reads as
/// fitting, so a rounding error says nothing.
const OVERFLOW_SLACK: f32 = 0.5;

pub(super) fn rows(
    panel: &CanvasPanel,
    group: &CanvasItem,
    viewport: Viewport,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let id = group.id.clone();
    let stack = group.stack;

    let switch_id = id.clone();
    let switch = panel::toggle(
        stack.is_some(),
        move |this: &mut CanvasPanel, on, cx| {
            let id = switch_id.clone();
            this.change_later(
                "stack",
                move |canvas| {
                    edit::set_stack(canvas, &id, on.then(Stack::default), viewport);
                },
                cx,
            )
        },
        cx,
    );

    let mut rows = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("canvas-stack"),
            Some(rox_i18n::t!("canvas-stack.description")),
            switch,
        ));

    let Some(stack) = stack else {
        return rows.into_any_element();
    };

    let axes = [
        (rox_i18n::t!("canvas-stack-axis-y"), StackAxis::Y),
        (rox_i18n::t!("canvas-stack-axis-x"), StackAxis::X),
    ];
    rows = rows.child(nested(setting_row(
        rox_i18n::t!("canvas-stack-axis"),
        None,
        panel::choices_shared(
            &axes,
            stack.axis,
            edit_stack(&id, viewport, "stack.axis", |stack, axis| stack.axis = axis),
            cx,
        ),
    )));

    let gap_id = id.clone();
    rows = rows.child(nested(setting_row(
        rox_i18n::t!("canvas-stack-gap"),
        None,
        number(
            panel,
            &format!("stack-gap:{id}"),
            stack.gap,
            false,
            move |canvas, v| {
                if let Some(stack) = canvas.find_mut(&gap_id).and_then(|g| g.stack.as_mut()) {
                    stack.gap = v;
                }
            },
            cx,
        ),
    )));

    rows = rows.child(nested(panel::setting_row_wrap(
        rox_i18n::t!("canvas-stack-padding"),
        Some(rox_i18n::t!("canvas-stack-padding.description")),
        padding_fields(panel, &id, stack.padding, cx),
    )));

    let ends = [
        (rox_i18n::t!("canvas-stack-start"), StackAlign::Start),
        (rox_i18n::t!("canvas-stack-center"), StackAlign::Center),
        (rox_i18n::t!("canvas-stack-end"), StackAlign::End),
    ];
    let runs = [
        (rox_i18n::t!("canvas-stack-start"), StackJustify::Start),
        (rox_i18n::t!("canvas-stack-center"), StackJustify::Center),
        (rox_i18n::t!("canvas-stack-end"), StackJustify::End),
        (rox_i18n::t!("canvas-stack-between"), StackJustify::Between),
    ];
    rows = rows
        .child(nested(panel::setting_row_wrap(
            rox_i18n::t!("canvas-stack-justify"),
            Some(rox_i18n::t!("canvas-stack-justify.description")),
            panel::choices_shared(
                &runs,
                stack.justify,
                edit_stack(&id, viewport, "stack.justify", |stack, justify| {
                    stack.justify = justify
                }),
                cx,
            ),
        )))
        .child(nested(setting_row(
            rox_i18n::t!("canvas-stack-align"),
            Some(rox_i18n::t!("canvas-stack-align.description")),
            panel::choices_shared(
                &ends,
                stack.align,
                edit_stack(&id, viewport, "stack.align", |stack, align| {
                    stack.align = align
                }),
                cx,
            ),
        )));

    let reverse = edit_stack(&id, viewport, "stack.reverse", |stack, on| {
        stack.reverse = on
    });
    rows = rows.child(nested(setting_row(
        rox_i18n::t!("canvas-stack-reverse"),
        Some(rox_i18n::t!("canvas-stack-reverse.description")),
        panel::toggle(stack.reverse, reverse, cx),
    )));

    if let Some(over) = overflow(panel, group, &stack, viewport) {
        let units = (over.round() as i64).to_string();
        rows = rows.child(note(rox_i18n::t!("canvas-stack-overflow", units = units)));
    }

    rows.into_any_element()
}

/// Under a stacked child's x and y: what they mean there.
pub(super) fn offset_note() -> AnyElement {
    note(rox_i18n::t!("canvas-stack-offset-note"))
}

/// A stacked child's own rows: its weight in the room left over, its
/// margins before and after along the axis, and where it sits across.
pub(super) fn child_rows(
    panel: &CanvasPanel,
    item: &CanvasItem,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let id = item.id.clone();
    let place = item.place;

    let grow_id = id.clone();
    let grow = number(
        panel,
        &format!("grow:{id}"),
        place.grow,
        false,
        move |canvas, v| {
            if let Some(item) = canvas.find_mut(&grow_id) {
                item.place.grow = v.max(0.);
            }
        },
        cx,
    );

    let mut margin = div().flex().flex_row().items_center().gap(tokens::SPACE_XS);
    for side in 0..2 {
        let setter = id.clone();
        margin = margin.child(number(
            panel,
            &format!("margin{side}:{id}"),
            place.margin[side],
            false,
            move |canvas, v| {
                if let Some(item) = canvas.find_mut(&setter) {
                    item.place.margin[side] = v;
                }
            },
            cx,
        ));
    }

    let sides = [
        (rox_i18n::t!("canvas-stack-align-self-stack"), None),
        (rox_i18n::t!("canvas-stack-start"), Some(StackAlign::Start)),
        (
            rox_i18n::t!("canvas-stack-center"),
            Some(StackAlign::Center),
        ),
        (rox_i18n::t!("canvas-stack-end"), Some(StackAlign::End)),
    ];
    let align_id = id.clone();
    let align = panel::choices_shared(
        &sides,
        place.align_self,
        move |this: &mut CanvasPanel, side: Option<StackAlign>, cx| {
            let id = align_id.clone();
            this.change_later(
                "place.align_self",
                move |canvas| {
                    if let Some(item) = canvas.find_mut(&id) {
                        item.place.align_self = side;
                    }
                },
                cx,
            )
        },
        cx,
    );

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("canvas-stack-grow"),
            Some(rox_i18n::t!("canvas-stack-grow.description")),
            grow,
        ))
        .child(setting_row(
            rox_i18n::t!("canvas-stack-margin"),
            Some(rox_i18n::t!("canvas-stack-margin.description")),
            margin,
        ))
        .child(panel::setting_row_wrap(
            rox_i18n::t!("canvas-stack-align-self"),
            Some(rox_i18n::t!("canvas-stack-align-self.description")),
            align,
        ))
        .into_any_element()
}

/// The four sides of a stack's padding: left, top, right, bottom.
fn padding_fields(
    panel: &CanvasPanel,
    id: &str,
    padding: [f32; 4],
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_end()
        .items_center()
        .gap(tokens::SPACE_XS);

    for side in 0..4 {
        let setter = id.to_string();
        row = row.child(number(
            panel,
            &format!("stack-pad{side}:{id}"),
            padding[side],
            false,
            move |canvas, v| {
                if let Some(stack) = canvas.find_mut(&setter).and_then(|g| g.stack.as_mut()) {
                    stack.padding[side] = v.max(0.);
                }
            },
            cx,
        ));
    }

    row.into_any_element()
}

/// How many units the run of `group`'s children passes the box inside its
/// padding along the axis, as the editor lays them out, if it does.
fn overflow(
    panel: &CanvasPanel,
    group: &CanvasItem,
    stack: &Stack,
    viewport: Viewport,
) -> Option<f32> {
    let canvas = &panel.config.canvas;
    let frame = edit::group_frame(canvas, &group.id, viewport)?;
    let rox_canvas::Content::Group { items } = &group.content else {
        return None;
    };

    let entries = stack::entries(stack, items, frame.size, stack::shown);
    let (_, run) = stack::cells(stack, frame.size, &entries);
    let over = run - stack.inner(frame.size)[stack.axis.along()];
    (over > OVERFLOW_SLACK).then_some(over)
}

/// One field of the stack changed, as one undo step.
fn edit_stack<V: Copy + 'static>(
    id: &str,
    viewport: Viewport,
    key: &'static str,
    set: fn(&mut Stack, V),
) -> impl Fn(&mut CanvasPanel, V, &mut Context<CanvasPanel>) + Clone + 'static {
    let id = id.to_string();
    move |this: &mut CanvasPanel, value: V, cx| {
        let id = id.clone();
        this.change_later(
            key,
            move |canvas| {
                let Some(mut stack) = canvas.find(&id).and_then(|group| group.stack) else {
                    return;
                };
                set(&mut stack, value);
                edit::set_stack(canvas, &id, Some(stack), viewport);
            },
            cx,
        )
    }
}

fn nested(row: gpui::Div) -> AnyElement {
    rox_panel_kit::ui::nested(row).into_any_element()
}

fn note(text: SharedString) -> AnyElement {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
        .into_any_element()
}
