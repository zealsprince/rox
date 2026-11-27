//! The inspector's rows for a stacking group: the switch, then the axis,
//! the gap, and where the children sit across and along it. A run longer
//! than the group says by how much under the rows, since nothing shrinks to
//! fit. And the note on a stacked child that its x and y are offsets.

use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div};
use rox_canvas::{CanvasItem, Stack, StackAlign, StackAxis, Viewport, stack};
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
            edit_stack(&id, viewport, |stack, axis| stack.axis = axis),
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

    let ends = || {
        [
            (rox_i18n::t!("canvas-stack-start"), StackAlign::Start),
            (rox_i18n::t!("canvas-stack-center"), StackAlign::Center),
            (rox_i18n::t!("canvas-stack-end"), StackAlign::End),
        ]
    };
    rows = rows
        .child(nested(setting_row(
            rox_i18n::t!("canvas-stack-justify"),
            Some(rox_i18n::t!("canvas-stack-justify.description")),
            panel::choices_shared(
                &ends(),
                stack.justify,
                edit_stack(&id, viewport, |stack, justify| stack.justify = justify),
                cx,
            ),
        )))
        .child(nested(setting_row(
            rox_i18n::t!("canvas-stack-align"),
            Some(rox_i18n::t!("canvas-stack-align.description")),
            panel::choices_shared(
                &ends(),
                stack.align,
                edit_stack(&id, viewport, |stack, align| stack.align = align),
                cx,
            ),
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

/// How many units the run of `group`'s children passes its box along the
/// axis, as the editor lays them out, if it does.
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

    let entries = stack::entries(items, frame.size, stack::shown);
    let (_, run) = stack::cells(stack, frame.size, &entries);
    let over = run - frame.size[stack.axis.along()];
    (over > OVERFLOW_SLACK).then_some(over)
}

/// One field of the stack changed, as one undo step.
fn edit_stack<V: Copy + 'static>(
    id: &str,
    viewport: Viewport,
    set: fn(&mut Stack, V),
) -> impl Fn(&mut CanvasPanel, V, &mut Context<CanvasPanel>) + Clone + 'static {
    let id = id.to_string();
    move |this: &mut CanvasPanel, value: V, cx| {
        let id = id.clone();
        this.change_later(
            "stack",
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
