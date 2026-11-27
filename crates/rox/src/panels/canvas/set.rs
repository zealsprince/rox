//! Picking several items in design mode and what the editor does to them
//! together: the modifier press, the marquee, Select All, a set carried by
//! a drag, and the set's edits. Every edit goes through the panel's one
//! edit path, so each is one undo step. The rules and the arithmetic are in
//! [`crate::canvas::selection`].

use gpui::{Context, Modifiers, PathBuilder, Pixels, Point, Window};
use rox_canvas::{Anchor, CanvasItem, Content, Position};
use rox_design::palette;

use super::{Begin, CanvasPanel, Gesture, Shape, at_origin, paint_outline, stroke};
use crate::canvas::align::Line;
use crate::canvas::edit;
use crate::canvas::selection::{self, Member, Order, Selection};

/// Another selected item a move carries along with the one pressed.
pub(super) struct Rider {
    pub(super) id: String,
    /// As it stood at the press.
    pub(super) item: CanvasItem,
    flat: bool,
}

/// A drag on empty stage, panel-relative.
pub(super) struct Marquee {
    from: [f32; 2],
    to: [f32; 2],
    /// The groups whose children it picks from, outermost first.
    scope: Vec<String>,
    /// What was held before it, kept under a modifier.
    base: Selection,
}

/// Where each rider goes for a press moved by `delta` pixels: the same
/// distance on screen at its own depth, each axis in its own unit.
pub(super) fn carry(gesture: &Gesture, delta: [f32; 2]) -> Vec<(String, Position)> {
    gesture
        .riders
        .iter()
        .map(|rider| {
            let position = edit::moved(
                &rider.item,
                &gesture.parent,
                &gesture.camera,
                gesture.viewport,
                rider.item.place.position,
                delta,
                rider.flat,
            );
            (rider.id.clone(), position)
        })
        .collect()
}

/// Shift and Ctrl (Cmd on macOS) both add to a set on the stage, which has
/// no row order to take a run along.
fn adds(modifiers: Modifiers) -> bool {
    modifiers.shift || modifiers.secondary()
}

impl CanvasPanel {
    /// A press on an item. A plain press on something selected, or inside
    /// it, keeps the selection so a drag moves it, a whole set included,
    /// and steps the selection only if it lets go without moving
    /// ([`selection::press`]). A plain press elsewhere picks at once. Under
    /// a modifier it goes in or out of the set. Whatever ends up selected
    /// under the press moves with the drag.
    pub(super) fn pick(
        &mut self,
        local: [f32; 2],
        chain: &[String],
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending = None;

        if adds(modifiers) {
            let target = self.target_in_set(local, chain);
            let items = &self.config.canvas.items;
            let same = self
                .selected
                .primary()
                .is_none_or(|primary| selection::same_parent(items, primary, &target));
            self.selected.toggle(&target, same);

            self.activate(window, cx);
            cx.notify();
            if self.selected.contains(&target) {
                self.begin(&target, local, Begin::Move, cx);
            }
            return;
        }

        let again = self.again(local, chain);
        let canvas = &self.config.canvas;
        let locked = |id: &str| canvas.find(id).is_none_or(|item| item.locked);
        let press = selection::press(&self.selected, chain, again, locked);

        match press.now {
            Some(id) => self.select(Some(id), window, cx),
            None => {
                self.activate(window, cx);
                cx.notify();
            }
        }
        self.pending = press.on_release.map(|id| (id, local));

        if let Some(id) = press.drags {
            self.begin(&id, local, Begin::Move, cx);
        }
    }

    /// Under a modifier the press picks at the set's own level: the item
    /// along the chain that sits in the set's parent, so a set inside a group
    /// grows from the stage. Anything else picks as a plain press does.
    fn target_in_set(&mut self, local: [f32; 2], chain: &[String]) -> String {
        let scope = selection::scope(&self.config.canvas.items, &self.selected);
        let inside = chain.len() > scope.len() && chain[..scope.len()] == scope[..];
        // Outside the set's parent nothing along the chain is held, so a
        // press there picks the outermost.
        self.again(local, chain);
        if self.selected.is_empty() || !inside {
            return chain[0].clone();
        }

        chain[scope.len()].clone()
    }

    /// A press on empty stage. A plain one clears the selection and picks at
    /// the top level; under a modifier it adds to what's held, among the
    /// held items' siblings.
    pub(super) fn start_marquee(
        &mut self,
        local: [f32; 2],
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_press = None;
        self.pending = None;

        let (scope, base) = match adds(modifiers) {
            true => (
                selection::scope(&self.config.canvas.items, &self.selected),
                self.selected.clone(),
            ),
            false => (Vec::new(), Selection::default()),
        };
        self.selected = base.clone();
        self.activate(window, cx);

        self.marquee = Some(Marquee {
            from: local,
            to: local,
            scope,
            base,
        });
        cx.notify();
    }

    /// Picks every unlocked item in the marquee's scope whose quad the box
    /// touches. Touch rather than enclose: a marquee only starts on empty
    /// stage, and a stage of large or tilted items leaves little room to
    /// start outside one and sweep all the way round it.
    pub(super) fn marquee_to(&mut self, local: [f32; 2], held: bool, cx: &mut Context<Self>) {
        // The release happened where this window couldn't see it.
        if !held {
            self.marquee = None;
            cx.notify();
            return;
        }

        let Some(marquee) = &mut self.marquee else {
            return;
        };
        marquee.to = local;

        let rect = selection::rect_between(marquee.from, marquee.to);
        let mut ids = marquee.base.ids().to_vec();
        ids.extend(
            self.shapes
                .iter()
                .filter(|shape| shape.groups == marquee.scope && !shape.locked)
                .filter(|shape| selection::touches(&shape.corners, rect))
                .map(|shape| shape.id.clone()),
        );
        self.selected.replace(ids);
        cx.notify();
    }

    pub(super) fn marquee_rect(&self) -> Option<[f32; 4]> {
        let marquee = self.marquee.as_ref()?;
        Some(selection::rect_between(marquee.from, marquee.to))
    }

    /// Every item beside the primary in its parent: the stage's items, or
    /// the group's.
    pub(super) fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = &self.config.canvas.items;
        let scope = selection::scope(items, &self.selected);
        let Some(children) = selection::children_at(items, &scope) else {
            return;
        };

        let primary = self.selected.primary().map(str::to_string);
        self.selected
            .replace(children.iter().map(|item| item.id.clone()).collect());
        if let Some(primary) = primary {
            self.selected.raise(&primary);
        }

        self.activate(window, cx);
        cx.notify();
    }

    /// A press on a row of the inspector's tree, with the list modifiers:
    /// shift for a run of siblings, Ctrl or Cmd to toggle one.
    pub(crate) fn tree_pick(
        &mut self,
        id: &str,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = selection::tree_click(
            &self.config.canvas.items,
            &self.selected,
            id,
            modifiers.shift,
            modifiers.secondary(),
        );

        self.activate(window, cx);
        cx.notify();
    }

    /// The other selected items a move from `id` carries. Locked ones stay.
    pub(super) fn riders(&self, id: &str, cx: &Context<Self>) -> Vec<Rider> {
        if !self.selected.several() || !self.selected.contains(id) {
            return Vec::new();
        }

        self.selected
            .ids()
            .iter()
            .filter(|other| *other != id)
            .filter_map(|other| self.config.canvas.find(other))
            .filter(|item| !item.locked)
            .map(|item| Rider {
                id: item.id.clone(),
                item: item.clone(),
                flat: self.is_flat(item, cx),
            })
            .collect()
    }

    /// The pressed item's screen box, or the box around it and the items it
    /// carries, which is what snaps.
    pub(super) fn start_box(&self, shape: &Shape, riders: &[String]) -> [f32; 4] {
        let quads: Vec<[[f32; 2]; 4]> = std::iter::once(shape)
            .chain(riders.iter().filter_map(|id| self.shape(id)))
            .map(|shape| shape.corners)
            .collect();

        match selection::around(&quads) {
            Some([[left, top], _, [right, bottom], _]) => [left, top, right, bottom],
            None => shape.edges(),
        }
    }

    /// The selected items' shapes while there are several, for the set's
    /// outlines and its box.
    pub(super) fn set_shapes(&self, active: bool) -> Vec<Shape> {
        if !active || !self.selected.several() {
            return Vec::new();
        }

        self.selected
            .ids()
            .iter()
            .filter_map(|id| self.shape(id))
            .cloned()
            .collect()
    }

    /// The selected items that the set tools move: every one not locked.
    fn movable(&self) -> Vec<String> {
        let canvas = &self.config.canvas;
        self.selected
            .ids()
            .iter()
            .filter(|id| canvas.find(id).is_some_and(|item| !item.locked))
            .cloned()
            .collect()
    }

    pub(super) fn group_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected.ids().to_vec();
        let viewport = self.viewport();
        let levels = self.levels_left();

        let mut made = None;
        self.change(
            |canvas| made = edit::group_many(canvas, &ids, viewport, levels),
            window,
            cx,
        );
        if made.is_some() {
            self.select(made, window, cx);
        }
    }

    /// Ungroups every group in the set.
    pub(super) fn ungroup_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let canvas = &self.config.canvas;
        let groups: Vec<String> = self
            .selected
            .ids()
            .iter()
            .filter(|id| {
                canvas
                    .find(id)
                    .is_some_and(|item| matches!(item.content, Content::Group { .. }))
            })
            .cloned()
            .collect();
        if groups.is_empty() {
            return;
        }

        let viewport = self.viewport();
        self.change(
            |canvas| {
                for id in &groups {
                    edit::ungroup(canvas, id, viewport);
                }
            },
            window,
            cx,
        );
        self.select(None, window, cx);
    }

    /// The order tools on a set: it moves through its siblings' paint order
    /// together and keeps its own order.
    pub(super) fn order_set(&mut self, order: Order, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lead) = self.selected.primary().map(str::to_string) else {
            return;
        };
        let held = self.selected.clone();

        let viewport = self.viewport();
        self.change(
            |canvas| {
                let Some(parent) = edit::parent_frame(canvas, &lead, viewport) else {
                    return;
                };
                let Some(siblings) = edit::siblings(&canvas.items, &lead) else {
                    return;
                };

                let picked: Vec<bool> = siblings
                    .iter()
                    .map(|item| held.contains(&item.id))
                    .collect();
                let depths = edit::depths(siblings, &parent);
                let moves: Vec<(String, f32)> = selection::reordered(&depths, &picked, order)
                    .into_iter()
                    .map(|(ix, depth)| {
                        let z = edit::z_for_depth(&siblings[ix], &parent, depths[ix], depth);
                        (siblings[ix].id.clone(), z)
                    })
                    .collect();

                for (id, z) in moves {
                    if let Some(item) = canvas.find_mut(&id) {
                        item.place.position.z = z;
                    }
                }
            },
            window,
            cx,
        );
    }

    /// Aligns the set's unlocked items to one another's bounds. In a stack
    /// only across its axis.
    pub(super) fn align_set(&mut self, line: Line, window: &mut Window, cx: &mut Context<Self>) {
        if self.stacked_along(line.axis_and_end().0) {
            return;
        }
        self.arrange_set(window, cx, move |members, parent| {
            selection::aligned(members, parent, line)
        });
    }

    /// Equal gaps between the set's unlocked items along `axis`, 0 for x.
    pub(crate) fn distribute_set(
        &mut self,
        axis: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.stacked_along(axis) {
            return;
        }
        self.arrange_set(window, cx, move |members, parent| {
            selection::distributed(members, parent, axis)
        });
    }

    /// Whether the set sits in a stack that runs along `axis`.
    fn stacked_along(&self, axis: usize) -> bool {
        self.selected
            .primary()
            .and_then(|id| edit::stack_of(&self.config.canvas, id))
            .is_some_and(|stack| stack.axis.along() == axis)
    }

    fn arrange_set(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        arrange: impl FnOnce(&[Member], [f32; 2]) -> Option<Vec<rox_canvas::Place>>,
    ) {
        let ids = self.movable();
        let Some(first) = ids.first().cloned() else {
            return;
        };

        // Whether each draws flat is read off its last paint, before the edit.
        let flats: Vec<bool> = ids
            .iter()
            .map(|id| {
                self.config
                    .canvas
                    .find(id)
                    .is_some_and(|item| self.is_flat(item, cx))
            })
            .collect();

        let viewport = self.viewport();
        self.change(
            |canvas| {
                let Some(parent) = edit::parent_frame(canvas, &first, viewport) else {
                    return;
                };
                let members: Option<Vec<Member>> = ids
                    .iter()
                    .zip(&flats)
                    .map(|(id, &flat)| {
                        let place = canvas.find(id)?.place;
                        let cell = edit::parent_frame(canvas, id, viewport)?.cell;
                        Some(Member { place, cell, flat })
                    })
                    .collect();
                let Some(places) = members.and_then(|members| arrange(&members, parent.size))
                else {
                    return;
                };

                for (id, place) in ids.iter().zip(places) {
                    if let Some(item) = canvas.find_mut(id) {
                        item.place = place;
                    }
                }
            },
            window,
            cx,
        );
    }

    /// The inspector's shared rows: one edit applied to every selected item.
    pub(crate) fn edit_set_later(
        &mut self,
        key: &'static str,
        edit: impl Fn(&mut CanvasItem) + 'static,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selected.ids().to_vec();
        self.change_later(
            key,
            move |canvas| {
                for id in &ids {
                    if let Some(item) = canvas.find_mut(id) {
                        edit(item);
                    }
                }
            },
            cx,
        );
    }

    /// Every id in `ids` on `anchor`, each staying where it is.
    pub(crate) fn anchor_all(
        &mut self,
        ids: Vec<String>,
        anchor: Anchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.viewport();
        self.change(
            |canvas| {
                for id in &ids {
                    let Some(parent) = edit::parent_frame(canvas, id, viewport) else {
                        continue;
                    };
                    if let Some(item) = canvas.find_mut(id) {
                        item.place = edit::with_anchor(item, &parent, anchor);
                    }
                }
            },
            window,
            cx,
        );
    }
}

/// Each selected item's outline, then the box around them all. The
/// single-item handles stay hidden: a set doesn't resize or turn.
pub(super) fn paint_set(members: &[Shape], origin: Point<Pixels>, window: &mut Window) {
    if members.len() < 2 {
        return;
    }

    for shape in members {
        paint_outline(shape, origin, window);
    }

    let quads: Vec<[[f32; 2]; 4]> = members.iter().map(|shape| shape.corners).collect();
    if let Some(around) = selection::around(&quads) {
        let points = around.map(|p| at_origin(p, origin));
        stroke(&points, 1., palette::alpha(palette::accent(), 0xb0), window);
    }
}

pub(super) fn paint_marquee(rect: [f32; 4], origin: Point<Pixels>, window: &mut Window) {
    let [left, top, right, bottom] = rect;
    let points =
        [[left, top], [right, top], [right, bottom], [left, bottom]].map(|p| at_origin(p, origin));

    let mut fill = PathBuilder::fill();
    fill.add_polygon(&points, true);
    if let Ok(path) = fill.build() {
        window.paint_path(path, palette::alpha(palette::accent(), 0x22));
    }
    stroke(&points, 1., palette::accent(), window);
}
