//! The inspector with several items selected: how many, the aligns and the
//! distributes among them, the order tools, the few rows every item has in
//! the same sense (anchor, opacity, visibility), then group and delete. A
//! shared row whose values differ says so, and an edit on it goes to every
//! item as one undo step. Position and size rows stay hidden: each item
//! keeps its own units or fractions, so one number can't stand for all.

use std::rc::Rc;

use gpui::{AnyElement, Context, Div, IntoElement, ParentElement, Styled, div};
use rox_canvas::{Anchor, CanvasItem, Content};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, setting_block, setting_row};
use rox_panel_kit::ui as settings_ui;

use super::{ToolPress, anchor_picker, line_face, order_tools, tool, tool_row};
use crate::canvas::align::{Arrange, Line};
use crate::panels::canvas::CanvasPanel;

pub(super) fn rows(panel: &CanvasPanel, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let items: Vec<CanvasItem> = panel
        .selected
        .ids()
        .iter()
        .filter_map(|id| panel.config.canvas.find(id))
        .cloned()
        .collect();

    let heading = div()
        .text_color(palette::text())
        .child(rox_i18n::t!("canvas-set-count", count = items.len()));

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(heading)
        .child(arrange(items.len(), cx))
        .child(setting_block(
            rox_i18n::t!("canvas-order"),
            None,
            None,
            tool_row(order_tools(cx)),
        ))
        .child(shared_rows(panel, &items, cx))
        .child(actions(&items, cx))
        .into_any_element()
}

/// The aligns to the set's bounds, and with three or more, the
/// distributes. Locked items stay where they are.
fn arrange(count: usize, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let tip = |name: &str| {
        rox_i18n::t!(
            "canvas-align-tip-selection",
            tool = rox_i18n::t!(name).to_string()
        )
    };

    let mut tools: Vec<gpui::Stateful<Div>> = Line::ALL
        .into_iter()
        .map(|line| {
            let (icon, name, key) = line_face(line);
            let press: ToolPress = Rc::new(move |this, window, cx| {
                this.arrange_selected(Arrange::Align(line), window, cx)
            });
            tool(
                icon,
                tip(name),
                format!("canvas-align-{key}"),
                false,
                press,
                cx,
            )
        })
        .collect();

    if count >= 3 {
        let spread = [
            (icons::MOVE_HORIZONTAL, "canvas-distribute-x", "x", 0),
            (icons::MOVE_VERTICAL, "canvas-distribute-y", "y", 1),
        ];
        for (icon, name, key, axis) in spread {
            let press: ToolPress =
                Rc::new(move |this, window, cx| this.distribute_set(axis, window, cx));
            tools.push(tool(
                icon,
                rox_i18n::t!(name),
                format!("canvas-distribute-{key}"),
                false,
                press,
                cx,
            ));
        }
    }

    setting_block(
        rox_i18n::t!("canvas-align-to-selection"),
        None,
        None,
        tool_row(tools),
    )
    .into_any_element()
}

/// A muted word beside a control whose items disagree.
fn mixed(differ: bool) -> Option<AnyElement> {
    differ.then(|| {
        div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("canvas-set-mixed"))
            .into_any_element()
    })
}

fn with_mixed(control: impl IntoElement, differ: bool) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .children(mixed(differ))
        .child(control)
}

fn shared_rows(
    panel: &CanvasPanel,
    items: &[CanvasItem],
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let ids: Vec<String> = items.iter().map(|item| item.id.clone()).collect();
    let first = items.first();
    let all = |same: &dyn Fn(&CanvasItem, &CanvasItem) -> bool| {
        first.is_none_or(|first| items.iter().all(|item| same(first, item)))
    };

    let anchor: Option<Anchor> = all(&|a, b| a.place.anchor == b.place.anchor)
        .then(|| first.map(|item| item.place.anchor))
        .flatten();
    let anchors = anchor_picker(ids, anchor, cx);

    // A mixed opacity sits at the average, and a drag sets them all.
    let same_opacity = all(&|a, b| (a.opacity - b.opacity).abs() < 0.005);
    let mean = items.iter().map(|item| item.opacity).sum::<f32>() / items.len().max(1) as f32;
    let readout = match same_opacity {
        true => rox_i18n::format::format_percent((mean * 100.).round() as f64),
        false => rox_i18n::t!("canvas-set-mixed").to_string(),
    };
    let slider = panel::value_slider_edit_sized(
        &panel.inspector.scrubs.opacity,
        &panel.inspector.edit,
        mean,
        readout,
        format!("{:.0}", mean * 100.),
        1.,
        panel::SliderWidth::Fill,
        panel::SLIDER_STEP,
        |v| v / 100.,
        |this: &mut CanvasPanel, f, cx| {
            this.edit_set_later("opacity", move |item| item.opacity = f, cx)
        },
        cx,
    );

    // The slider takes what the label leaves, so a long label and the mixed
    // word both fit at the inspector's width.
    let opacity = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_MD)
        .child(div().flex_none().child(rox_i18n::t!("canvas-item-opacity")))
        .child(div().flex_1().min_w_0().child(slider));

    let shown = items.iter().all(|item| item.visible);
    let visible = panel::toggle(
        shown,
        |this: &mut CanvasPanel, on, cx| {
            this.edit_set_later("visible", move |item| item.visible = on, cx)
        },
        cx,
    );

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("canvas-item-anchor"),
            None,
            with_mixed(anchors, anchor.is_none()),
        ))
        .child(opacity)
        .child(setting_row(
            rox_i18n::t!("canvas-set-visible"),
            None,
            with_mixed(visible, !all(&|a, b| a.visible == b.visible)),
        ))
        .into_any_element()
}

/// Group, Ungroup while a group is among them, and Delete.
fn actions(items: &[CanvasItem], cx: &mut Context<CanvasPanel>) -> AnyElement {
    let me = cx.entity().downgrade();
    let button =
        |label: gpui::SharedString,
         icon: &'static str,
         key: &'static str,
         f: fn(&mut CanvasPanel, &mut gpui::Window, &mut Context<CanvasPanel>)| {
            let me = me.clone();
            settings_ui::small_button(label, icon, false, move |_, window, cx| {
                me.update(cx, |this, cx| f(this, window, cx)).ok();
            })
            .keyed(key)
        };

    let holds_group = items
        .iter()
        .any(|item| matches!(item.content, Content::Group { .. }));

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(tokens::SPACE_XS)
        .child(button(
            rox_i18n::t!("canvas-group"),
            icons::LAYERS,
            "canvas-group",
            CanvasPanel::group_selected,
        ))
        .children(holds_group.then(|| {
            button(
                rox_i18n::t!("canvas-ungroup"),
                icons::LAYERS,
                "canvas-ungroup",
                CanvasPanel::ungroup_selected,
            )
        }))
        .child(button(
            rox_i18n::t!("canvas-delete"),
            icons::TRASH,
            "canvas-delete",
            CanvasPanel::delete_selected,
        ))
        .into_any_element()
}
