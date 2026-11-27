//! A group that lays its children out one after another along x or y, with
//! a gap, the way a column of a cover and two lines of text sits centred in
//! whatever space a window leaves.
//!
//! The stack decides where each child's box goes: along the axis in list
//! order, and across it by `align` or the child's own `align_self`. Each
//! child keeps its own size, limits and aspect, measured against the box
//! inside the group's padding as anywhere else, so a fraction along the axis
//! is a fraction of that box and not of what's left. A child's margins add
//! to the gap on their side. A run longer than the box runs past it, since
//! nothing shrinks.
//!
//! A child with `grow` takes a share of the room left along the axis once
//! the fixed lengths, margins and gaps are counted, by its weight among the
//! others that grow. Its own size is the base it grows from, and its limits
//! still hold: one that stops at its maximum, or a locked shape that runs
//! out of room across, hands the rest to the others, and whatever none of
//! them takes goes to `justify`. Nothing grows while the run already
//! overflows.
//!
//! A child's own x and y then move it from where the stack put it, as do
//! its routes and sway. Depth, turns and scale are left alone, since the
//! stack lays out the flat boxes before anything is projected; like a CSS
//! transform, a child's turn and scale don't take room.
//!
//! Each child takes room by its presence, 0 to 1: a hidden one none, its
//! margins included, and one easing in or out a share, so its neighbours
//! close up as it fades.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::mat::Mat4;
use crate::place::fitted;
use crate::project::Frame;
use crate::state::CanvasItem;

/// A grown box that ends this close to what it was offered took all of it.
const SLACK: f32 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stack {
    pub axis: StackAxis,
    /// Units between one child and the next.
    pub gap: f32,
    /// Where each child sits across the axis.
    pub align: StackAlign,
    /// Where the run of children sits along the axis, or `between` to share
    /// the room left over out between them.
    pub justify: StackJustify,
    /// Units the children keep in from the group's box: left, top, right,
    /// bottom.
    #[serde(skip_serializing_if = "no_padding")]
    pub padding: [f32; 4],
    /// Runs the children from the far end: the first one sits there, and
    /// `justify`'s start is that end.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reverse: bool,
}

impl Default for Stack {
    fn default() -> Self {
        Stack {
            axis: StackAxis::Y,
            gap: 0.,
            align: StackAlign::Start,
            justify: StackJustify::Start,
            padding: [0.; 4],
            reverse: false,
        }
    }
}

impl Stack {
    /// The padding as it lays out: a negative or broken side reads as none.
    pub fn pads(&self) -> [f32; 4] {
        self.padding
            .map(|p| if p.is_finite() { p.max(0.) } else { 0. })
    }

    /// The box inside the padding of a group box of `group`, which the
    /// children are laid out in and measured against.
    pub fn inner(&self, group: [f32; 2]) -> [f32; 2] {
        let [left, top, right, bottom] = self.pads();
        [
            (group[0] - left - right).max(0.),
            (group[1] - top - bottom).max(0.),
        ]
    }

    /// The frame a group hands its children, moved in to the box inside the
    /// padding.
    pub fn inner_frame(&self, frame: &Frame) -> Frame {
        let [left, top, ..] = self.pads();
        Frame {
            transform: frame.transform * Mat4::translate([left, top, 0.]),
            size: self.inner(frame.size),
            ..*frame
        }
    }
}

fn no_padding(padding: &[f32; 4]) -> bool {
    *padding == [0.; 4]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackAxis {
    X,
    #[default]
    Y,
}

impl StackAxis {
    /// The index of the axis the children run along, 0 for x.
    pub fn along(self) -> usize {
        match self {
            StackAxis::X => 0,
            StackAxis::Y => 1,
        }
    }

    pub fn across(self) -> usize {
        1 - self.along()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackAlign {
    #[default]
    Start,
    Center,
    End,
}

impl StackAlign {
    fn share(self) -> f32 {
        match self {
            StackAlign::Start => 0.,
            StackAlign::Center => 0.5,
            StackAlign::End => 1.,
        }
    }
}

/// Where the run sits along the axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackJustify {
    #[default]
    Start,
    Center,
    End,
    /// The room left over shared out into the gaps, the first child at the
    /// start and the last at the end. With one child, or none left over,
    /// it reads as start.
    Between,
}

impl StackJustify {
    /// The share of the room left over that goes ahead of the run.
    fn share(self) -> f32 {
        match self {
            StackJustify::Start | StackJustify::Between => 0.,
            StackJustify::Center => 0.5,
            StackJustify::End => 1.,
        }
    }
}

/// Where the stack puts one child's box: its top-left in the units of the
/// box inside the padding, and its size, its own or what it grew to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackCell {
    pub at: [f32; 2],
    pub size: [f32; 2],
}

/// A child as the stack sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// Its box as its own sizes, limits and aspect give it: what it grows
    /// from.
    pub size: [f32; 2],
    /// How much room it takes, 0 to 1.
    pub presence: f32,
    /// Its weight in the room left along the axis, 0 for none.
    pub grow: f32,
    /// Units before and after it along the axis, on top of the gap.
    pub margin: [f32; 2],
    /// Where it sits across, in place of the stack's align.
    pub align: Option<StackAlign>,
    /// The box its sizes and limits give before its aspect, which a locked
    /// shape that grows fits inside across the axis.
    pub space: [f32; 2],
    /// The longest it grows to along the axis.
    pub most: f32,
    pub aspect: Option<f32>,
}

impl Entry {
    /// A child of a fixed size that takes all its room and nothing more.
    pub fn sized(size: [f32; 2]) -> Entry {
        Entry {
            size,
            presence: 1.,
            grow: 0.,
            margin: [0.; 2],
            align: None,
            space: size,
            most: f32::INFINITY,
            aspect: None,
        }
    }

    /// Its box when offered `length` along `along`: never under its own size
    /// nor past its maximum, and for a locked shape the largest of it inside
    /// the length and its space across.
    fn grown(&self, along: usize, length: f32) -> [f32; 2] {
        let length = length.min(self.most).max(self.size[along]);
        let Some(aspect) = self.aspect else {
            let mut size = self.size;
            size[along] = length;
            return size;
        };

        let mut space = self.space;
        space[along] = length;
        if !(space[0] > 0. && space[1] > 0.) {
            return self.size;
        }

        let fit = fitted(space, aspect);
        if fit[along] < self.size[along] {
            self.size
        } else {
            fit
        }
    }
}

/// Each entry's cell, and how long the run is along the axis, margins and
/// gaps included, which overflows the box inside the padding when it's
/// longer.
///
/// An entry easing in or out keeps its size and sits in the middle of the
/// share of room it has, so it shrinks away in place. A gap counts before
/// each entry after the first, by that entry's presence, and only once
/// something is showing ahead of it.
pub fn cells(stack: &Stack, group: [f32; 2], entries: &[Entry]) -> (Vec<StackCell>, f32) {
    let along = stack.axis.along();
    let across = stack.axis.across();
    let inner = stack.inner(group);
    let gap = finite(stack.gap);

    let presences: Vec<f32> = entries.iter().map(|e| presence(e.presence)).collect();
    let margins: Vec<[f32; 2]> = entries.iter().map(|e| e.margin.map(finite)).collect();

    // How much of a gap goes before each entry.
    let mut ahead = 0f32;
    let gap_count: f32 = presences
        .iter()
        .map(|&p| {
            let share = p * ahead.min(1.);
            ahead += p;
            share
        })
        .sum();

    let fixed: f32 = entries
        .iter()
        .zip(&presences)
        .zip(&margins)
        .map(|((entry, p), m)| p * (m[0] + entry.size[along] + m[1]))
        .sum::<f32>()
        + gap * gap_count;

    let mut sizes: Vec<[f32; 2]> = entries.iter().map(|e| e.size).collect();
    if inner[along] - fixed > 0. {
        grow(entries, &presences, along, inner[along] - fixed, &mut sizes);
    }

    // Laid out from zero first, since justify needs the run's length. The
    // sums go in the order the stack has always added them, so a stack with
    // no margins, growth or spread lands on exactly the same numbers.
    let lay = |spread: f32| {
        let mut starts = Vec::with_capacity(entries.len());
        let mut cursor = 0f32;
        let mut ahead = 0f32;
        for (ix, size) in sizes.iter().enumerate() {
            let (p, m) = (presences[ix], margins[ix]);
            cursor += gap * p * ahead.min(1.);
            if spread != 0. {
                cursor += spread * p * ahead.min(1.);
            }
            cursor += m[0] * p;

            starts.push(cursor + (p - 1.) * size[along] / 2.);

            cursor += size[along] * p;
            cursor += m[1] * p;
            ahead += p;
        }
        (starts, cursor)
    };

    // Growth takes the room first, so `between` shares only what it leaves.
    let (mut starts, mut run) = lay(0.);
    let left = inner[along] - run;
    let lead = match stack.justify {
        StackJustify::Between => {
            if left > 0. && gap_count > 0. {
                (starts, run) = lay(left / gap_count);
            }
            0.
        }
        justify => justify.share() * left,
    };

    let mut cells: Vec<StackCell> = entries
        .iter()
        .zip(starts)
        .zip(&sizes)
        .map(|((entry, start), &size)| {
            let mut at = [0.; 2];
            at[along] = lead + start;
            let align = entry.align.unwrap_or(stack.align);
            at[across] = align.share() * (inner[across] - size[across]);
            StackCell { at, size }
        })
        .collect();

    if stack.reverse {
        for cell in &mut cells {
            cell.at[along] = inner[along] - cell.at[along] - cell.size[along];
        }
    }

    (cells, run)
}

/// Shares `room` units along the axis among the entries that grow, by
/// weight, writing each grown box into `sizes`. One that stops short of its
/// offer, at its maximum or out of room across for its shape, keeps what it
/// reached and the rest goes round the others again.
fn grow(entries: &[Entry], presences: &[f32], along: usize, room: f32, sizes: &mut [[f32; 2]]) {
    let weight = |ix: usize| {
        let grow = entries[ix].grow;
        if grow.is_finite() && grow > 0. {
            grow
        } else {
            0.
        }
    };
    let taken = |ix: usize, size: [f32; 2]| presences[ix] * (size[along] - entries[ix].size[along]);

    let mut open: Vec<usize> = (0..entries.len())
        .filter(|&ix| presences[ix] > 0. && weight(ix) > 0.)
        .collect();
    let mut pool = room;

    while !open.is_empty() && pool > 0. {
        let total: f32 = open.iter().map(|&ix| presences[ix] * weight(ix)).sum();
        if total <= 0. {
            break;
        }

        // A share past the whole pool would only matter while every grower
        // is mostly faded, where it would run off without end.
        let offers: Vec<(usize, f32, [f32; 2])> = open
            .iter()
            .map(|&ix| {
                let offer = entries[ix].size[along] + (pool * weight(ix) / total).min(pool);
                (ix, offer, entries[ix].grown(along, offer))
            })
            .collect();

        let short: Vec<&(usize, f32, [f32; 2])> = offers
            .iter()
            .filter(|(_, offer, size)| size[along] < offer - SLACK)
            .collect();
        if short.is_empty() {
            for &(ix, _, size) in &offers {
                sizes[ix] = size;
            }
            break;
        }

        for &&(ix, _, size) in &short {
            sizes[ix] = size;
            pool -= taken(ix, size);
            open.retain(|&other| other != ix);
        }
    }
}

/// The stack's entries for `children` in a group box of `group`, each sized
/// as it lays out in the box inside the padding, with `presence` saying how
/// much room each takes.
pub fn entries(
    stack: &Stack,
    children: &[CanvasItem],
    group: [f32; 2],
    mut presence: impl FnMut(&CanvasItem) -> f32,
) -> Vec<Entry> {
    let inner = stack.inner(group);
    let along = stack.axis.along();

    children
        .iter()
        .map(|child| {
            let place = &child.place;
            Entry {
                size: place.size_in(inner),
                presence: presence(child),
                grow: place.grow,
                margin: place.margin,
                align: place.align_self,
                space: place.space_in(inner),
                most: place.max_size[along].unwrap_or(f32::INFINITY),
                aspect: place.aspect(),
            }
        })
        .collect()
}

/// What a child takes in the editor and anywhere no case is read: all of
/// its room while it shows, none while it's switched off.
pub fn shown(child: &CanvasItem) -> f32 {
    if child.visible { 1. } else { 0. }
}

/// Held to 0 to 1, a NaN reading as hidden.
fn presence(p: f32) -> f32 {
    if p.is_nan() { 0. } else { p.clamp(0., 1.) }
}

/// A broken number reads as none.
fn finite(n: f32) -> f32 {
    if n.is_finite() { n } else { 0. }
}

/// A stack that doesn't parse reads as none, so it costs the stack and not
/// the item.
pub(crate) fn lenient_stack<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Stack>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }

    match Stack::deserialize(value) {
        Ok(stack) => Ok(Some(stack)),
        Err(e) => {
            log::warn!("canvas: dropping a stack that doesn't parse: {e}");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::state::{Content, Length, Place};

    fn column(gap: f32, align: StackAlign, justify: StackJustify) -> Stack {
        Stack {
            gap,
            align,
            justify,
            ..Stack::default()
        }
    }

    fn entry(w: f32, h: f32) -> Entry {
        Entry::sized([w, h])
    }

    /// An entry that grows by `weight` from `h` high.
    fn growing(w: f32, h: f32, weight: f32) -> Entry {
        Entry {
            grow: weight,
            ..entry(w, h)
        }
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    fn ats(cells: &[StackCell]) -> Vec<[f32; 2]> {
        cells.iter().map(|cell| cell.at).collect()
    }

    fn sizes(cells: &[StackCell]) -> Vec<[f32; 2]> {
        cells.iter().map(|cell| cell.size).collect()
    }

    #[test]
    fn a_column_runs_down_with_its_gaps_from_the_top() {
        let stack = column(10., StackAlign::Start, StackJustify::Start);
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 50.), entry(200., 20.), entry(50., 30.)],
        );

        assert_eq!(ats(&cells), [[0., 0.], [0., 60.], [0., 90.]]);
        assert_eq!(sizes(&cells), [[100., 50.], [200., 20.], [50., 30.]]);
        assert_eq!(run, 120.);
    }

    #[test]
    fn centred_both_ways_the_block_sits_in_the_middle() {
        let stack = column(10., StackAlign::Center, StackJustify::Center);
        let (cells, run) =
            super::cells(&stack, [400., 600.], &[entry(100., 50.), entry(200., 20.)]);

        // 80 long in 600 leaves 260 above, and each is centred across.
        assert_eq!(run, 80.);
        assert_eq!(ats(&cells), [[150., 260.], [100., 320.]]);
    }

    #[test]
    fn a_row_runs_along_x_and_ends_at_the_far_side() {
        let stack = Stack {
            axis: StackAxis::X,
            gap: 5.,
            align: StackAlign::End,
            justify: StackJustify::End,
            ..Stack::default()
        };
        let (cells, run) = super::cells(&stack, [300., 100.], &[entry(40., 10.), entry(60., 100.)]);

        assert_eq!(run, 105.);
        assert_eq!(ats(&cells), [[195., 90.], [240., 0.]]);
    }

    #[test]
    fn a_hidden_child_takes_no_room_and_no_gap() {
        let stack = column(10., StackAlign::Start, StackJustify::Start);
        let mut hidden = entry(100., 50.);
        hidden.presence = 0.;

        // Hidden first: the next one starts at the top with no gap before it.
        let (cells, run) = super::cells(&stack, [400., 600.], &[hidden, entry(80., 20.)]);
        assert_eq!(cells[1].at, [0., 0.]);
        assert_eq!(run, 20.);

        // Hidden between two: one gap, not two.
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(80., 20.), hidden, entry(80., 20.)],
        );
        assert_eq!(cells[2].at, [0., 30.]);
        assert_eq!(run, 50.);
    }

    #[test]
    fn a_child_easing_out_shrinks_in_place_and_its_neighbours_close_up() {
        let stack = column(10., StackAlign::Start, StackJustify::Start);
        let mut fading = entry(100., 40.);
        fading.presence = 0.5;

        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(80., 20.), fading, entry(80., 20.)],
        );

        // Half its room and half a gap: centred on a 20-long share.
        assert!(near(cells[1].at, [0., 15.]), "{cells:?}");
        assert!(near(cells[2].at, [0., 55.]), "{cells:?}");
        assert_eq!(run, 75.);

        // Junk presence reads as hidden or whole, never past either.
        fading.presence = f32::NAN;
        let (_, run) = super::cells(&stack, [400., 600.], &[entry(80., 20.), fading]);
        assert_eq!(run, 20.);
        fading.presence = 7.;
        let (_, run) = super::cells(&stack, [400., 600.], &[entry(80., 20.), fading]);
        assert_eq!(run, 70.);
    }

    #[test]
    fn a_run_longer_than_its_group_overflows_without_shrinking() {
        let stack = column(0., StackAlign::Start, StackJustify::Center);
        let (cells, run) = super::cells(&stack, [100., 100.], &[entry(10., 80.), entry(10., 80.)]);

        // 160 in 100 overflows 30 at each end.
        assert_eq!(run, 160.);
        assert_eq!(ats(&cells), [[0., -30.], [0., 50.]]);
    }

    #[test]
    fn one_growing_child_takes_all_the_room_left() {
        let stack = column(10., StackAlign::Start, StackJustify::Center);
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 50.), growing(100., 40., 1.), entry(100., 30.)],
        );

        // 50 + 40 + 30 + two gaps leaves 460, all of it for the middle one,
        // which grows from its own 40. Nothing's left to centre.
        assert_eq!(cells[1].size, [100., 500.]);
        assert_eq!(ats(&cells), [[0., 0.], [0., 60.], [0., 570.]]);
        assert_eq!(run, 600.);
    }

    #[test]
    fn growing_children_share_by_weight() {
        let stack = column(0., StackAlign::Start, StackJustify::Start);
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[
                growing(100., 0., 1.),
                growing(100., 0., 2.),
                entry(100., 300.),
            ],
        );

        assert!(near(cells[0].size, [100., 100.]), "{cells:?}");
        assert!(near(cells[1].size, [100., 200.]), "{cells:?}");
        assert!(near(cells[2].at, [0., 300.]), "{cells:?}");
        assert!((run - 600.).abs() < 1e-3);

        // Each grows from its own size, sharing only what's left.
        let (cells, _) = super::cells(
            &stack,
            [400., 600.],
            &[
                growing(100., 100., 1.),
                growing(100., 0., 1.),
                entry(100., 300.),
            ],
        );
        assert!(near(cells[0].size, [100., 200.]), "{cells:?}");
        assert!(near(cells[1].size, [100., 100.]), "{cells:?}");
    }

    #[test]
    fn a_child_at_its_maximum_passes_the_rest_on() {
        let stack = column(0., StackAlign::Start, StackJustify::End);
        let capped = Entry {
            most: 120.,
            ..growing(100., 20., 1.)
        };
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[capped, growing(100., 20., 1.), entry(100., 60.)],
        );

        // 500 left: half each would be 270, so the capped one stops at 120
        // and the other takes the 400 it leaves on top of its own 20.
        assert!(near(cells[0].size, [100., 120.]), "{cells:?}");
        assert!(near(cells[1].size, [100., 420.]), "{cells:?}");
        assert!((run - 600.).abs() < 1e-3);

        // Alone and capped, what it doesn't take goes back to justify.
        let (cells, run) = super::cells(&stack, [400., 600.], &[capped, entry(100., 60.)]);
        assert_eq!(cells[0].size, [100., 120.]);
        assert_eq!(run, 180.);
        assert_eq!(cells[0].at, [0., 420.]);
    }

    #[test]
    fn a_locked_shape_grows_into_the_largest_box_the_room_allows() {
        // The cover: square, its space across the card's width less 20 and
        // capped at 540, no base height.
        let cover = |width: f32| Entry {
            size: [width, 0.],
            space: [width, 0.],
            most: 540.,
            aspect: Some(1.),
            ..growing(width, 0., 1.)
        };
        let stack = column(0., StackAlign::Center, StackJustify::Center);

        // A short card: the room along is the limit, so it's 300 square.
        let (cells, _) = super::cells(&stack, [500., 500.], &[cover(480.), entry(400., 200.)]);
        assert!(near(cells[0].size, [300., 300.]), "{cells:?}");
        assert!(near(cells[0].at, [100., 0.]), "{cells:?}");

        // A tall one: the width is, and what it can't use goes back to
        // justify, so the block sits centred.
        let (cells, run) = super::cells(&stack, [500., 1200.], &[cover(480.), entry(400., 200.)]);
        assert!(near(cells[0].size, [480., 480.]), "{cells:?}");
        assert!((run - 680.).abs() < 1e-3);
        assert!(near(cells[0].at, [10., 260.]), "{cells:?}");

        // A huge one: its maximum.
        let (cells, _) = super::cells(&stack, [2000., 2000.], &[cover(1900.), entry(400., 200.)]);
        assert!(near(cells[0].size, [540., 540.]), "{cells:?}");

        // A shape that stops short across hands the rest to another grower.
        let (cells, _) = super::cells(
            &stack,
            [500., 1200.],
            &[cover(480.), growing(400., 100., 1.)],
        );
        assert!(near(cells[0].size, [480., 480.]), "{cells:?}");
        assert!(near(cells[1].size, [400., 720.]), "{cells:?}");
    }

    #[test]
    fn nothing_grows_when_the_children_already_overflow() {
        let stack = column(0., StackAlign::Start, StackJustify::Start);
        let (cells, run) = super::cells(
            &stack,
            [100., 100.],
            &[growing(10., 30., 1.), entry(10., 90.)],
        );

        assert_eq!(cells[0].size, [10., 30.]);
        assert_eq!(run, 120.);
    }

    #[test]
    fn a_hidden_grower_takes_nothing_and_a_fading_one_keeps_its_box() {
        let stack = column(0., StackAlign::Start, StackJustify::Start);
        let mut hidden = growing(100., 0., 1.);
        hidden.presence = 0.;
        let (cells, _) = super::cells(
            &stack,
            [400., 600.],
            &[hidden, growing(100., 0., 1.), entry(100., 200.)],
        );
        assert!(near(cells[1].size, [100., 400.]), "{cells:?}");

        // Half there, it's still offered the whole room and takes half of it,
        // so it shrinks away in place.
        hidden.presence = 0.5;
        let (cells, run) = super::cells(&stack, [400., 600.], &[hidden, entry(100., 200.)]);
        assert!(near(cells[0].size, [100., 400.]), "{cells:?}");
        assert!((run - 400.).abs() < 1e-3);
    }

    #[test]
    fn margins_add_to_the_gap_and_go_with_a_hidden_child() {
        let stack = column(10., StackAlign::Start, StackJustify::Start);
        let mut spaced = entry(100., 40.);
        spaced.margin = [12., 20.];
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 30.), spaced, entry(100., 30.)],
        );

        // 30, a gap of 10 and 12 more, 40, 20 and a gap, then the last.
        assert_eq!(ats(&cells), [[0., 0.], [0., 52.], [0., 122.]]);
        assert_eq!(run, 152.);

        // The same layout with the air baked into a padded height, the old
        // way, puts the neighbours in the same places.
        let padded = entry(100., 72.);
        let (baked, baked_run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 30.), padded, entry(100., 30.)],
        );
        assert_eq!(baked[2].at, cells[2].at);
        assert_eq!(baked_run, run);

        // Hidden, its margins go too.
        spaced.presence = 0.;
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 30.), spaced, entry(100., 30.)],
        );
        assert_eq!(cells[2].at, [0., 40.]);
        assert_eq!(run, 70.);

        // A margin on the first child counts even with no gap ahead of it.
        let mut first = entry(100., 30.);
        first.margin = [8., 0.];
        let (cells, _) = super::cells(&stack, [400., 600.], &[first]);
        assert_eq!(cells[0].at, [0., 8.]);
    }

    #[test]
    fn between_spreads_whats_left_and_growth_leaves_it_nothing() {
        let stack = column(10., StackAlign::Start, StackJustify::Between);
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 50.), entry(100., 50.), entry(100., 50.)],
        );
        assert_eq!(ats(&cells), [[0., 0.], [0., 275.], [0., 550.]]);
        assert_eq!(run, 600.);

        let (cells, _) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 50.), growing(100., 50., 1.), entry(100., 50.)],
        );
        assert_eq!(ats(&cells), [[0., 0.], [0., 60.], [0., 550.]]);

        // One child sits at the start, and an overflowing run too.
        let (cells, _) = super::cells(&stack, [400., 600.], &[entry(100., 50.)]);
        assert_eq!(cells[0].at, [0., 0.]);
        let (cells, _) = super::cells(&stack, [400., 60.], &[entry(100., 50.), entry(100., 50.)]);
        assert_eq!(ats(&cells), [[0., 0.], [0., 60.]]);
    }

    #[test]
    fn a_child_can_sit_across_on_its_own() {
        let stack = column(0., StackAlign::Start, StackJustify::Start);
        let mut right = entry(100., 50.);
        right.align = Some(StackAlign::End);
        let (cells, _) = super::cells(&stack, [400., 600.], &[entry(100., 50.), right]);

        assert_eq!(ats(&cells), [[0., 0.], [300., 50.]]);
    }

    #[test]
    fn padding_keeps_the_children_inside_it() {
        let stack = Stack {
            padding: [10., 20., 30., 40.],
            ..column(0., StackAlign::End, StackJustify::End)
        };
        assert_eq!(stack.inner([400., 600.]), [360., 540.]);

        // Cells are in the box inside the padding.
        let (cells, run) = super::cells(&stack, [400., 600.], &[entry(100., 40.)]);
        assert_eq!(cells[0].at, [260., 500.]);
        assert_eq!(run, 40.);

        // Fractions measure against the inner box too.
        let half = child(Place {
            size: [Length::Stage(0.5), Length::Stage(0.5)],
            ..Place::default()
        });
        let entries = super::entries(&stack, &[half], [400., 600.], shown);
        assert_eq!(entries[0].size, [180., 270.]);

        // Junk or negative padding reads as none, and padding past the box
        // leaves an empty one.
        let odd = Stack {
            padding: [-5., f32::NAN, 500., 0.],
            ..Stack::default()
        };
        assert_eq!(odd.pads(), [0., 0., 500., 0.]);
        assert_eq!(odd.inner([400., 600.]), [0., 600.]);
    }

    #[test]
    fn padding_moves_the_childrens_frame_in() {
        let stack = Stack {
            padding: [10., 20., 30., 40.],
            ..Stack::default()
        };
        let frame = Frame {
            transform: Mat4::translate([100., 0., 0.]),
            size: [400., 600.],
            opacity: 1.,
            k: 1.,
            rest: 1.,
            cell: None,
        };
        let inner = stack.inner_frame(&frame);

        assert_eq!(inner.size, [360., 540.]);
        assert_eq!(inner.transform.point([0., 0., 0.]), [110., 20., 0.]);
    }

    #[test]
    fn reversed_the_first_child_sits_at_the_far_end() {
        let stack = Stack {
            reverse: true,
            ..column(10., StackAlign::Start, StackJustify::Start)
        };
        let (cells, run) =
            super::cells(&stack, [400., 600.], &[entry(100., 50.), entry(100., 30.)]);

        assert_eq!(ats(&cells), [[0., 550.], [0., 510.]]);
        assert_eq!(run, 90.);
    }

    fn child(place: Place) -> CanvasItem {
        CanvasItem {
            place,
            content: Content::Image {
                asset: "a".into(),
                fit: Default::default(),
                slice: None,
            },
            ..CanvasItem::default()
        }
    }

    #[test]
    fn children_keep_their_own_sizes_fractions_of_the_group_included() {
        let square = child(Place {
            size: [Length::Stage(1.), Length::Stage(0.6)],
            aspect: Some(1.),
            ..Place::default()
        });
        let capped = child(Place {
            size: [Length::Stage(1.), Length::Units(30.)],
            max_size: [Some(250.), None],
            ..Place::default()
        });
        let line = child(Place {
            size: [
                Length::Mixed {
                    stage: 0.5,
                    units: 20.,
                },
                Length::Units(24.),
            ],
            ..Place::default()
        });
        let mut off = line.clone();
        off.visible = false;
        let children = [square, capped, off, line];

        // A wide group: the square is held by the height's 60%.
        let stack = column(10., StackAlign::Center, StackJustify::Center);
        let wide = entries(&stack, &children, [1000., 500.], shown);
        let sizes: Vec<[f32; 2]> = wide.iter().map(|e| e.size).collect();
        assert_eq!(sizes, [[300., 300.], [250., 30.], [520., 24.], [520., 24.]]);
        assert_eq!(wide[2].presence, 0.);

        let (cells, run) = super::cells(&stack, [1000., 500.], &wide);
        assert_eq!(run, 300. + 10. + 30. + 10. + 24.);
        assert!(near(cells[0].at, [350., 63.]), "{cells:?}");
        assert!(near(cells[1].at, [375., 373.]), "{cells:?}");
        assert!(near(cells[3].at, [240., 413.]), "{cells:?}");

        // A tall one: the square is held by the width, and the fraction along
        // the axis is of the group, not of what's left.
        let tall = entries(&stack, &children, [400., 1000.], shown);
        assert_eq!(tall[0].size, [400., 400.]);
        assert_eq!(tall[1].size, [250., 30.]);
    }

    #[test]
    fn a_childs_place_carries_its_growth_margin_and_alignment() {
        let mut item = child(Place {
            size: [Length::Stage(0.8), Length::Units(0.)],
            max_size: [Some(540.), Some(540.)],
            aspect: Some(1.),
            grow: 1.,
            margin: [12., 20.],
            align_self: Some(StackAlign::End),
            ..Place::default()
        });
        item.id = "cover".into();

        let stack = column(0., StackAlign::Center, StackJustify::Center);
        let entry = entries(&stack, std::slice::from_ref(&item), [500., 800.], shown)[0];
        assert_eq!(entry.grow, 1.);
        assert_eq!(entry.margin, [12., 20.]);
        assert_eq!(entry.align, Some(StackAlign::End));
        assert_eq!(entry.space, [400., 0.]);
        assert_eq!(entry.most, 540.);

        // It saves its stack fields, and a place without them saves none.
        let saved = serde_json::to_value(item.place).unwrap();
        assert_eq!(saved["grow"], json!(1.0));
        assert_eq!(saved["margin"], json!([12.0, 20.0]));
        assert_eq!(saved["align_self"], json!("end"));
        assert_eq!(serde_json::from_value::<Place>(saved).unwrap(), item.place);

        let plain = serde_json::to_value(Place::default()).unwrap();
        for key in ["grow", "margin", "align_self"] {
            assert!(plain.get(key).is_none(), "{plain}");
        }
    }

    #[test]
    fn a_stack_round_trips_and_a_plain_item_writes_none() {
        let plain = serde_json::to_value(CanvasItem::default()).unwrap();
        assert!(plain.get("stack").is_none(), "{plain}");

        let item: CanvasItem = serde_json::from_value(json!({
            "id": "card",
            "stack": { "axis": "y", "gap": 22, "align": "center", "justify": "center" },
            "content": { "kind": "group", "items": [] },
        }))
        .unwrap();
        assert_eq!(
            item.stack,
            Some(column(22., StackAlign::Center, StackJustify::Center))
        );
        let again = serde_json::to_value(&item).unwrap();
        assert_eq!(
            again["stack"],
            json!({ "axis": "y", "gap": 22.0, "align": "center", "justify": "center" })
        );

        // The newer fields write only when set.
        let full: CanvasItem = serde_json::from_value(json!({
            "id": "card",
            "stack": { "justify": "between", "padding": [1, 2, 3, 4], "reverse": true },
        }))
        .unwrap();
        let stack = full.stack.unwrap();
        assert_eq!(stack.justify, StackJustify::Between);
        assert_eq!(stack.padding, [1., 2., 3., 4.]);
        assert!(stack.reverse);
        let saved = serde_json::to_value(&full).unwrap();
        assert_eq!(saved["stack"]["padding"], json!([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(saved["stack"]["reverse"], json!(true));

        // Fields left out take their defaults, and junk costs only the stack.
        let short: CanvasItem =
            serde_json::from_value(json!({ "id": "r", "stack": { "axis": "x" } })).unwrap();
        assert_eq!(
            short.stack.map(|s| (s.axis, s.gap)),
            Some((StackAxis::X, 0.))
        );

        let odd: CanvasItem =
            serde_json::from_value(json!({ "id": "o", "stack": { "axis": "z" }, "opacity": 0.5 }))
                .unwrap();
        assert_eq!((odd.stack, odd.opacity), (None, 0.5));

        // Between is a justify, not an align.
        let wrong: CanvasItem =
            serde_json::from_value(json!({ "id": "w", "stack": { "align": "between" } })).unwrap();
        assert_eq!(wrong.stack, None);
    }
}
