//! The canvas editor's selection (ADR 33): the items picked in design mode,
//! and the arithmetic the editor runs on several of them at once. Pure, so
//! every rule is a test.
//!
//! A selection lives inside one parent, the stage or one group, the way
//! design tools keep it: a set drags, aligns and reorders in one parent's
//! units and one paint order, which items under two parents don't share.
//! Picking an item under another parent starts a new selection with it.
//!
//! The last item picked is the primary. Everything that acts on one item
//! (the inspector's rows for it, the handles, Step Out) reads the primary,
//! so a selection of one behaves as the single selection always did.

use rox_canvas::{CanvasItem, Content, Place, StackCell, paint_order};

use crate::canvas::align::{self, Line};
use crate::canvas::edit;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Selection {
    /// In the order they were picked, the primary last.
    ids: Vec<String>,
}

impl Selection {
    pub(crate) fn primary(&self) -> Option<&str> {
        self.ids.last().map(String::as_str)
    }

    pub(crate) fn ids(&self) -> &[String] {
        &self.ids
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// More than one item, where the set tools take over from the
    /// single-item ones.
    pub(crate) fn several(&self) -> bool {
        self.ids.len() > 1
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|held| held == id)
    }

    /// Just `id`, or nothing.
    pub(crate) fn set(&mut self, id: Option<String>) {
        self.ids = id.into_iter().collect();
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
    }

    /// Exactly `ids`, the last one primary. A repeat keeps its last place.
    pub(crate) fn replace(&mut self, ids: Vec<String>) {
        self.ids.clear();
        for id in ids {
            self.push(id);
        }
    }

    /// Makes a held item the primary, keeping the rest.
    pub(crate) fn raise(&mut self, id: &str) {
        if let Some(at) = self.ids.iter().position(|held| held == id) {
            let id = self.ids.remove(at);
            self.ids.push(id);
        }
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.ids.retain(|id| keep(id));
    }

    /// `id` in or out. Under another parent than the set's, it starts a new
    /// selection instead.
    pub(crate) fn toggle(&mut self, id: &str, same_parent: bool) {
        if !same_parent {
            self.set(Some(id.to_string()));
            return;
        }

        match self.ids.iter().position(|held| held == id) {
            Some(at) => drop(self.ids.remove(at)),
            None => self.ids.push(id.to_string()),
        }
    }

    fn push(&mut self, id: String) {
        self.ids.retain(|held| *held != id);
        self.ids.push(id);
    }
}

// ── A press ──

/// What a plain press on the stage does, decided before anything changes. A
/// drag from the press moves what's selected; the press only steps the
/// selection when it lets go without moving.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Press {
    /// What a drag from here moves, with the rest of a set riding along.
    /// None when that is locked.
    pub drags: Option<String>,
    /// Selected at the press: a press on nothing selected picks at once.
    pub now: Option<String>,
    /// Selected on a release that didn't move.
    pub on_release: Option<String>,
}

/// A plain press on `chain`, the ids under the pointer outermost first.
/// `again` is a press on the same spot and chain as the last one, which
/// steps one group in from the selected level, and round to the outermost
/// from the innermost.
pub(crate) fn press(
    selection: &Selection,
    chain: &[String],
    again: bool,
    locked: impl Fn(&str) -> bool,
) -> Press {
    let deeper = again
        .then(|| {
            let at = chain
                .iter()
                .position(|id| Some(id.as_str()) == selection.primary())?;
            chain.get(at + 1)
        })
        .flatten();
    let step = deeper.unwrap_or(&chain[0]).clone();
    let unlocked = |id: &String| (!locked(id)).then(|| id.clone());

    // One selection lives in one parent, so at most one id along the chain
    // is held.
    match chain.iter().find(|id| selection.contains(id)) {
        Some(held) => Press {
            drags: unlocked(held),
            now: None,
            // Letting go on one of a set leaves just that one.
            on_release: Some(match selection.several() {
                true => held.clone(),
                false => step,
            }),
        },

        None => Press {
            drags: unlocked(&step),
            now: Some(step),
            on_release: None,
        },
    }
}

// ── Parents ──

/// The groups holding `id`, outermost first: empty at the top level.
pub(crate) fn parent_path(items: &[CanvasItem], id: &str) -> Option<Vec<String>> {
    let mut path = edit::path_to(items, id)?;
    path.pop();
    Some(path)
}

/// Whether `a` and `b` sit in the same list.
pub(crate) fn same_parent(items: &[CanvasItem], a: &str, b: &str) -> bool {
    match (parent_path(items, a), parent_path(items, b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The list the groups along `path` lead to: the top level for an empty one.
pub(crate) fn children_at<'a>(
    items: &'a [CanvasItem],
    path: &[String],
) -> Option<&'a [CanvasItem]> {
    let mut list = items;
    for id in path {
        let group = list.iter().find(|item| &item.id == id)?;
        let Content::Group { items } = &group.content else {
            return None;
        };
        list = items;
    }

    Some(list)
}

/// Where the set's parent is: the primary's parent, or the top level with
/// nothing selected.
pub(crate) fn scope(items: &[CanvasItem], selection: &Selection) -> Vec<String> {
    selection
        .primary()
        .and_then(|id| parent_path(items, id))
        .unwrap_or_default()
}

/// What a click on tree row `id` leaves selected. Shift takes the run of
/// siblings from the primary to `id`, keeping the primary where it was so
/// the next shift-click runs from it again. Ctrl or Cmd toggles, and both
/// add the run to what's held. A row under another parent starts afresh.
pub(crate) fn tree_click(
    items: &[CanvasItem],
    selection: &Selection,
    id: &str,
    shift: bool,
    toggle: bool,
) -> Selection {
    let mut out = selection.clone();
    let anchor = selection
        .primary()
        .filter(|primary| same_parent(items, primary, id));

    match (shift, anchor) {
        (true, Some(anchor)) => {
            let siblings = edit::siblings(items, id).unwrap_or_default();
            let at = |want: &str| siblings.iter().position(|item| item.id == want);
            let (Some(from), Some(to)) = (at(anchor), at(id)) else {
                return out;
            };

            let (lo, hi) = (from.min(to), from.max(to));
            let mut run: Vec<String> = siblings[lo..=hi]
                .iter()
                .map(|item| item.id.clone())
                .filter(|run_id| run_id != anchor)
                .collect();
            run.push(anchor.to_string());

            if toggle {
                let mut held = selection.ids.clone();
                held.retain(|held| !run.contains(held));
                held.extend(run);
                out.replace(held);
            } else {
                out.replace(run);
            }
        }

        (true, None) => out.set(Some(id.to_string())),

        (false, _) if toggle => out.toggle(id, anchor.is_some() || selection.is_empty()),

        (false, _) => out.set(Some(id.to_string())),
    }

    out
}

// ── The marquee ──

/// Whether a quad, corners in order, touches a screen box: left, top,
/// right, bottom. Separating axes, so a tilted item counts only where its
/// real quad reaches, not its bounding box. An edge on the box's edge
/// touches.
pub(crate) fn touches(quad: &[[f32; 2]; 4], rect: [f32; 4]) -> bool {
    let [left, top, right, bottom] = rect;
    let corners = [[left, top], [right, top], [right, bottom], [left, bottom]];

    let apart = |axis: [f32; 2]| {
        let span = |points: &[[f32; 2]]| {
            points
                .iter()
                .map(|p| p[0] * axis[0] + p[1] * axis[1])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), d| {
                    (lo.min(d), hi.max(d))
                })
        };
        let (a_lo, a_hi) = span(quad);
        let (b_lo, b_hi) = span(&corners);
        a_hi < b_lo || b_hi < a_lo
    };

    if apart([1., 0.]) || apart([0., 1.]) {
        return false;
    }

    // A quad's edge normals. A degenerate edge gives a zero axis, which
    // separates nothing.
    (0..4).all(|i| {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        !apart([a[1] - b[1], b[0] - a[0]])
    })
}

/// The box two corners of a drag span: left, top, right, bottom.
pub(crate) fn rect_between(a: [f32; 2], b: [f32; 2]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[0].max(b[0]),
        a[1].max(b[1]),
    ]
}

// ── Align and distribute ──

/// One item of a set, as the set tools see it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Member {
    pub place: Place,
    /// Where its stack puts it and how big, if it's in one.
    pub cell: Option<StackCell>,
    /// A flat item draws without its scale.
    pub flat: bool,
}

/// Each member moved so its box lines up with `line` of the set's bounds:
/// the left edges on the leftmost, the middles on the middle of the span,
/// and so on. The box is the one the single-item align uses, before any
/// turn. Only positions move, each axis in its own unit. None when a member
/// has no area or the parent has none.
pub(crate) fn aligned(members: &[Member], parent: [f32; 2], line: Line) -> Option<Vec<Place>> {
    let boxes = boxes(members, parent)?;
    let (axis, end) = line.axis_and_end();

    let lo = boxes.iter().map(|b| b[axis]).fold(f32::INFINITY, f32::min);
    let hi = boxes
        .iter()
        .map(|b| b[axis] + b[axis + 2])
        .fold(f32::NEG_INFINITY, f32::max);

    let out = members
        .iter()
        .zip(&boxes)
        .map(|(member, b)| {
            let want = lo + end * (hi - lo - b[axis + 2]);
            shifted(member.place, parent, axis, want - b[axis])
        })
        .collect();

    Some(out)
}

/// Each member moved along `axis` so the gaps between neighbours are
/// equal, the two outermost staying put. Neighbours are taken in the order
/// of their middles. By gaps, not by middles: items of different sizes read
/// as evenly spaced when the space between them is even. None under three
/// members, or when one has no area.
pub(crate) fn distributed(members: &[Member], parent: [f32; 2], axis: usize) -> Option<Vec<Place>> {
    if members.len() < 3 {
        return None;
    }
    let boxes = boxes(members, parent)?;

    let middle = |b: &[f32; 4]| b[axis] + b[axis + 2] / 2.;
    let mut order: Vec<usize> = (0..members.len()).collect();
    order.sort_by(|&a, &b| middle(&boxes[a]).total_cmp(&middle(&boxes[b])));

    let first = &boxes[order[0]];
    let last = &boxes[order[order.len() - 1]];
    let start = first[axis];
    let end = last[axis] + last[axis + 2];
    let lengths: f32 = boxes.iter().map(|b| b[axis + 2]).sum();
    let gap = (end - start - lengths) / (members.len() - 1) as f32;

    let mut out: Vec<Place> = members.iter().map(|member| member.place).collect();
    let mut at = start;
    for ix in order {
        let b = &boxes[ix];
        out[ix] = shifted(members[ix].place, parent, axis, at - b[axis]);
        at += b[axis + 2] + gap;
    }

    Some(out)
}

/// Every member's box in the parent's units: left, top, width, height.
fn boxes(members: &[Member], parent: [f32; 2]) -> Option<Vec<[f32; 4]>> {
    let area = |w: f32, h: f32| w > 0. && h > 0. && w.is_finite() && h.is_finite();
    if !area(parent[0], parent[1]) {
        return None;
    }

    members
        .iter()
        .map(|member| {
            let b = align::drawn_box(&member.place, parent, member.cell, member.flat);
            area(b[2], b[3]).then_some(b)
        })
        .collect()
}

fn shifted(place: Place, parent: [f32; 2], axis: usize, by: f32) -> Place {
    let mut delta = [0.; 2];
    delta[axis] = by;

    let mut out = place;
    out.position = place.position.shifted(delta, parent);
    out
}

/// The bounds of several screen quads as corners, clockwise from the
/// top-left. None for no quads.
pub(crate) fn around(quads: &[[[f32; 2]; 4]]) -> Option<[[f32; 2]; 4]> {
    let mut points = quads.iter().flatten();
    let first = *points.next()?;

    let (mut min, mut max) = (first, first);
    for [x, y] in points {
        min = [min[0].min(*x), min[1].min(*y)];
        max = [max[0].max(*x), max[1].max(*y)];
    }

    Some([min, [max[0], min[1]], max, [min[0], max[1]]])
}

// ── Order ──

/// Which way an order tool moves a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// One unpicked sibling further forward.
    Forward,
    Backward,
    /// In front of every unpicked sibling.
    Front,
    Back,
}

/// The new depth of each picked sibling that has to move for `order`, as
/// sibling index and depth. The picked ones keep their order among
/// themselves, and the unpicked ones keep theirs and their depths. Empty
/// when nothing moves.
pub(crate) fn reordered(depths: &[f32], picked: &[bool], order: Order) -> Vec<(usize, f32)> {
    let before = paint_order(depths);
    let after = permuted(&before, picked, order);
    if after == before {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut floor: Option<f32> = None;
    let mut i = 0;
    while i < after.len() {
        if !picked[after[i]] {
            floor = Some(depths[after[i]]);
            i += 1;
            continue;
        }

        let start = i;
        while i < after.len() && picked[after[i]] {
            i += 1;
        }
        let run = &after[start..i];
        let ceil = after.get(i).map(|&ix| depths[ix]);

        // Each one already between the last and the next unpicked sibling
        // keeps its depth. The rest share out the room that's left.
        let mut last = floor;
        for (k, &ix) in run.iter().enumerate() {
            let depth = depths[ix];
            if last.is_none_or(|lo| depth > lo) && ceil.is_none_or(|hi| depth < hi) {
                last = Some(depth);
                continue;
            }

            let left = (run.len() - k) as f32;
            let placed = match (last, ceil) {
                (Some(lo), Some(hi)) if hi > lo => lo + (hi - lo) / (left + 1.),
                // A tie leaves no room between, so the run steps past it,
                // as the single-item tools do.
                (Some(lo), _) => lo + edit::DEPTH_STEP,
                (None, Some(hi)) => hi - edit::DEPTH_STEP * left,
                (None, None) => depth,
            };
            out.push((ix, placed));
            last = Some(placed);
        }
    }

    out
}

/// The paint order after `order`, back to front.
fn permuted(before: &[usize], picked: &[bool], order: Order) -> Vec<usize> {
    let mut out = before.to_vec();
    let is = |ix: usize| picked.get(ix).copied().unwrap_or(false);

    match order {
        // From the front back, so a run of picked items passes its
        // neighbour together.
        Order::Forward => {
            for i in (0..out.len().saturating_sub(1)).rev() {
                if is(out[i]) && !is(out[i + 1]) {
                    out.swap(i, i + 1);
                }
            }
        }

        Order::Backward => {
            for i in 1..out.len() {
                if is(out[i]) && !is(out[i - 1]) {
                    out.swap(i, i - 1);
                }
            }
        }

        Order::Front => {
            let (picked, rest): (Vec<usize>, Vec<usize>) = out.iter().partition(|&&ix| is(ix));
            out = rest.into_iter().chain(picked).collect();
        }

        Order::Back => {
            let (picked, rest): (Vec<usize>, Vec<usize>) = out.iter().partition(|&&ix| is(ix));
            out = picked.into_iter().chain(rest).collect();
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use rox_canvas::{Anchor, Length, Position};

    use super::*;

    fn ids(selection: &Selection) -> Vec<&str> {
        selection.ids().iter().map(String::as_str).collect()
    }

    fn leaf(id: &str) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            ..CanvasItem::default()
        }
    }

    fn group(id: &str, items: Vec<CanvasItem>) -> CanvasItem {
        CanvasItem {
            id: id.into(),
            content: Content::Group { items },
            ..CanvasItem::default()
        }
    }

    /// a, b, g holding c and d, e.
    fn tree() -> Vec<CanvasItem> {
        vec![
            leaf("a"),
            leaf("b"),
            group("g", vec![leaf("c"), leaf("d")]),
            leaf("e"),
        ]
    }

    fn one(id: &str) -> Selection {
        let mut selection = Selection::default();
        selection.set(Some(id.into()));
        selection
    }

    #[test]
    fn the_last_pick_is_the_primary_and_a_toggle_keeps_one_parent() {
        let mut selection = one("a");
        selection.toggle("b", true);
        assert_eq!(ids(&selection), ["a", "b"]);
        assert_eq!(selection.primary(), Some("b"));
        assert!(selection.several());

        selection.toggle("b", true);
        assert_eq!(ids(&selection), ["a"]);

        // Under another parent it starts again.
        selection.toggle("c", false);
        assert_eq!(ids(&selection), ["c"]);

        selection.replace(vec!["a".into(), "b".into(), "a".into()]);
        assert_eq!(ids(&selection), ["b", "a"]);
        selection.raise("b");
        assert_eq!(selection.primary(), Some("b"));
    }

    fn chain(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn pressed(
        selection: &Selection,
        ids: &[&str],
        again: bool,
        locked: &[&str],
    ) -> (Option<String>, Option<String>, Option<String>) {
        let out = press(selection, &chain(ids), again, |id| locked.contains(&id));
        (out.drags, out.now, out.on_release)
    }

    fn some(id: &str) -> Option<String> {
        Some(id.to_string())
    }

    #[test]
    fn a_press_on_nothing_selected_picks_the_outermost_and_drags_it() {
        let none = Selection::default();
        assert_eq!(
            pressed(&none, &["a"], false, &[]),
            (some("a"), some("a"), None)
        );

        // With nothing selected a second press has no level to step from.
        assert_eq!(
            pressed(&none, &["g", "c"], true, &[]),
            (some("g"), some("g"), None)
        );

        // Another item than the one selected: a fresh pick.
        assert_eq!(
            pressed(&one("c"), &["g", "d"], true, &[]),
            (some("g"), some("g"), None)
        );
    }

    #[test]
    fn a_press_on_the_selection_drags_it_and_steps_only_on_release() {
        // A selected top-level item.
        assert_eq!(
            pressed(&one("a"), &["a"], true, &[]),
            (some("a"), None, some("a"))
        );

        // A child of the selected group: the drag moves the group, a click
        // steps in.
        assert_eq!(
            pressed(&one("g"), &["g", "c"], true, &[]),
            (some("g"), None, some("c"))
        );
        assert_eq!(
            pressed(&one("g"), &["g", "c"], false, &[]),
            (some("g"), None, some("g")),
            "a press elsewhere on it starts from the outermost again"
        );

        // A selected nested child: the drag moves the child, and only a
        // click goes round to the outermost.
        assert_eq!(
            pressed(&one("c"), &["g", "c"], true, &[]),
            (some("c"), None, some("g"))
        );
    }

    #[test]
    fn a_press_on_a_stacked_child_drags_the_child_once_it_is_selected() {
        let deck = ["deck", "card"];
        assert_eq!(
            pressed(&Selection::default(), &deck, false, &[]),
            (some("deck"), some("deck"), None)
        );
        assert_eq!(
            pressed(&one("deck"), &deck, true, &[]),
            (some("deck"), None, some("card"))
        );
        assert_eq!(
            pressed(&one("card"), &deck, true, &[]),
            (some("card"), None, some("deck"))
        );
    }

    #[test]
    fn a_press_on_one_of_a_set_drags_the_set_and_a_click_leaves_that_one() {
        let mut set = one("a");
        set.toggle("b", true);
        assert_eq!(
            pressed(&set, &["a"], true, &[]),
            (some("a"), None, some("a"))
        );

        // A press inside a group of the set takes the group.
        let mut set = one("a");
        set.toggle("g", true);
        assert_eq!(
            pressed(&set, &["g", "c"], true, &[]),
            (some("g"), None, some("g"))
        );

        // Off the set a plain press starts again.
        assert_eq!(
            pressed(&set, &["e"], false, &[]),
            (some("e"), some("e"), None)
        );
    }

    #[test]
    fn a_locked_item_is_picked_and_stepped_but_never_dragged() {
        assert_eq!(
            pressed(&Selection::default(), &["a"], false, &["a"]),
            (None, some("a"), None)
        );
        assert_eq!(
            pressed(&one("g"), &["g", "c"], true, &["g"]),
            (None, None, some("c"))
        );

        // A locked child doesn't hold its unlocked group still.
        assert_eq!(
            pressed(&one("g"), &["g", "c"], true, &["c"]),
            (some("g"), None, some("c"))
        );
    }

    #[test]
    fn parents_are_read_off_the_tree() {
        let items = tree();
        assert_eq!(parent_path(&items, "a"), Some(Vec::new()));
        assert_eq!(parent_path(&items, "d"), Some(vec!["g".to_string()]));
        assert!(same_parent(&items, "a", "e"));
        assert!(same_parent(&items, "c", "d"));
        assert!(!same_parent(&items, "a", "c"));
        assert!(!same_parent(&items, "a", "nowhere"));

        let inside = children_at(&items, &["g".to_string()]).unwrap();
        assert_eq!(inside.len(), 2);
        assert_eq!(children_at(&items, &[]).map(<[_]>::len), Some(4));
        assert!(children_at(&items, &["a".to_string()]).is_none());

        assert_eq!(scope(&items, &one("c")), ["g"]);
        assert!(scope(&items, &Selection::default()).is_empty());
    }

    #[test]
    fn a_tree_click_takes_a_run_toggles_or_starts_again() {
        let items = tree();

        let run = tree_click(&items, &one("a"), "e", true, false);
        assert_eq!(ids(&run), ["b", "g", "e", "a"], "the anchor stays primary");

        // Shift again runs from the same anchor.
        let shorter = tree_click(&items, &run, "b", true, false);
        assert_eq!(ids(&shorter), ["b", "a"]);

        let toggled = tree_click(&items, &one("a"), "e", false, true);
        assert_eq!(ids(&toggled), ["a", "e"]);
        let off = tree_click(&items, &toggled, "a", false, true);
        assert_eq!(ids(&off), ["e"]);

        // Both add the run to what's held.
        let held = tree_click(&items, &toggled, "b", true, true);
        assert_eq!(ids(&held), ["a", "b", "g", "e"]);

        // Another parent: a plain start, whichever modifier.
        assert_eq!(ids(&tree_click(&items, &one("a"), "c", true, false)), ["c"]);
        assert_eq!(ids(&tree_click(&items, &one("a"), "c", false, true)), ["c"]);
        assert_eq!(ids(&tree_click(&items, &run, "d", false, false)), ["d"]);
    }

    #[test]
    fn a_marquee_touches_by_the_real_quad() {
        let square = [[0., 0.], [10., 0.], [10., 10.], [0., 10.]];
        assert!(touches(&square, [5., 5., 20., 20.]));
        assert!(touches(&square, [10., 10., 20., 20.]), "a shared corner");
        assert!(!touches(&square, [11., 0., 20., 20.]));
        assert!(touches(&square, [-5., -5., 50., 50.]), "enclosed");
        assert!(touches(&square, [2., 2., 3., 3.]), "inside it");

        // A diamond: its bounding box reaches the box's corner, it doesn't.
        let diamond = [[10., 0.], [20., 10.], [10., 20.], [0., 10.]];
        assert!(!touches(&diamond, [0., 0., 3., 3.]));
        assert!(touches(&diamond, [0., 0., 6., 6.]));

        // A tilted quad as a perspective item projects.
        let tilted = [[100., 40.], [180., 60.], [180., 140.], [100., 160.]];
        assert!(!touches(&tilted, [170., 40., 200., 55.]));
        assert!(touches(&tilted, [170., 40., 200., 70.]));

        assert_eq!(rect_between([5., 9.], [1., 2.]), [1., 2., 5., 9.]);
    }

    const PARENT: [f32; 2] = [1000., 500.];

    fn member(anchor: Anchor, x: Length, y: f32, size: [f32; 2]) -> Member {
        Member {
            place: Place {
                anchor,
                position: Position {
                    xy: [x, Length::Units(y)],
                    z: 3.,
                },
                size: [Length::Units(size[0]), Length::Units(size[1])],
                ..Place::default()
            },
            cell: None,
            flat: true,
        }
    }

    fn lefts(places: &[Place], flat: bool) -> Vec<f32> {
        places
            .iter()
            .map(|place| align::drawn_box(place, PARENT, None, flat)[0])
            .collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    #[test]
    fn align_lines_up_on_the_sets_bounds_in_each_items_unit() {
        let set = [
            member(Anchor::TopLeft, Length::Units(100.), 0., [50., 50.]),
            member(Anchor::Center, Length::Stage(0.1), 30., [200., 40.]),
            member(Anchor::BottomRight, Length::Units(-300.), -10., [80., 20.]),
        ];
        let start = lefts(&set.map(|m| m.place), true);
        assert!(close(&start, &[100., 500., 620.]), "{start:?}");

        let left = aligned(&set, PARENT, Line::Left).unwrap();
        assert!(close(&lefts(&left, true), &[100., 100., 100.]));
        assert!(
            matches!(left[1].position.xy[0], Length::Stage(f) if (f - -0.3).abs() < 1e-5),
            "a fractional x stays a fraction: {:?}",
            left[1].position.xy[0]
        );
        assert_eq!(left[2].position.xy[1], Length::Units(-10.), "y stays");
        assert!(left.iter().all(|place| place.position.z == 3.));

        // The span runs 100 to 700.
        let right = aligned(&set, PARENT, Line::Right).unwrap();
        assert!(close(&lefts(&right, true), &[650., 500., 620.]));

        let middle = aligned(&set, PARENT, Line::CenterX).unwrap();
        assert!(close(&lefts(&middle, true), &[375., 300., 360.]));

        let top = aligned(&set, PARENT, Line::Top).unwrap();
        let tops: Vec<f32> = top
            .iter()
            .map(|place| align::drawn_box(place, PARENT, None, true)[1])
            .collect();
        assert!(close(&tops, &[0., 0., 0.]), "{tops:?}");
    }

    #[test]
    fn align_reads_a_scaled_items_drawn_box() {
        let mut big = member(Anchor::TopLeft, Length::Units(300.), 0., [100., 100.]);
        big.place.scale = 2.;
        big.flat = false;
        let small = member(Anchor::TopLeft, Length::Units(0.), 0., [10., 10.]);

        // It draws 200 wide about its middle, from 250.
        let out = aligned(&[big, small], PARENT, Line::Left).unwrap();
        assert!(close(
            &[align::drawn_box(&out[0], PARENT, None, false)[0]],
            &[0.]
        ));
        assert!(close(&lefts(&out[1..], true), &[0.]));
    }

    #[test]
    fn distribute_evens_the_gaps_and_keeps_the_outermost() {
        // Five of different widths, given out of order.
        let set = [
            member(Anchor::TopLeft, Length::Units(900.), 0., [100., 10.]),
            member(Anchor::TopLeft, Length::Units(0.), 0., [50., 10.]),
            member(Anchor::TopLeft, Length::Stage(0.2), 0., [150., 10.]),
            member(Anchor::TopLeft, Length::Units(410.), 0., [20., 10.]),
            member(Anchor::TopLeft, Length::Units(600.), 0., [80., 10.]),
        ];

        let out = distributed(&set, PARENT, 0).unwrap();
        let boxes: Vec<[f32; 4]> = out
            .iter()
            .map(|place| align::drawn_box(place, PARENT, None, true))
            .collect();

        // In order of their middles: 0, 2, 3, 4, then 900.
        let order = [1, 2, 3, 4, 0];
        assert!(close(
            &[boxes[1][0], boxes[0][0] + boxes[0][2]],
            &[0., 1000.]
        ));
        let gaps: Vec<f32> = order
            .windows(2)
            .map(|w| boxes[w[1]][0] - (boxes[w[0]][0] + boxes[w[0]][2]))
            .collect();
        assert!(close(&gaps, &[150.; 4]), "{gaps:?}");
        assert!(matches!(out[2].position.xy[0], Length::Stage(_)));

        assert!(distributed(&set[..2], PARENT, 0).is_none());
        assert!(distributed(&set, [0., 500.], 0).is_none());
    }

    #[test]
    fn the_set_box_holds_every_quad() {
        let a = [[0., 0.], [10., 0.], [10., 10.], [0., 10.]];
        let b = [[30., -5.], [40., 0.], [35., 20.], [25., 15.]];
        assert_eq!(
            around(&[a, b]),
            Some([[0., -5.], [40., -5.], [40., 20.], [0., 20.]])
        );
        assert_eq!(around(&[]), None);
    }

    /// The paint order once `moved` is applied.
    fn applied(depths: &[f32], moved: &[(usize, f32)]) -> Vec<usize> {
        let mut depths = depths.to_vec();
        for &(ix, depth) in moved {
            depths[ix] = depth;
        }
        paint_order(&depths)
    }

    #[test]
    fn order_tools_carry_a_set_and_keep_its_order() {
        let depths = [0., 10., 20., 30., 40.];
        let picked = [true, false, true, false, false];

        let forward = reordered(&depths, &picked, Order::Forward);
        assert_eq!(applied(&depths, &forward), [1, 0, 3, 2, 4]);

        let front = reordered(&depths, &picked, Order::Front);
        assert_eq!(applied(&depths, &front), [1, 3, 4, 0, 2]);

        let back = reordered(&depths, &[false, false, true, false, true], Order::Back);
        assert_eq!(applied(&depths, &back), [2, 4, 0, 1, 3]);

        let backward = reordered(&depths, &[false, true, true, false, false], Order::Backward);
        assert_eq!(applied(&depths, &backward), [1, 2, 0, 3, 4]);

        // A run passes its neighbour together.
        let run = reordered(&depths, &[true, true, false, false, false], Order::Forward);
        assert_eq!(applied(&depths, &run), [2, 0, 1, 3, 4]);
    }

    #[test]
    fn order_tools_leave_what_doesnt_move() {
        let depths = [0., 10., 20., 30.];

        // Already at the front: nothing.
        let front = [false, false, true, true];
        assert!(reordered(&depths, &front, Order::Front).is_empty());
        assert!(reordered(&depths, &front, Order::Forward).is_empty());

        // The back one is already there, so only the other moves.
        let moved = reordered(&depths, &[true, false, true, false], Order::Backward);
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert_eq!(applied(&depths, &moved), [0, 2, 1, 3]);

        // Siblings all at one depth still come apart in the right order.
        let flat = [0.; 4];
        let moved = reordered(&flat, &[true, true, false, false], Order::Front);
        assert_eq!(applied(&flat, &moved), [2, 3, 0, 1]);
    }
}
