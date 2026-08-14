//! The arrangement editor the composable strips share: the shown items as
//! chips in display order, a tray of the hidden ones below. Drag along a
//! well reorders, drag between the wells and the tray shows and hides, and
//! the chips' plus and x do the same by click. The config behind it is one
//! ordered list per panel; an item off the list is hidden. Most items show
//! at most once per row, so each line can carry its own copy; one the
//! catalog marks repeatable keeps its tray chip while shown, so a well
//! can hold several. A panel whose layout stacks rows edits them through
//! [`arrange_rows_editor`]: one well per row, a button below adding the
//! next; the flat [`arrange_editor`] is the same thing capped at one well.

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
    /// Whether one row may hold more than one of this item. A repeatable
    /// item keeps its tray chip while shown, so another copy is always
    /// one plus away; the spacers and dividers, mostly. A non-repeatable
    /// item is unique per row, not per editor: each row can carry its own.
    pub repeats: bool,
}

/// The value a chip drag carries. The type is generic over the item enum,
/// so a drop only ever dispatches to editors of the same panel kind; the
/// editor id guards the one case left, two settings windows of the same
/// kind open at once. `from` is the (row, index) place the drag left, or
/// None off the tray: with repeatable items on a well, the place is the
/// identity a value alone can't give.
#[derive(Clone)]
struct ArrangeDrag<V: Clone + 'static> {
    editor: &'static str,
    value: V,
    from: Option<(usize, usize)>,
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

/// Whether the catalog lets `value` sit twice on one row.
fn can_repeat<V: PartialEq + Copy>(registry: &[ArrangeSpec<V>], value: V) -> bool {
    registry
        .iter()
        .find(|s| s.value == value)
        .is_some_and(|s| s.repeats)
}

/// Insert `value` into `row` at `at`. Uniqueness is per row: a
/// non-repeatable value already on the row leaves first, pulling the
/// drop point along when it sat before it, so a drop replaces the row's
/// copy instead of doubling it while other rows keep theirs.
fn insert_row_unique<V: PartialEq + Copy>(row: &mut Vec<V>, value: V, at: usize, unique: bool) {
    let mut at = at.min(row.len());
    if unique {
        let mut ix = 0;
        row.retain(|v| {
            let keep = *v != value;
            if !keep && ix < at {
                at -= 1;
            }
            ix += 1;
            keep
        });
    }
    row.insert(at.min(row.len()), value);
}

/// `rows` with the chip at `from` moved to sit at `to`, both (row, index)
/// places into the rows as they stand before the move.
fn moved_at<V: PartialEq + Copy>(
    registry: &[ArrangeSpec<V>],
    rows: &[Vec<V>],
    from: (usize, usize),
    to: (usize, usize),
) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    let Some(value) = rows.get(from.0).and_then(|row| row.get(from.1)).copied() else {
        return rows;
    };
    rows[from.0].remove(from.1);
    let (to_row, mut to_ix) = to;
    // Only a same-row forward move has to account for the removed slot.
    if to_row == from.0 && from.1 < to_ix {
        to_ix -= 1;
    }
    let unique = !can_repeat(registry, value);
    if let Some(row) = rows.get_mut(to_row) {
        insert_row_unique(row, value, to_ix, unique);
    }
    rows
}

/// `rows` with `value` inserted at `to`, the tray-to-well drop.
fn inserted<V: PartialEq + Copy>(
    registry: &[ArrangeSpec<V>],
    rows: &[Vec<V>],
    value: V,
    to: (usize, usize),
) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    let unique = !can_repeat(registry, value);
    if let Some(row) = rows.get_mut(to.0) {
        insert_row_unique(row, value, to.1, unique);
    }
    rows
}

/// `rows` without the chip at `at`.
fn removed_at<V: Copy>(rows: &[Vec<V>], at: (usize, usize)) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    if let Some(row) = rows.get_mut(at.0) {
        if at.1 < row.len() {
            row.remove(at.1);
        }
    }
    rows
}

/// `rows` without row `at`, the empty well's x. The last row stays, so
/// the editor always shows at least one well.
fn removed_row<V: Copy>(rows: &[Vec<V>], at: usize) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    if rows.len() > 1 && at < rows.len() {
        rows.remove(at);
    }
    rows
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

/// The flat editor most strips use: [`arrange_rows_editor`] capped at one
/// well, so `apply` gets the single row back as the plain list it stores.
pub fn arrange_editor<P: 'static, V: PartialEq + Copy + 'static>(
    id: &'static str,
    registry: &'static [ArrangeSpec<V>],
    items: &[V],
    apply: impl Fn(&mut P, Vec<V>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    arrange_rows_editor(
        id,
        registry,
        &[items.to_vec()],
        Some(1),
        move |this, rows, cx| apply(this, rows.into_iter().next().unwrap_or_default(), cx),
        cx,
    )
}

/// The editor itself: one well per row over the hidden tray, and while
/// `max_rows` allows another, a button below the wells that opens one.
/// A drag moves a chip along its well or into any other, an empty well
/// keeps an x that drops the row, and `apply` stores the edited rows and
/// notifies. `id` names this editor instance so a drag never lands in
/// another window's copy, and it scopes the whole subtree's element ids:
/// the chips key on their labels, so two editors on one page would
/// otherwise share a chip's drag state.
pub fn arrange_rows_editor<P: 'static, V: PartialEq + Copy + 'static>(
    id: &'static str,
    registry: &'static [ArrangeSpec<V>],
    rows: &[Vec<V>],
    max_rows: Option<usize>,
    apply: impl Fn(&mut P, Vec<Vec<V>>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    // At least one well, so an emptied config still has a drop target.
    let rows: Vec<Vec<V>> = if rows.is_empty() {
        vec![Vec::new()]
    } else {
        rows.to_vec()
    };

    // The wells: every shown chip drags, drops before the chip it lands
    // on, and hides on its x. The tail past a well's last chip catches
    // drops meant for the end of that row.
    let mut wells = div().flex().flex_col().gap(tokens::SPACE_XS);
    for (row_ix, row) in rows.iter().enumerate() {
        let mut bar = well();
        for (ix, value) in row.iter().copied().enumerate() {
            let Some(spec) = registry.iter().find(|s| s.value == value) else {
                continue;
            };
            let drag = ArrangeDrag {
                editor: id,
                value,
                from: Some((row_ix, ix)),
                label: spec.label,
                icon: spec.icon,
            };
            let drop_rows = rows.clone();
            let drop_apply = apply.clone();
            let hide_rows = rows.clone();
            let hide_apply = apply.clone();
            bar = bar.child(
                // Keyed by position as well as label: two spacers on a
                // well are two chips, and sharing an id would share their
                // drag state. The place folds to one integer for the id.
                chip(spec.label, spec.icon, false)
                    .id((spec.label, (row_ix << 16) | ix))
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
                        let rows = match drag.from {
                            Some(from) => moved_at(registry, &drop_rows, from, (row_ix, ix)),
                            None => inserted(registry, &drop_rows, drag.value, (row_ix, ix)),
                        };
                        drop_apply(this, rows, cx);
                    }))
                    .child(chip_action(icons::CLOSE).on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            hide_apply(this, removed_at(&hide_rows, (row_ix, ix)), cx);
                        }),
                    )),
            );
        }
        let tail_rows = rows.clone();
        let tail_apply = apply.clone();
        let tail_to = (row_ix, row.len());
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
                    let rows = match drag.from {
                        Some(from) => moved_at(registry, &tail_rows, from, tail_to),
                        None => inserted(registry, &tail_rows, drag.value, tail_to),
                    };
                    tail_apply(this, rows, cx);
                })),
        );
        if row.is_empty() && rows.len() > 1 {
            let drop_rows = rows.clone();
            let drop_apply = apply.clone();
            bar = bar.child(chip_action(icons::CLOSE).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    drop_apply(this, removed_row(&drop_rows, row_ix), cx);
                }),
            ));
        }
        wells = wells.child(bar);
    }
    if max_rows.is_none_or(|max| rows.len() < max) {
        let add_rows = rows.clone();
        let add_apply = apply.clone();
        let drop_rows = rows.clone();
        let drop_apply = apply.clone();
        wells = wells.child(
            div()
                .id("add-row")
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(tokens::SPACE_XS)
                .p(tokens::SPACE_XS)
                .min_h(px(36.))
                .rounded(tokens::RADIUS)
                .border_1()
                .border_dashed()
                .border_color(palette::border())
                .cursor_pointer()
                .hover(|s| s.border_color(palette::border_light()))
                .child(
                    svg()
                        .path(icons::PLUS)
                        .size(px(12.))
                        .text_color(palette::text_faint()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child("Add Row"),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut rows = add_rows.clone();
                    rows.push(Vec::new());
                    add_apply(this, rows, cx);
                }))
                // A chip dropped on the button starts its row with it.
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
                    let mut rows = drop_rows.clone();
                    rows.push(Vec::new());
                    let to = (rows.len() - 1, 0);
                    let rows = match drag.from {
                        Some(from) => moved_at(registry, &rows, from, to),
                        None => inserted(registry, &rows, drag.value, to),
                    };
                    drop_apply(this, rows, cx);
                })),
        );
    }

    // The tray: the chips some open row still lacks, dimmed, and the
    // repeatable ones always. Uniqueness is per row, so a piece shown on
    // one line stays offered until every line holds it. A drop from a
    // well hides the dragged item; a chip's plus (or a drag up into a
    // well) shows it at its stock position on the first row without it.
    let tray_rows = rows.clone();
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
            tray_apply(this, removed_at(&tray_rows, from), cx);
        }));
    for spec in registry
        .iter()
        .filter(|s| s.repeats || rows.iter().any(|row| !row.contains(&s.value)))
    {
        let drag = ArrangeDrag {
            editor: id,
            value: spec.value,
            from: None,
            label: spec.label,
            icon: spec.icon,
        };
        let show_rows = rows.clone();
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
                        let mut rows = show_rows.clone();
                        let target = rows
                            .iter()
                            .position(|row| !row.contains(&value))
                            .unwrap_or(0);
                        rows[target] = insert_stock(registry, &rows[target], value);
                        show_apply(this, rows, cx);
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
        .child(wells)
        .child(caption("Hidden"))
        .child(tray)
}

#[cfg(test)]
mod tests {
    use super::{
        dedup, insert_stock, inserted, moved_at, removed_at, removed_row, without, ArrangeSpec,
    };

    /// The place ops that read the catalog for repeatability, pinned to
    /// the test registry.
    fn moved(rows: &[Vec<u8>], from: (usize, usize), to: (usize, usize)) -> Vec<Vec<u8>> {
        moved_at(REGISTRY, rows, from, to)
    }
    fn insert(rows: &[Vec<u8>], value: u8, to: (usize, usize)) -> Vec<Vec<u8>> {
        inserted(REGISTRY, rows, value, to)
    }

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
    /// the pointer was, and moving to the tail appends. The place ops
    /// leave every other copy of a repeated value where it stood.
    #[test]
    fn place_ops_reorder_insert_and_remove() {
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 0), (0, 2)), [vec![1, 0, 2]]);
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 2), (0, 0)), [vec![2, 0, 1]]);
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 0), (0, 3)), [vec![1, 2, 0]]);
        // Two copies keep their identities: moving the second leaves the
        // first, and removing by place takes just the one.
        assert_eq!(moved(&[vec![3, 0, 3]], (0, 2), (0, 0)), [vec![3, 3, 0]]);
        assert_eq!(insert(&[vec![0, 3]], 3, (0, 1)), [vec![0, 3, 3]]);
        assert_eq!(removed_at(&[vec![3, 0, 3]], (0, 2)), [vec![3, 0]]);
    }

    /// A chip moved across rows lands where the pointer was; only a
    /// same-row forward move adjusts for the removed slot. A row drops
    /// whole, except the last one.
    #[test]
    fn row_ops_move_across_and_drop_rows() {
        assert_eq!(
            moved(&[vec![0, 1], vec![2]], (0, 1), (1, 0)),
            [vec![0], vec![1, 2]]
        );
        assert_eq!(
            moved(&[vec![0], vec![1, 2]], (1, 0), (0, 1)),
            [vec![0, 1], vec![2]]
        );
        assert_eq!(removed_row(&[vec![0], vec![]], 1), [vec![0]]);
        assert_eq!(removed_row::<u8>(&[vec![0]], 0), [vec![0]]);
    }

    /// Uniqueness is per row: a non-repeatable value landing on a row
    /// that already holds it replaces that row's copy, wherever it sat,
    /// while another row's copy stays; a repeatable value stacks.
    #[test]
    fn row_landings_keep_a_row_unique() {
        assert_eq!(
            moved(&[vec![0, 1], vec![0]], (1, 0), (0, 2)),
            [vec![1, 0], vec![]]
        );
        assert_eq!(
            insert(&[vec![0, 1], vec![0]], 0, (0, 2)),
            [vec![1, 0], vec![0]]
        );
        assert_eq!(insert(&[vec![3, 0]], 3, (0, 2)), [vec![3, 0, 3]]);
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
