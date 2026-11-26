//! The canvas editor's arithmetic (ADR 33): what a drag, a resize or a turn
//! does to an item's place, where snapping pulls it, how grouping and
//! ungrouping re-express a place so nothing moves on screen, and the undo
//! stack. Pure functions over rox-canvas's types, so every one of them tests
//! without a window.
//!
//! Drawing the handles and reading the pointer belong to the canvas panel,
//! which calls in here with screen pixels and writes the result through its
//! one edit path.

use std::time::{Duration, Instant};

use rox_canvas::{
    Anchor, Camera, CanvasItem, CanvasState, Content, Frame, Length, Live, MAX_DEPTH, Mat4, Place,
    Position, Viewport, child_frame, paint_order, project, unproject_delta,
};

/// How far an edge or a center is pulled to a snap line, in pixels.
pub(crate) const SNAP_REACH: f32 = 6.;

/// The angle a shift-held turn sticks to, in degrees.
pub(crate) const TURN_SNAP: f32 = 15.;

/// How far Bring Forward and Send Back carry an item past the last sibling
/// in that direction, in pixels of depth.
const DEPTH_STEP: f32 = 10.;

/// How far a new item steps down and right off one already standing where
/// it would go, in the parent's units.
pub(crate) const CASCADE_STEP: f32 = 24.;

/// Steps kept for undo. Each holds two whole item trees, dock dumps included.
const MAX_UNDOS: usize = 200;

/// Edits to one field this close together are one undo step, so a scrubbed
/// slider undoes in one go.
const MERGE_WINDOW: Duration = Duration::from_millis(800);

/// Grids and text stay square to the screen at their own size. Rotation and
/// scale are stored on them and unused.
pub(crate) fn is_flat(content: &Content) -> bool {
    matches!(content, Content::Grid { .. } | Content::Text { .. })
}

// ── Undo ──

#[derive(Clone)]
struct Step<T> {
    before: T,
    after: T,
    /// Set for edits that merge with the next one to the same field.
    key: Option<String>,
    at: Instant,
}

/// The canvas's history: whole item trees before and after each edit. Dock
/// edits inside a grid stay out of it, because a live grid keeps its own
/// tree across an undo and folds its dump back in on the next save.
pub(crate) struct History<T = CanvasState> {
    undos: Vec<Step<T>>,
    redos: Vec<Step<T>>,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            undos: Vec::new(),
            redos: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> History<T> {
    /// A new edit drops whatever was undone, the way every editor does.
    pub(crate) fn record(&mut self, before: T, after: T, key: Option<&str>, now: Instant) {
        if before == after {
            return;
        }

        self.redos.clear();

        if let (Some(key), Some(last)) = (key, self.undos.last_mut())
            && last.key.as_deref() == Some(key)
            && now.saturating_duration_since(last.at) < MERGE_WINDOW
        {
            last.after = after;
            last.at = now;
            return;
        }

        if self.undos.len() >= MAX_UNDOS {
            self.undos.remove(0);
        }

        self.undos.push(Step {
            before,
            after,
            key: key.map(str::to_owned),
            at: now,
        });
    }

    /// The tree to go back to.
    pub(crate) fn undo(&mut self) -> Option<T> {
        let step = self.undos.pop()?;
        let before = step.before.clone();
        self.redos.push(step);
        Some(before)
    }

    /// The tree to go forward to.
    pub(crate) fn redo(&mut self) -> Option<T> {
        let step = self.redos.pop()?;
        let after = step.after.clone();
        self.undos.push(step);
        Some(after)
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.undos.is_empty()
    }

    pub(crate) fn can_redo(&self) -> bool {
        !self.redos.is_empty()
    }
}

// ── The tree ──

/// Ids from the top level down to `id`, `id` last.
pub(crate) fn path_to(items: &[CanvasItem], id: &str) -> Option<Vec<String>> {
    for item in items {
        if item.id == id {
            return Some(vec![item.id.clone()]);
        }

        if let Content::Group { items } = &item.content
            && let Some(mut path) = path_to(items, id)
        {
            path.insert(0, item.id.clone());
            return Some(path);
        }
    }

    None
}

/// The list holding `id` and its index there.
fn siblings_mut<'a>(
    items: &'a mut Vec<CanvasItem>,
    id: &str,
) -> Option<(&'a mut Vec<CanvasItem>, usize)> {
    if let Some(ix) = items.iter().position(|item| item.id == id) {
        return Some((items, ix));
    }

    for item in items.iter_mut() {
        if let Content::Group { items } = &mut item.content
            && let Some(found) = siblings_mut(items, id)
        {
            return Some(found);
        }
    }

    None
}

/// The list holding `id`, read only.
pub(crate) fn siblings<'a>(items: &'a [CanvasItem], id: &str) -> Option<&'a [CanvasItem]> {
    if items.iter().any(|item| item.id == id) {
        return Some(items);
    }

    items.iter().find_map(|item| match &item.content {
        Content::Group { items } => siblings(items, id),
        _ => None,
    })
}

/// The frame `id` is placed in: the stage's, or its group's.
pub(crate) fn parent_frame(canvas: &CanvasState, id: &str, viewport: Viewport) -> Option<Frame> {
    let path = path_to(&canvas.items, id)?;
    let mut frame = Frame::stage(&canvas.stage, viewport);
    let mut items = canvas.items.as_slice();

    for group_id in &path[..path.len() - 1] {
        let group = items.iter().find(|item| &item.id == group_id)?;
        frame = child_frame(group, &frame, &Live::default());

        let Content::Group { items: children } = &group.content else {
            return None;
        };
        items = children;
    }

    Some(frame)
}

/// The frame a group hands its children.
pub(crate) fn group_frame(canvas: &CanvasState, id: &str, viewport: Viewport) -> Option<Frame> {
    let parent = parent_frame(canvas, id, viewport)?;
    let group = canvas.find(id)?;
    matches!(group.content, Content::Group { .. })
        .then(|| child_frame(group, &parent, &Live::default()))
}

/// Takes `id` out wherever it sits, children and dock dump with it.
pub(crate) fn remove(canvas: &mut CanvasState, id: &str) -> Option<CanvasItem> {
    let (list, ix) = siblings_mut(&mut canvas.items, id)?;
    Some(list.remove(ix))
}

/// Whether the groups nest within what this canvas has left of the cap.
pub(crate) fn fits(canvas: &CanvasState, levels_left: usize) -> bool {
    canvas.cost().depth <= levels_left
}

// ── Moving ──

/// The position that carries `item` by `screen_delta` pixels from where it
/// sat at `start`, at its own depth. Each axis stays in its unit, so a drag
/// on a fractional axis writes a fraction.
pub(crate) fn moved(
    item: &CanvasItem,
    parent: &Frame,
    camera: &Camera,
    viewport: Viewport,
    start: Position,
    screen_delta: [f32; 2],
    flat: bool,
) -> Position {
    let mut at_start = item.clone();
    at_start.place.position = start;

    let [du, dv] = unproject_delta(&at_start, parent, camera, viewport, flat, screen_delta);

    start.shifted([du, dv], parent.size)
}

/// The nudge that pulls a box's edges or center onto the nearest line within
/// `reach`, per axis. `moving` is left, top, right, bottom.
pub(crate) fn snap(moving: [f32; 4], xs: &[f32], ys: &[f32], reach: f32) -> [f32; 2] {
    let pull = |own: [f32; 3], lines: &[f32]| {
        let mut best: Option<f32> = None;

        for line in lines {
            for edge in own {
                let d = line - edge;
                if d.abs() <= reach && best.is_none_or(|b| d.abs() < b.abs()) {
                    best = Some(d);
                }
            }
        }

        best.unwrap_or(0.)
    };

    let [left, top, right, bottom] = moving;
    [
        pull([left, (left + right) / 2., right], xs),
        pull([top, (top + bottom) / 2., bottom], ys),
    ]
}

// ── Resizing ──

/// Which sides a resize handle moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Edges {
    /// The eight handles, corners first.
    pub(crate) const ALL: [Edges; 8] = [
        Edges::of(true, true, false, false),
        Edges::of(false, true, true, false),
        Edges::of(false, false, true, true),
        Edges::of(true, false, false, true),
        Edges::of(false, true, false, false),
        Edges::of(false, false, true, false),
        Edges::of(false, false, false, true),
        Edges::of(true, false, false, false),
    ];

    const fn of(left: bool, top: bool, right: bool, bottom: bool) -> Edges {
        Edges {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// A flat item's place after dragging `edges` of its screen box by `delta`.
/// The sides not dragged stay where they were, so the center moves too. Each
/// length keeps its unit, and the box never shrinks under `floor` pixels.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resized(
    item: &CanvasItem,
    parent: &Frame,
    camera: &Camera,
    viewport: Viewport,
    start: [f32; 4],
    edges: Edges,
    delta: [f32; 2],
    floor: [f32; 2],
    flat: bool,
) -> Place {
    let [mut left, mut top, mut right, mut bottom] = start;
    if edges.left {
        left = (left + delta[0]).min(right - floor[0]);
    }
    if edges.right {
        right = (right + delta[0]).max(left + floor[0]);
    }
    if edges.top {
        top = (top + delta[1]).min(bottom - floor[1]);
    }
    if edges.bottom {
        bottom = (bottom + delta[1]).max(top + floor[1]);
    }

    let k = if parent.k > 0. { parent.k } else { 1. };
    held_to_limits(
        &item.place,
        0,
        [edges.left, edges.right],
        [&mut left, &mut right],
        k,
    );
    held_to_limits(
        &item.place,
        1,
        [edges.top, edges.bottom],
        [&mut top, &mut bottom],
        k,
    );
    let units = [(right - left) / k, (bottom - top) / k];

    let mut next = item.clone();
    for axis in 0..2 {
        next.place.size[axis] = match item.place.size[axis] {
            Length::Stage(_) if parent.size[axis] > 0. => {
                Length::Stage(units[axis] / parent.size[axis])
            }
            _ => Length::Units(units[axis]),
        };
    }

    // The new size moves the center about the anchor, so it's measured where
    // it landed and carried to the middle of the new box.
    if let Some(projected) = project(
        &next,
        parent,
        camera,
        viewport,
        None,
        flat,
        &Live::default(),
    ) {
        let (origin, extent) = super::bbox(&projected.corners);
        let landed = [origin[0] + extent[0] / 2., origin[1] + extent[1] / 2.];
        let target = [(left + right) / 2., (top + bottom) / 2.];
        let shift = [target[0] - landed[0], target[1] - landed[1]];

        let [du, dv] = unproject_delta(&next, parent, camera, viewport, flat, shift);
        next.place.position = next.place.position.shifted([du, dv], parent.size);
    }

    next.place
}

/// A dragged side of a size with limits stops at them, measured from the
/// side that isn't dragged. `ends` are the near and far edge in pixels.
fn held_to_limits(place: &Place, axis: usize, dragged: [bool; 2], ends: [&mut f32; 2], k: f32) {
    let limited = place.min_size[axis].is_some() || place.max_size[axis].is_some();
    if !limited || !(dragged[0] || dragged[1]) {
        return;
    }

    let [near, far] = ends;
    let span = place.limited(axis, (*far - *near) / k) * k;
    if dragged[0] {
        *near = *far - span;
    } else {
        *far = *near + span;
    }
}

/// The same place measured from another anchor, so the item stays put.
pub(crate) fn with_anchor(item: &CanvasItem, parent: &Frame, anchor: Anchor) -> Place {
    let size = item.place.size_in(parent.size);
    let from = item.place.anchor.fraction();
    let to = anchor.fraction();

    // The center sits at the anchor's point, plus the position, plus the
    // anchor's own offset into the box, which moves the other way.
    let mut place = item.place;
    place.anchor = anchor;
    for axis in 0..2 {
        let by = (from[axis] - to[axis]) * (parent.size[axis] - size[axis]);
        place.position.xy[axis] = place.position.xy[axis].shifted(by, parent.size[axis]);
    }

    place
}

/// A length switched to the other unit at the same size.
pub(crate) fn other_unit(length: Length, parent_axis: f32) -> Length {
    match length {
        Length::Units(n) if parent_axis > 0. => Length::Stage(n / parent_axis),
        Length::Units(n) => Length::Units(n),
        Length::Stage(f) => Length::Units(f * parent_axis),
    }
}

// ── Turning ──

/// The angle the pointer swept about `center` going from `from` to `to`, in
/// degrees, -180 to 180. Screen y runs down, so clockwise is positive, the
/// way a turn about z goes.
pub(crate) fn ring_turn(center: [f32; 2], from: [f32; 2], to: [f32; 2]) -> f32 {
    let angle = |p: [f32; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
    let swept = (angle(to) - angle(from)).to_degrees();

    (swept + 180.).rem_euclid(360.) - 180.
}

/// A grip turns the item as if the pointer rolled its edge: travel along an
/// arc of `radius` pixels.
pub(crate) fn grip_turn(travel: f32, radius: f32) -> f32 {
    if radius <= 0. {
        return 0.;
    }

    (travel / radius).to_degrees()
}

pub(crate) fn snapped(angle: f32, shift: bool) -> f32 {
    if shift {
        (angle / TURN_SNAP).round() * TURN_SNAP
    } else {
        angle
    }
}

// ── Depth ──

/// The depth that carries sibling `index` exactly one place forward or back
/// in paint order, or None when it's already at that end.
pub(crate) fn step_depth(depths: &[f32], index: usize, forward: bool) -> Option<f32> {
    let order = paint_order(depths);
    let at = order.iter().position(|&ix| ix == index)?;

    let (passed, beyond) = if forward {
        (*order.get(at + 1)?, order.get(at + 2).copied())
    } else {
        (
            *order.get(at.checked_sub(1)?)?,
            at.checked_sub(2).map(|ix| order[ix]),
        )
    };

    let target = depths[passed];
    let sign = if forward { 1. } else { -1. };

    Some(match beyond {
        Some(beyond) if (depths[beyond] - target) * sign > 0. => (target + depths[beyond]) / 2.,
        _ => target + sign * DEPTH_STEP,
    })
}

/// The depth that carries sibling `index` in front of every other sibling,
/// or behind them all, or None when it's already there.
pub(crate) fn end_depth(depths: &[f32], index: usize, front: bool) -> Option<f32> {
    let order = paint_order(depths);
    let at = order.iter().position(|&ix| ix == index)?;
    let last = order.len().checked_sub(1)?;

    match front {
        true if at == last => None,
        false if at == 0 => None,
        true => Some(depths[order[last]] + DEPTH_STEP),
        false => Some(depths[order[0]] - DEPTH_STEP),
    }
}

/// Each sibling's depth, as the painter orders them.
pub(crate) fn depths(siblings: &[CanvasItem], parent: &Frame) -> Vec<f32> {
    siblings
        .iter()
        .map(|item| {
            let frame = child_frame(item, parent, &Live::default());
            frame
                .transform
                .point([frame.size[0] / 2., frame.size[1] / 2., 0.])[2]
        })
        .collect()
}

/// The `position.z` that puts the item at `target` depth. Depth runs along
/// the parent's z axis as it sits in the world, so a turned group scales it.
pub(crate) fn z_for_depth(item: &CanvasItem, parent: &Frame, current: f32, target: f32) -> f32 {
    let along = parent.transform.vector([0., 0., 1.])[2];
    if along.abs() <= f32::EPSILON {
        return item.place.position.z;
    }

    item.place.position.z + (target - current) / along
}

// ── Placing ──

/// Where a new item with `place` goes among `siblings`: its position, stepped
/// down and right by [`CASCADE_STEP`] for as long as a sibling of the same
/// size on the same anchor already stands there, the way a design tool
/// cascades a duplicate. A copy over an identical one reads as a single item.
/// Size counts so a new item over a converted window's full-stage grid stays
/// centred. Positions compare where they land in a parent box of `parent`,
/// and a fractional axis steps by the same units.
pub(crate) fn cascaded(siblings: &[CanvasItem], place: &Place, parent: [f32; 2]) -> Position {
    let taken = |spot: Position| {
        let [x, y, _] = spot.resolve(parent);
        siblings.iter().any(|item| {
            let [sx, sy, _] = item.place.offset_in(parent);
            item.place.anchor == place.anchor
                && item.place.size == place.size
                && (sx - x).abs() < 0.5
                && (sy - y).abs() < 0.5
        })
    };

    let mut spot = place.position;
    while taken(spot) {
        spot = spot.shifted([CASCADE_STEP; 2], parent);
    }

    spot
}

// ── Groups ──

/// Wraps `id` in a new group at its place, with the item at the group's
/// center filling it, so nothing moves on screen. The group's id.
pub(crate) fn group(canvas: &mut CanvasState, id: &str, levels_left: usize) -> Option<String> {
    let group_id = canvas.fresh_id("group");
    let before = canvas.clone();

    let (list, ix) = siblings_mut(&mut canvas.items, id)?;
    let mut item = list.remove(ix);
    let place = std::mem::replace(
        &mut item.place,
        Place {
            anchor: Anchor::Center,
            size: [Length::Stage(1.), Length::Stage(1.)],
            ..Place::default()
        },
    );

    list.insert(
        ix,
        CanvasItem {
            id: group_id.clone(),
            place,
            content: Content::Group { items: vec![item] },
            ..CanvasItem::default()
        },
    );

    if !fits(canvas, levels_left) {
        *canvas = before;
        return None;
    }

    Some(group_id)
}

/// Lifts every child of group `id` into the group's parent, each re-placed
/// so it stays where it was on screen. The group's opacity folds into them.
pub(crate) fn ungroup(canvas: &mut CanvasState, id: &str, viewport: Viewport) -> bool {
    let Some(parent) = parent_frame(canvas, id, viewport) else {
        return false;
    };
    let Some(inner) = group_frame(canvas, id, viewport) else {
        return false;
    };

    let Some((list, ix)) = siblings_mut(&mut canvas.items, id) else {
        return false;
    };
    let group = list.remove(ix);
    let Content::Group { items } = group.content else {
        list.insert(ix, group);
        return false;
    };

    let lifted = items.into_iter().map(|mut child| {
        child.place = reexpress(&child, &inner, &parent);
        child.opacity *= group.opacity;
        child.visible &= group.visible;
        child.locked |= group.locked;
        child
    });
    list.splice(ix..ix, lifted);

    true
}

/// Moves `id` into group `into`, or to the top level for None, re-placed so
/// it stays where it was on screen. Refused into itself, into one of its
/// own children, or past the nesting cap.
pub(crate) fn move_into(
    canvas: &mut CanvasState,
    id: &str,
    into: Option<&str>,
    viewport: Viewport,
    levels_left: usize,
) -> bool {
    if let Some(into) = into {
        let inside_itself =
            path_to(&canvas.items, into).is_none_or(|path| path.iter().any(|step| step == id));
        if inside_itself {
            return false;
        }
    }

    let Some(from) = parent_frame(canvas, id, viewport) else {
        return false;
    };
    let to = match into {
        Some(into) => match group_frame(canvas, into, viewport) {
            Some(frame) => frame,
            None => return false,
        },
        None => Frame::stage(&canvas.stage, viewport),
    };

    let before = canvas.clone();
    let Some(mut item) = remove(canvas, id) else {
        return false;
    };
    item.place = reexpress(&item, &from, &to);

    let list = match into {
        Some(into) => match canvas.find_mut(into).map(|group| &mut group.content) {
            Some(Content::Group { items }) => items,
            _ => {
                *canvas = before;
                return false;
            }
        },
        None => &mut canvas.items,
    };
    list.push(item);

    if !fits(canvas, levels_left) {
        *canvas = before;
        return false;
    }

    true
}

/// The place in `to` that lands `item` where it sits in `from`: the same
/// center, the same turn, the same size on screen. Rotation and scale
/// compose through the frames; the size keeps its unit.
pub(crate) fn reexpress(item: &CanvasItem, from: &Frame, to: &Frame) -> Place {
    let own = child_frame(item, from, &Live::default());
    let size = own.size;
    let center = own.transform.point([size[0] / 2., size[1] / 2., 0.]);

    let Some(back) = invert(&to.transform) else {
        return item.place;
    };

    // What's left of the item's own turn and scale once the new parent's
    // are taken out.
    let linear = back * own.transform;
    let column = linear.vector([1., 0., 0.]);
    let scale = (column[0] * column[0] + column[1] * column[1] + column[2] * column[2]).sqrt();
    let rotation = if scale > f32::EPSILON {
        euler_xyz(&linear, scale)
    } else {
        item.place.rotation
    };

    let [fx, fy] = item.place.anchor.fraction();
    let pivot = [(0.5 - fx) * size[0], (0.5 - fy) * size[1]];
    let local = back.point(center);
    let units = [
        local[0] - pivot[0] - fx * to.size[0],
        local[1] - pivot[1] - fy * to.size[1],
    ];
    let xy = item.place.position.xy;
    let position = Position {
        xy: [0, 1].map(|axis| xy[axis].with_units(units[axis], to.size[axis])),
        z: local[2],
    };

    let length = |old: Length, units: f32, axis: usize| match old {
        Length::Stage(_) if to.size[axis] > 0. => Length::Stage(units / to.size[axis]),
        Length::Stage(_) => Length::Units(units),
        Length::Units(_) => Length::Units(units),
    };

    Place {
        anchor: item.place.anchor,
        position,
        size: [
            length(item.place.size[0], size[0], 0),
            length(item.place.size[1], size[1], 1),
        ],
        rotation,
        scale: if scale > f32::EPSILON {
            scale
        } else {
            item.place.scale
        },
        ..item.place
    }
}

/// Angles a, b, c with `m / scale` = Rx(a) Ry(b) Rz(c), in degrees.
fn euler_xyz(m: &Mat4, scale: f32) -> [f32; 3] {
    let r = |row: usize, col: usize| m.0[row][col] / scale;
    let b = r(0, 2).clamp(-1., 1.).asin();

    // Looking straight along y, x and z turn about the same axis and only
    // their sum is known, so z takes none of it.
    if b.cos().abs() < 1e-5 {
        let a = r(2, 1).atan2(r(1, 1));
        return [a.to_degrees(), b.to_degrees(), 0.];
    }

    let a = (-r(1, 2)).atan2(r(2, 2));
    let c = (-r(0, 1)).atan2(r(0, 0));

    // Adding zero turns a negative zero into a plain one for the readouts.
    [
        a.to_degrees() + 0.,
        b.to_degrees() + 0.,
        c.to_degrees() + 0.,
    ]
}

/// The inverse of an affine transform, None when it flattens space.
fn invert(m: &Mat4) -> Option<Mat4> {
    let a = &m.0;
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0];

    let det = a[0][0] * cof(1, 2, 1, 2) - a[0][1] * cof(1, 2, 0, 2) + a[0][2] * cof(1, 2, 0, 1);
    if det.abs() <= f32::EPSILON {
        return None;
    }

    let inv = [
        [cof(1, 2, 1, 2), -cof(0, 2, 1, 2), cof(0, 1, 1, 2)],
        [-cof(1, 2, 0, 2), cof(0, 2, 0, 2), -cof(0, 1, 0, 2)],
        [cof(1, 2, 0, 1), -cof(0, 2, 0, 1), cof(0, 1, 0, 1)],
    ]
    .map(|row| row.map(|v| v / det));

    let t = [a[0][3], a[1][3], a[2][3]];
    let mut out = Mat4::IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            out.0[r][c] = inv[r][c];
        }
        out.0[r][3] = -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]);
    }

    Some(out)
}

/// How many more group levels a canvas `level` deep can hold.
pub(crate) fn levels_left(level: usize) -> usize {
    MAX_DEPTH.saturating_sub(level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_canvas::{Projected, Stage};
    use serde_json::json;

    const VIEW: Viewport = Viewport {
        width: 1600.,
        height: 900.,
    };

    fn camera() -> Camera {
        Camera {
            fov: 35.,
            parallax: 0.,
        }
    }

    fn stage() -> Frame {
        Frame::stage(&Stage::default(), VIEW)
    }

    fn image(id: &str, position: [f32; 3], size: [f32; 2]) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            place: Place {
                position: Position::units(position),
                size: [Length::Units(size[0]), Length::Units(size[1])],
                ..Place::default()
            },
            content: Content::Image {
                asset: "a".into(),
                fit: Default::default(),
                slice: None,
            },
            ..CanvasItem::default()
        }
    }

    fn grid(id: &str, position: [f32; 3], size: [Length; 2]) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            place: Place {
                position: Position::units(position),
                size,
                ..Place::default()
            },
            content: Content::Grid {
                dock: json!({ "id": id }),
            },
            ..CanvasItem::default()
        }
    }

    /// Where [`cascaded`] puts a new item, in units on the test stage.
    fn cascade_at(siblings: &[CanvasItem], place: &Place) -> [f32; 3] {
        cascaded(siblings, place, stage().size).resolve(stage().size)
    }

    /// A new 100 by 100 item at the stage centre.
    fn new_at_centre() -> Place {
        image("new", [0.; 3], [100., 100.]).place
    }

    #[test]
    fn a_first_item_takes_the_spot_it_was_given() {
        let others = [image("a", [200., 0., 0.], [100., 100.])];
        assert_eq!(cascade_at(&others, &new_at_centre()), [0.; 3]);
        assert_eq!(cascade_at(&[], &new_at_centre()), [0.; 3]);
    }

    #[test]
    fn each_new_copy_steps_down_and_right_past_the_ones_already_there() {
        let step = CASCADE_STEP;
        let mut siblings = Vec::new();

        let mut spots = Vec::new();
        for n in 0..4 {
            let spot = cascade_at(&siblings, &new_at_centre());
            spots.push(spot);
            siblings.push(image(&format!("copy{n}"), spot, [100., 100.]));
        }

        assert_eq!(
            spots,
            [
                [0., 0., 0.],
                [step, step, 0.],
                [2. * step, 2. * step, 0.],
                [3. * step, 3. * step, 0.],
            ]
        );
    }

    #[test]
    fn a_gap_in_the_cascade_is_filled_first() {
        let siblings = [image("b", [CASCADE_STEP, CASCADE_STEP, 0.], [100., 100.])];
        assert_eq!(cascade_at(&siblings, &new_at_centre()), [0.; 3]);
    }

    #[test]
    fn an_item_of_another_size_or_on_another_anchor_leaves_the_spot_free() {
        let full = grid("grid", [0.; 3], [Length::Stage(1.), Length::Stage(1.)]);
        assert_eq!(cascade_at(&[full], &new_at_centre()), [0.; 3]);

        let mut corner = image("corner", [0.; 3], [100., 100.]);
        corner.place.anchor = Anchor::TopLeft;
        assert_eq!(cascade_at(&[corner], &new_at_centre()), [0.; 3]);
    }

    #[test]
    fn a_copy_at_another_depth_still_covers_the_spot_on_screen() {
        let deeper = image("deeper", [0., 0., 300.], [100., 100.]);
        assert_eq!(
            cascade_at(&[deeper], &new_at_centre()),
            [CASCADE_STEP, CASCADE_STEP, 0.]
        );
    }

    fn put(item: &CanvasItem, parent: &Frame) -> Projected {
        project(
            item,
            parent,
            &camera(),
            VIEW,
            None,
            is_flat(&item.content),
            &Live::default(),
        )
        .expect("in front of the camera")
    }

    fn center(projected: &Projected) -> [f32; 2] {
        let c = projected.corners;
        [
            (c[0][0] + c[1][0] + c[2][0] + c[3][0]) / 4.,
            (c[0][1] + c[1][1] + c[2][1] + c[3][1]) / 4.,
        ]
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    fn same_corners(a: &Projected, b: &Projected) -> bool {
        a.corners
            .iter()
            .zip(b.corners)
            .all(|(p, q)| near(p[0], q[0]) && near(p[1], q[1]))
    }

    fn on_screen(canvas: &CanvasState, id: &str) -> Projected {
        let parent = parent_frame(canvas, id, VIEW).expect("placed");
        put(canvas.find(id).expect("present"), &parent)
    }

    #[test]
    fn a_drag_moves_the_item_by_the_drag() {
        let mut item = image("a", [0., 0., 0.], [200., 100.]);
        let before = center(&put(&item, &stage()));

        item.place.position = moved(
            &item,
            &stage(),
            &camera(),
            VIEW,
            Position::units([0.; 3]),
            [40., -25.],
            is_flat(&item.content),
        );
        let after = center(&put(&item, &stage()));

        assert!(near(after[0] - before[0], 40.), "{before:?} {after:?}");
        assert!(near(after[1] - before[1], -25.), "{before:?} {after:?}");
    }

    #[test]
    fn a_far_item_moves_more_units_for_the_same_drag() {
        let near_item = image("n", [0., 0., 0.], [200., 100.]);
        let far_item = image("f", [0., 0., -800.], [200., 100.]);

        let near_move = moved(
            &near_item,
            &stage(),
            &camera(),
            VIEW,
            Position::units([0.; 3]),
            [30., 0.],
            is_flat(&near_item.content),
        );
        let far_move = moved(
            &far_item,
            &stage(),
            &camera(),
            VIEW,
            Position::units([0., 0., -800.]),
            [30., 0.],
            is_flat(&far_item.content),
        );

        let units = |p: Position| p.resolve(stage().size);
        assert!(near(units(near_move)[0], 30.));
        assert!(
            units(far_move)[0] > units(near_move)[0] * 1.5,
            "{far_move:?}"
        );
        assert_eq!(far_move.z, -800.);
    }

    #[test]
    fn snapping_pulls_an_edge_within_reach_and_leaves_one_out_of_reach() {
        let xs = [0., 800., 1600.];
        let ys = [0., 450., 900.];

        // The left edge is 4 px off the stage's left, the top 20 px off.
        let pull = snap([4., 20., 204., 120.], &xs, &ys, SNAP_REACH);
        assert_eq!(pull, [-4., 0.]);

        // A center 3 px past the stage's middle comes back to it.
        let pull = snap([703., 300., 903., 500.], &xs, &ys, SNAP_REACH);
        assert_eq!(pull, [-3., 0.]);
    }

    #[test]
    fn a_resize_from_the_left_keeps_the_right_edge() {
        let item = grid(
            "g",
            [100., 50., 0.],
            [Length::Units(400.), Length::Units(300.)],
        );
        let start = super::super::bbox(&put(&item, &stage()).corners);
        let start = [
            start.0[0],
            start.0[1],
            start.0[0] + start.1[0],
            start.0[1] + start.1[1],
        ];
        let left = Edges {
            left: true,
            ..Edges::default()
        };

        let mut next = item.clone();
        next.place = resized(
            &item,
            &stage(),
            &camera(),
            VIEW,
            start,
            left,
            [-60., 0.],
            [10., 10.],
            is_flat(&item.content),
        );
        let (origin, extent) = super::super::bbox(&put(&next, &stage()).corners);

        assert!(near(origin[0], start[0] - 60.), "{origin:?}");
        assert!(
            near(origin[0] + extent[0], start[2]),
            "{origin:?} {extent:?}"
        );
        assert!(near(origin[1], start[1]));
        assert_eq!(next.place.size[0], Length::Units(460.));
    }

    #[test]
    fn a_stage_length_stays_a_fraction_through_a_resize() {
        let item = grid("g", [0., 0., 0.], [Length::Stage(0.5), Length::Units(300.)]);
        let (origin, extent) = super::super::bbox(&put(&item, &stage()).corners);
        let start = [
            origin[0],
            origin[1],
            origin[0] + extent[0],
            origin[1] + extent[1],
        ];
        let corner = Edges {
            right: true,
            bottom: true,
            ..Edges::default()
        };

        let place = resized(
            &item,
            &stage(),
            &camera(),
            VIEW,
            start,
            corner,
            [160., 20.],
            [10., 10.],
            is_flat(&item.content),
        );

        match place.size[0] {
            Length::Stage(f) => assert!(near(f, 0.6), "{f}"),
            other => panic!("became {other:?}"),
        }
        assert_eq!(place.size[1], Length::Units(320.));
    }

    #[test]
    fn changing_the_anchor_or_the_unit_leaves_the_item_in_place() {
        let mut item = image("a", [40., -30., 0.], [200., 100.]);
        item.place.rotation = [0., 20., 0.];
        let before = put(&item, &stage());

        for anchor in [Anchor::TopLeft, Anchor::BottomRight, Anchor::Top] {
            let mut moved = item.clone();
            moved.place = with_anchor(&item, &stage(), anchor);
            assert!(same_corners(&before, &put(&moved, &stage())), "{anchor:?}");
        }

        let mut switched = item.clone();
        switched.place.size[0] = other_unit(item.place.size[0], stage().size[0]);
        assert!(matches!(switched.place.size[0], Length::Stage(_)));
        assert!(same_corners(&before, &put(&switched, &stage())));
    }

    /// An item whose x is a quarter of the stage and whose y is units.
    fn fractional(id: &str) -> CanvasItem {
        let mut item = image(id, [0.; 3], [200., 100.]);
        item.place.anchor = Anchor::TopLeft;
        item.place.position = Position {
            xy: [Length::Stage(0.25), Length::Units(40.)],
            z: 0.,
        };
        item
    }

    fn fraction_near(length: Length, want: f32) -> bool {
        matches!(length, Length::Stage(f) if (f - want).abs() < 1e-4)
    }

    #[test]
    fn a_drag_on_a_fractional_axis_writes_a_fraction() {
        let mut item = fractional("a");
        let before = center(&put(&item, &stage()));

        let position = moved(
            &item,
            &stage(),
            &camera(),
            VIEW,
            item.place.position,
            [160., 30.],
            false,
        );

        // 160 of the stage's 1600 is a tenth more, and y stays units.
        assert!(fraction_near(position.xy[0], 0.35), "{position:?}");
        assert!(matches!(position.xy[1], Length::Units(n) if near(n, 70.)));

        item.place.position = position;
        let after = center(&put(&item, &stage()));
        assert!(near(after[0] - before[0], 160.), "{before:?} {after:?}");
        assert!(near(after[1] - before[1], 30.), "{before:?} {after:?}");
    }

    #[test]
    fn a_resize_stops_at_a_limit_and_keeps_the_edge_not_dragged() {
        let mut item = grid(
            "g",
            [0., 0., 0.],
            [Length::Stage(0.25), Length::Units(300.)],
        );
        item.place.min_size = [Some(300.), None];
        item.place.max_size = [Some(500.), None];

        let (origin, extent) = super::super::bbox(&put(&item, &stage()).corners);
        let start = [
            origin[0],
            origin[1],
            origin[0] + extent[0],
            origin[1] + extent[1],
        ];
        let resize = |edges: Edges, delta: [f32; 2]| {
            let mut next = item.clone();
            next.place = resized(
                &item,
                &stage(),
                &camera(),
                VIEW,
                start,
                edges,
                delta,
                [10., 10.],
                true,
            );
            (
                next.place.size[0],
                super::super::bbox(&put(&next, &stage()).corners),
            )
        };

        // 300 more to the left asks for 700 and stops at 500, the right edge
        // where it was.
        let left = Edges {
            left: true,
            ..Edges::default()
        };
        let (width, (origin, extent)) = resize(left, [-300., 0.]);
        assert!(fraction_near(width, 500. / 1600.), "{width:?}");
        assert!(
            near(origin[0] + extent[0], start[2]),
            "{origin:?} {extent:?}"
        );
        assert!(near(extent[0], 500.));

        // 250 off the right asks for 150 and stops at 300 from the left edge.
        let right = Edges {
            right: true,
            ..Edges::default()
        };
        let (width, (origin, extent)) = resize(right, [-250., 0.]);
        assert!(fraction_near(width, 300. / 1600.), "{width:?}");
        assert!(near(origin[0], start[0]), "{origin:?}");
        assert!(near(extent[0], 300.));
    }

    #[test]
    fn a_new_anchor_or_a_new_group_keeps_a_fractional_axis_fractional_and_in_place() {
        let item = fractional("pic");
        let before = put(&item, &stage());

        for anchor in [Anchor::Center, Anchor::BottomRight] {
            let mut moved = item.clone();
            moved.place = with_anchor(&item, &stage(), anchor);
            assert!(moved.place.position.xy[0].is_fraction(), "{anchor:?}");
            assert!(!moved.place.position.xy[1].is_fraction(), "{anchor:?}");
            assert!(same_corners(&before, &put(&moved, &stage())), "{anchor:?}");
        }

        let mut turned = image("turned", [100., -50., -80.], [600., 400.]);
        turned.place.rotation = [0., 20., 0.];
        turned.content = Content::Group { items: Vec::new() };
        let mut canvas = CanvasState {
            items: vec![item, turned],
            ..CanvasState::default()
        };

        assert!(move_into(
            &mut canvas,
            "pic",
            Some("turned"),
            VIEW,
            MAX_DEPTH
        ));
        let inside = canvas.find("pic").expect("present");
        assert!(inside.place.position.xy[0].is_fraction());
        assert!(same_corners(&before, &on_screen(&canvas, "pic")));
    }

    #[test]
    fn a_cascade_steps_a_fractional_axis_by_the_same_units() {
        let first = fractional("first");
        let spot = cascaded(std::slice::from_ref(&first), &first.place, stage().size);

        assert!(
            fraction_near(spot.xy[0], 0.25 + CASCADE_STEP / 1600.),
            "{spot:?}"
        );
        assert_eq!(spot.xy[1], Length::Units(40. + CASCADE_STEP));
    }

    #[test]
    fn the_ring_turns_by_the_angle_the_pointer_swept() {
        let c = [100., 100.];
        assert!(near(ring_turn(c, [200., 100.], [100., 200.]), 90.));
        assert!(near(ring_turn(c, [200., 100.], [100., 0.]), -90.));

        // Across the seam behind the center it takes the short way round.
        assert!(near(ring_turn(c, [0., 90.], [0., 110.]), -11.42));

        // A quarter turn about z takes the item's x axis to screen down.
        let mut item = image("a", [0., 0., 0.], [200., 100.]);
        item.place.rotation[2] = ring_turn(c, [200., 100.], [100., 200.]);
        let corners = put(&item, &stage()).corners;
        assert!(corners[1][1] > corners[0][1] + 150., "{corners:?}");
    }

    #[test]
    fn shift_snaps_a_turn_to_fifteen_degrees() {
        assert_eq!(snapped(37., true), 30.);
        assert_eq!(snapped(38., true), 45.);
        assert_eq!(snapped(-8., true), -15.);
        assert_eq!(snapped(37., false), 37.);
        assert!(near(
            grip_turn(std::f32::consts::FRAC_PI_2 * 100., 100.),
            90.
        ));
    }

    #[test]
    fn bring_forward_passes_exactly_one_sibling() {
        let depths = [0., 10., 20., 30.];

        let z = step_depth(&depths, 0, true).expect("room to go");
        let mut moved = depths;
        moved[0] = z;
        assert_eq!(paint_order(&moved), [1, 0, 2, 3]);

        let z = step_depth(&depths, 3, false).expect("room to go");
        let mut moved = depths;
        moved[3] = z;
        assert_eq!(paint_order(&moved), [0, 1, 3, 2]);

        // The front one goes no further, and ties still pass one.
        assert_eq!(step_depth(&depths, 3, true), None);
        let tied = [5., 5., 5.];
        let z = step_depth(&tied, 0, true).expect("room to go");
        let mut moved = tied;
        moved[0] = z;
        assert_eq!(paint_order(&moved)[2], 0);
    }

    #[test]
    fn to_front_and_to_back_pass_every_sibling() {
        let depths = [0., 30., 10., 20.];

        let z = end_depth(&depths, 2, true).expect("room to go");
        let mut moved = depths;
        moved[2] = z;
        assert_eq!(paint_order(&moved).last(), Some(&2));

        let z = end_depth(&depths, 1, false).expect("room to go");
        let mut moved = depths;
        moved[1] = z;
        assert_eq!(paint_order(&moved).first(), Some(&1));

        // Already at that end, or alone.
        assert_eq!(end_depth(&depths, 1, true), None);
        assert_eq!(end_depth(&depths, 0, false), None);
        assert_eq!(end_depth(&[4.], 0, true), None);
    }

    #[test]
    fn grouping_leaves_the_item_where_it_was_on_screen() {
        let mut item = image("pic", [120., -40., -200.], [300., 200.]);
        item.place.rotation = [10., 25., -5.];
        item.place.anchor = Anchor::TopLeft;
        let mut canvas = CanvasState {
            items: vec![item, grid("g", [0., 0., 0.], [Length::Stage(0.5); 2])],
            ..CanvasState::default()
        };
        let before = on_screen(&canvas, "pic");
        let grid_before = on_screen(&canvas, "g");

        let group_id = group(&mut canvas, "pic", MAX_DEPTH).expect("groups");
        assert_eq!(
            path_to(&canvas.items, "pic"),
            Some(vec![group_id, "pic".into()])
        );
        assert!(same_corners(&before, &on_screen(&canvas, "pic")));

        group(&mut canvas, "g", MAX_DEPTH).expect("groups");
        assert!(same_corners(&grid_before, &on_screen(&canvas, "g")));
    }

    #[test]
    fn ungrouping_leaves_every_child_where_it_was_on_screen() {
        let mut a = image("a", [-80., 30., 40.], [200., 120.]);
        a.place.rotation = [0., 30., 0.];
        a.place.scale = 1.5;
        let mut b = image("b", [90., -20., -60.], [100., 160.]);
        b.place.anchor = Anchor::BottomRight;
        let g = grid(
            "g",
            [10., 10., 0.],
            [Length::Stage(0.25), Length::Units(90.)],
        );

        let mut wrapper = CanvasItem {
            id: "group".into(),
            place: Place {
                position: Position::units([200., 50., -100.]),
                size: [Length::Units(500.), Length::Units(400.)],
                rotation: [15., -20., 10.],
                scale: 0.8,
                ..Place::default()
            },
            opacity: 0.5,
            content: Content::Group {
                items: vec![a, b, g],
            },
            ..CanvasItem::default()
        };
        wrapper.place.anchor = Anchor::Left;
        let mut canvas = CanvasState {
            items: vec![wrapper],
            ..CanvasState::default()
        };

        let before: Vec<Projected> = ["a", "b", "g"]
            .iter()
            .map(|id| on_screen(&canvas, id))
            .collect();

        assert!(ungroup(&mut canvas, "group", VIEW));
        assert_eq!(canvas.items.len(), 3);

        for (id, was) in ["a", "b", "g"].iter().zip(&before) {
            let now = on_screen(&canvas, id);
            assert!(same_corners(was, &now), "{id}: {was:?} became {now:?}");
            assert!(near(now.opacity, was.opacity), "{id}");
        }
        assert!(matches!(canvas.items[2].place.size[0], Length::Stage(_)));
    }

    #[test]
    fn moving_into_a_turned_group_keeps_the_screen_position() {
        let mut pic = image("pic", [-300., 100., 50.], [240., 180.]);
        pic.place.rotation = [0., 0., 20.];
        let turned = CanvasItem {
            id: "turned".into(),
            place: Place {
                position: Position::units([250., -60., -150.]),
                rotation: [-20., 35., 5.],
                scale: 1.25,
                ..Place::default()
            },
            content: Content::Group { items: Vec::new() },
            ..CanvasItem::default()
        };
        let mut canvas = CanvasState {
            items: vec![pic, turned],
            ..CanvasState::default()
        };
        let before = on_screen(&canvas, "pic");

        assert!(move_into(
            &mut canvas,
            "pic",
            Some("turned"),
            VIEW,
            MAX_DEPTH
        ));
        assert_eq!(
            path_to(&canvas.items, "pic"),
            Some(vec!["turned".into(), "pic".into()])
        );
        assert!(same_corners(&before, &on_screen(&canvas, "pic")));

        // A group can't go inside itself.
        assert!(!move_into(
            &mut canvas,
            "turned",
            Some("turned"),
            VIEW,
            MAX_DEPTH
        ));

        // And back out to the top keeps it too.
        assert!(move_into(&mut canvas, "pic", None, VIEW, MAX_DEPTH));
        assert!(same_corners(&before, &on_screen(&canvas, "pic")));
    }

    #[test]
    fn grouping_stops_at_the_cap() {
        let mut canvas = CanvasState {
            items: vec![image("pic", [0.; 3], [10., 10.])],
            ..CanvasState::default()
        };

        for _ in 0..2 {
            group(&mut canvas, "pic", 2).expect("under the cap");
        }

        let saved = canvas.clone();
        assert_eq!(group(&mut canvas, "pic", 2), None);
        assert_eq!(canvas, saved);
    }

    #[test]
    fn undo_restores_a_deleted_grid() {
        let dock = json!({ "version": 1, "center": { "kept": true } });
        let mut canvas = CanvasState {
            items: vec![CanvasItem {
                id: "grid".into(),
                content: Content::Grid { dock: dock.clone() },
                ..CanvasItem::default()
            }],
            ..CanvasState::default()
        };
        let mut undo = History::default();

        let before = canvas.clone();
        remove(&mut canvas, "grid").expect("present");
        undo.record(before, canvas.clone(), None, Instant::now());
        assert!(canvas.items.is_empty());

        let back = undo.undo().expect("one step");
        match &back.find("grid").expect("restored").content {
            Content::Grid { dock: restored } => assert_eq!(restored, &dock),
            _ => panic!("came back as something else"),
        }

        assert!(undo.redo().expect("redoes").items.is_empty());
    }

    #[test]
    fn one_drag_is_one_undo_step() {
        let mut canvas = CanvasState {
            items: vec![image("pic", [0.; 3], [100., 100.])],
            ..CanvasState::default()
        };
        let mut undo = History::default();

        // A drag edits live from its start and records once on release.
        let start = canvas.clone();
        for step in 1..=20 {
            let item = canvas.find_mut("pic").expect("present");
            item.place.position = moved(
                &start.items[0],
                &stage(),
                &camera(),
                VIEW,
                Position::units([0.; 3]),
                [step as f32 * 5., 0.],
                is_flat(&start.items[0].content),
            );
        }
        undo.record(start.clone(), canvas.clone(), None, Instant::now());

        assert_eq!(undo.undo(), Some(start));
        assert!(!undo.can_undo());
        assert!(undo.can_redo());
    }

    #[test]
    fn a_scrub_merges_and_a_new_edit_drops_the_redos() {
        let mut undo = History::default();
        let at = |n: usize| {
            let mut canvas = CanvasState::default();
            canvas.camera.fov = n as f32;
            canvas
        };
        let now = Instant::now();

        undo.record(at(0), at(1), Some("fov"), now);
        undo.record(at(1), at(2), Some("fov"), now + Duration::from_millis(100));
        undo.record(
            at(2),
            at(3),
            Some("parallax"),
            now + Duration::from_millis(200),
        );

        assert_eq!(undo.undo(), Some(at(2)));
        assert_eq!(undo.undo(), Some(at(0)));

        undo.record(at(0), at(9), None, now);
        assert!(!undo.can_redo());
    }
}
