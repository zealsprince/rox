//! The arrangement editor the composable strips share: the shown items as
//! chips in display order, a tray of the hidden ones below. Drag along the
//! bar reorders, drag between the rows shows and hides, and the chips'
//! plus and x do the same by click. The config behind it is one ordered
//! list per panel; an item off the list is hidden. Most items show at
//! most once; one the catalog marks repeatable keeps its tray chip while
//! shown, so the bar can hold several.

use gpui::{div, prelude::*, px, svg, Context, Div, Stateful, Window};

use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// One arrangeable item of a strip: its chip label and icon, and the
/// config value it stands for. Each panel declares its catalog as a
/// static slice in stock order; that order is where a re-shown item
/// slots back in.
pub struct ArrangeSpec<V: 'static> {
    pub label: &'static str,
    pub icon: Option<&'static str>,
    pub value: V,
    /// Whether the bar may hold more than one of this item. A repeatable
    /// item keeps its tray chip while shown, so another copy is always
    /// one plus away; the spacers and breaks, mostly.
    pub repeats: bool,
}

/// The value a chip drag carries. The type is generic over the item enum,
/// so a drop only ever dispatches to editors of the same panel kind; the
/// editor id guards the one case left, two settings windows of the same
/// kind open at once. `from` is the bar index the drag left, or None off
/// the tray: with repeatable items on the bar, the index is the identity
/// a value alone can't give.
#[derive(Clone)]
struct ArrangeDrag<V: Clone + 'static> {
    editor: &'static str,
    value: V,
    from: Option<usize>,
    label: &'static str,
    icon: Option<&'static str>,
}

/// The chip that floats under the pointer while one is dragged.
struct ChipPreview {
    label: &'static str,
    icon: Option<&'static str>,
}

impl Render for ChipPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        chip(self.label, self.icon, false)
            .border_1()
            .border_color(palette::border_light())
    }
}

/// The chip look shared by the bar, the tray, and the drag preview.
fn chip(label: &'static str, icon: Option<&'static str>, dimmed: bool) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .children(icon.map(|icon| {
            svg().path(icon).size(px(14.)).text_color(if dimmed {
                palette::text_faint()
            } else {
                palette::text_muted()
            })
        }))
        .child(
            div()
                .text_sm()
                .whitespace_nowrap()
                .text_color(if dimmed {
                    palette::text_muted()
                } else {
                    palette::text()
                })
                .child(label),
        )
}

/// The trailing glyph on a chip: the x that hides a shown item, the plus
/// that brings a hidden one back. Faint until hovered so the chips stay
/// quiet.
fn chip_action(icon: &'static str) -> Div {
    div()
        .flex_none()
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .child(
            svg()
                .path(icon)
                .size(px(12.))
                .text_color(palette::text_faint())
                .hover(|s| s.text_color(palette::text())),
        )
}

/// The bordered row a zone's chips sit in; wraps when the chips outgrow
/// the line, and holds its height while empty so it stays a drop target.
fn well() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(tokens::SPACE_XS)
        .p(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .border_1()
        .border_color(palette::border())
        .min_h(px(36.))
}

/// A zone's tiny caption above its well.
fn caption(text: &'static str) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_faint())
        .child(text)
}

/// `items` with the entry at `from` moved to sit at `to`, both indices
/// into the list as it stands before the move.
fn moved_at<V: Copy>(items: &[V], from: usize, to: usize) -> Vec<V> {
    let mut items = items.to_vec();
    if from >= items.len() {
        return items;
    }
    let value = items.remove(from);
    let to = if from < to { to - 1 } else { to };
    items.insert(to.min(items.len()), value);
    items
}

/// `items` with `value` inserted at `to`, the tray-to-bar drop.
fn inserted<V: Copy>(items: &[V], value: V, to: usize) -> Vec<V> {
    let mut items = items.to_vec();
    items.insert(to.min(items.len()), value);
    items
}

/// `items` without the entry at `at`.
fn removed_at<V: Copy>(items: &[V], at: usize) -> Vec<V> {
    let mut items = items.to_vec();
    if at < items.len() {
        items.remove(at);
    }
    items
}

/// `items` without `value`, every copy of it: the menu toggles' hide.
fn without<V: PartialEq + Copy>(items: &[V], value: V) -> Vec<V> {
    items.iter().copied().filter(|v| *v != value).collect()
}

/// `items` with `value` slotted at its stock position: after every shown
/// item that precedes it in the catalog. On a list still in catalog
/// order that restores exactly where the item used to sit; on a
/// rearranged list it stays deterministic.
fn insert_stock<V: PartialEq + Copy>(
    registry: &'static [ArrangeSpec<V>],
    items: &[V],
    value: V,
) -> Vec<V> {
    let rank = |v: V| {
        registry
            .iter()
            .position(|s| s.value == v)
            .unwrap_or(usize::MAX)
    };
    let target = rank(value);
    let at = items.iter().filter(|v| rank(**v) < target).count();
    let mut items = items.to_vec();
    items.insert(at.min(items.len()), value);
    items
}

/// Show or hide `value` on the list: the panels' quick menu toggles ride
/// this, hiding a shown item and slotting a hidden one back at its stock
/// position.
pub fn toggled<V: PartialEq + Copy>(
    registry: &'static [ArrangeSpec<V>],
    items: &[V],
    value: V,
) -> Vec<V> {
    if items.contains(&value) {
        without(items, value)
    } else {
        insert_stock(registry, items, value)
    }
}

/// Drop repeated values from a dump's list, keeping first positions, so a
/// hand-edited layout can't render an item twice. Items the catalog marks
/// repeatable pass through as often as they appear.
pub fn dedup<V: PartialEq + Copy>(registry: &'static [ArrangeSpec<V>], items: Vec<V>) -> Vec<V> {
    let repeats = |v: &V| {
        registry
            .iter()
            .find(|s| s.value == *v)
            .is_some_and(|s| s.repeats)
    };
    let mut out: Vec<V> = Vec::with_capacity(items.len());
    for item in items {
        if repeats(&item) || !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// The editor itself: the shown bar over the hidden tray. `id` names this
/// editor instance so a drag never lands in another window's copy, and it
/// scopes the whole subtree's element ids: the chips key on their labels,
/// so two editors on one page (the library's line slots) would otherwise
/// share a chip's drag state and a grab on one line would move the other's
/// item. `apply` stores the reordered list and notifies.
pub fn arrange_editor<P: 'static, V: PartialEq + Copy + 'static>(
    id: &'static str,
    registry: &'static [ArrangeSpec<V>],
    items: &[V],
    apply: impl Fn(&mut P, Vec<V>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    let shown: Vec<V> = items.to_vec();

    // The bar: every shown chip drags, drops before the chip it lands on,
    // and hides on its x. The tail past the last chip catches drops meant
    // for the end of the row.
    let mut bar = well();
    for (ix, value) in shown.iter().copied().enumerate() {
        let Some(spec) = registry.iter().find(|s| s.value == value) else {
            continue;
        };
        let drag = ArrangeDrag {
            editor: id,
            value,
            from: Some(ix),
            label: spec.label,
            icon: spec.icon,
        };
        let drop_items = shown.clone();
        let drop_apply = apply.clone();
        let hide_items = shown.clone();
        let hide_apply = apply.clone();
        bar = bar.child(
            // Keyed by position as well as label: two spacers on the bar
            // are two chips, and sharing an id would share their drag
            // state.
            chip(spec.label, spec.icon, false)
                .id((spec.label, ix))
                .cursor_pointer()
                .on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| ChipPreview {
                        label: drag.label,
                        icon: drag.icon,
                    })
                })
                .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
                    if drag.editor == id {
                        style.bg(palette::alpha(palette::accent(), 0x33))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
                    if drag.editor != id {
                        return;
                    }
                    let items = match drag.from {
                        Some(from) => moved_at(&drop_items, from, ix),
                        None => inserted(&drop_items, drag.value, ix),
                    };
                    drop_apply(this, items, cx);
                }))
                .child(chip_action(icons::CLOSE).on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        hide_apply(this, removed_at(&hide_items, ix), cx);
                    }),
                )),
        );
    }
    let tail_items = shown.clone();
    let tail_apply = apply.clone();
    bar = bar.child(
        div()
            .flex_1()
            .min_w(px(24.))
            .h(px(26.))
            .rounded(tokens::RADIUS)
            .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
                if drag.editor == id {
                    style.bg(palette::alpha(palette::accent(), 0x33))
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
                if drag.editor != id {
                    return;
                }
                let to = tail_items.len();
                let items = match drag.from {
                    Some(from) => moved_at(&tail_items, from, to),
                    None => inserted(&tail_items, drag.value, to),
                };
                tail_apply(this, items, cx);
            })),
    );

    // The tray: the hidden chips, dimmed, and the repeatable ones whether
    // hidden or not. A drop from the bar hides the dragged item; a chip's
    // plus (or a drag up into the bar) shows it, or another copy of it.
    let tray_items = shown.clone();
    let tray_apply = apply.clone();
    let mut tray = well()
        .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
            if drag.editor == id {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            } else {
                style
            }
        })
        .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
            if drag.editor != id {
                return;
            }
            // A tray chip dropped back on the tray has nothing to hide.
            let Some(from) = drag.from else {
                return;
            };
            tray_apply(this, removed_at(&tray_items, from), cx);
        }));
    for spec in registry
        .iter()
        .filter(|s| s.repeats || !shown.contains(&s.value))
    {
        let drag = ArrangeDrag {
            editor: id,
            value: spec.value,
            from: None,
            label: spec.label,
            icon: spec.icon,
        };
        let show_items = shown.clone();
        let show_apply = apply.clone();
        let value = spec.value;
        tray = tray.child(
            chip(spec.label, spec.icon, true)
                .id(spec.label)
                .cursor_pointer()
                .on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| ChipPreview {
                        label: drag.label,
                        icon: drag.icon,
                    })
                })
                .child(chip_action(icons::PLUS).on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        show_apply(this, insert_stock(registry, &show_items, value), cx);
                    }),
                )),
        );
    }

    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .child(caption("Shown"))
        .child(bar)
        .child(caption("Hidden"))
        .child(tray)
}

#[cfg(test)]
mod tests {
    use super::{dedup, insert_stock, inserted, moved_at, removed_at, without, ArrangeSpec};

    /// Value 3 stands in for a spacer: the one repeatable entry.
    const REGISTRY: &[ArrangeSpec<u8>] = &[
        ArrangeSpec {
            label: "a",
            icon: None,
            value: 0,
            repeats: false,
        },
        ArrangeSpec {
            label: "b",
            icon: None,
            value: 1,
            repeats: false,
        },
        ArrangeSpec {
            label: "c",
            icon: None,
            value: 2,
            repeats: false,
        },
        ArrangeSpec {
            label: "d",
            icon: None,
            value: 3,
            repeats: true,
        },
    ];

    /// Moving forward accounts for the removed slot, so a drop lands where
    /// the pointer was, and moving to the tail appends. The index ops
    /// leave every other copy of a repeated value where it stood.
    #[test]
    fn index_ops_reorder_insert_and_remove() {
        assert_eq!(moved_at(&[0, 1, 2], 0, 2), vec![1, 0, 2]);
        assert_eq!(moved_at(&[0, 1, 2], 2, 0), vec![2, 0, 1]);
        assert_eq!(moved_at(&[0, 1, 2], 0, 3), vec![1, 2, 0]);
        // Two copies keep their identities: moving the second leaves the
        // first, and removing by index takes just the one.
        assert_eq!(moved_at(&[3, 0, 3], 2, 0), vec![3, 3, 0]);
        assert_eq!(inserted(&[0, 3], 3, 1), vec![0, 3, 3]);
        assert_eq!(removed_at(&[3, 0, 3], 2), vec![3, 0]);
    }

    /// A re-shown item slots back after every shown item that precedes it
    /// in the catalog, restoring stock order on a stock list.
    #[test]
    fn stock_insert_restores_catalog_order() {
        assert_eq!(insert_stock(REGISTRY, &[0, 1, 3], 2), vec![0, 1, 2, 3]);
        assert_eq!(insert_stock(REGISTRY, &[1, 2], 0), vec![0, 1, 2]);
    }

    /// Duplicates in a hand-edited dump collapse to first positions,
    /// except the repeatable entry, which keeps every copy.
    #[test]
    fn dedup_keeps_first_positions_and_repeats() {
        assert_eq!(dedup(REGISTRY, vec![2, 0, 2, 1, 0]), vec![2, 0, 1]);
        assert_eq!(dedup(REGISTRY, vec![3, 0, 3, 3]), vec![3, 0, 3, 3]);
        assert_eq!(without(&[0, 1, 2], 1), vec![0, 2]);
    }
}
