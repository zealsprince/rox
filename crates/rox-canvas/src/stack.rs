//! A group that lays its children out one after another along x or y, with
//! a gap, the way a column of a cover and two lines of text sits centred in
//! whatever space a window leaves.
//!
//! The stack only decides where each child's box goes: along the axis in
//! list order, and across it by `align`. Each child keeps its own size,
//! limits and aspect, measured against the group's box as anywhere else, so
//! a fraction along the axis is a fraction of the group and not of what's
//! left. Nothing shrinks or grows to fit: a stack longer than its group runs
//! past it. A child's own x and y then move it from where the stack put
//! it, as do its routes and sway. Depth, turns and scale are left alone,
//! since the stack lays out the flat boxes before anything is projected;
//! like a CSS transform, a child's turn and scale don't take room.
//!
//! Each child takes room by its presence, 0 to 1: a hidden one none, and one
//! easing in or out a share, so its neighbours close up as it fades.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::state::CanvasItem;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stack {
    pub axis: StackAxis,
    /// Units between one child and the next.
    pub gap: f32,
    /// Where each child sits across the axis.
    pub align: StackAlign,
    /// Where the run of children sits along the axis.
    pub justify: StackAlign,
}

impl Default for Stack {
    fn default() -> Self {
        Stack {
            axis: StackAxis::Y,
            gap: 0.,
            align: StackAlign::Start,
            justify: StackAlign::Start,
        }
    }
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

/// A child as the stack sees it: its box's size in the group's units, and
/// how much room it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub size: [f32; 2],
    pub presence: f32,
}

/// Each entry's cell: where its box's top-left goes in the group's units,
/// before its own offset. The second value is how long the run is along the
/// axis, gaps included, which overflows the group when it's longer.
///
/// An entry easing in or out keeps its own size and sits in the middle of
/// the share of room it has, so it shrinks away in place. A gap counts
/// before each entry after the first, by that entry's presence, and only
/// once something is showing ahead of it.
pub fn cells(stack: &Stack, group: [f32; 2], entries: &[Entry]) -> (Vec<[f32; 2]>, f32) {
    let along = stack.axis.along();
    let across = stack.axis.across();
    let gap = if stack.gap.is_finite() { stack.gap } else { 0. };

    // Laid out from zero first, since justify needs the run's length.
    let mut starts = Vec::with_capacity(entries.len());
    let mut cursor = 0f32;
    let mut ahead = 0f32;
    for entry in entries {
        let presence = presence(entry.presence);
        cursor += gap * presence * ahead.min(1.);

        let length = entry.size[along];
        starts.push(cursor + (presence - 1.) * length / 2.);

        cursor += length * presence;
        ahead += presence;
    }

    let run = cursor;
    let lead = stack.justify.share() * (group[along] - run);
    let cells = entries
        .iter()
        .zip(starts)
        .map(|(entry, start)| {
            let mut cell = [0.; 2];
            cell[along] = lead + start;
            cell[across] = stack.align.share() * (group[across] - entry.size[across]);
            cell
        })
        .collect();

    (cells, run)
}

/// The stack's entries for `children` in a group box of `group`, each sized
/// as it lays out there, with `presence` saying how much room each takes.
pub fn entries(
    children: &[CanvasItem],
    group: [f32; 2],
    mut presence: impl FnMut(&CanvasItem) -> f32,
) -> Vec<Entry> {
    children
        .iter()
        .map(|child| Entry {
            size: child.place.size_in(group),
            presence: presence(child),
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

    fn column(gap: f32, align: StackAlign, justify: StackAlign) -> Stack {
        Stack {
            axis: StackAxis::Y,
            gap,
            align,
            justify,
        }
    }

    fn entry(w: f32, h: f32) -> Entry {
        Entry {
            size: [w, h],
            presence: 1.,
        }
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn a_column_runs_down_with_its_gaps_from_the_top() {
        let stack = column(10., StackAlign::Start, StackAlign::Start);
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(100., 50.), entry(200., 20.), entry(50., 30.)],
        );

        assert_eq!(cells, [[0., 0.], [0., 60.], [0., 90.]]);
        assert_eq!(run, 120.);
    }

    #[test]
    fn centred_both_ways_the_block_sits_in_the_middle() {
        let stack = column(10., StackAlign::Center, StackAlign::Center);
        let (cells, run) =
            super::cells(&stack, [400., 600.], &[entry(100., 50.), entry(200., 20.)]);

        // 80 long in 600 leaves 260 above, and each is centred across.
        assert_eq!(run, 80.);
        assert_eq!(cells, [[150., 260.], [100., 320.]]);
    }

    #[test]
    fn a_row_runs_along_x_and_ends_at_the_far_side() {
        let stack = Stack {
            axis: StackAxis::X,
            gap: 5.,
            align: StackAlign::End,
            justify: StackAlign::End,
        };
        let (cells, run) = super::cells(&stack, [300., 100.], &[entry(40., 10.), entry(60., 100.)]);

        assert_eq!(run, 105.);
        assert_eq!(cells, [[195., 90.], [240., 0.]]);
    }

    #[test]
    fn a_hidden_child_takes_no_room_and_no_gap() {
        let stack = column(10., StackAlign::Start, StackAlign::Start);
        let mut hidden = entry(100., 50.);
        hidden.presence = 0.;

        // Hidden first: the next one starts at the top with no gap before it.
        let (cells, run) = super::cells(&stack, [400., 600.], &[hidden, entry(80., 20.)]);
        assert_eq!(cells[1], [0., 0.]);
        assert_eq!(run, 20.);

        // Hidden between two: one gap, not two.
        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(80., 20.), hidden, entry(80., 20.)],
        );
        assert_eq!(cells[2], [0., 30.]);
        assert_eq!(run, 50.);
    }

    #[test]
    fn a_child_easing_out_shrinks_in_place_and_its_neighbours_close_up() {
        let stack = column(10., StackAlign::Start, StackAlign::Start);
        let mut fading = entry(100., 40.);
        fading.presence = 0.5;

        let (cells, run) = super::cells(
            &stack,
            [400., 600.],
            &[entry(80., 20.), fading, entry(80., 20.)],
        );

        // Half its room and half a gap: centred on a 20-long share.
        assert!(near(cells[1], [0., 15.]), "{cells:?}");
        assert!(near(cells[2], [0., 55.]), "{cells:?}");
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
        let stack = column(0., StackAlign::Start, StackAlign::Center);
        let (cells, run) = super::cells(&stack, [100., 100.], &[entry(10., 80.), entry(10., 80.)]);

        // 160 in 100 overflows 30 at each end.
        assert_eq!(run, 160.);
        assert_eq!(cells, [[0., -30.], [0., 50.]]);
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
        let wide = entries(&children, [1000., 500.], shown);
        let sizes: Vec<[f32; 2]> = wide.iter().map(|e| e.size).collect();
        assert_eq!(sizes, [[300., 300.], [250., 30.], [520., 24.], [520., 24.]]);
        assert_eq!(wide[2].presence, 0.);

        let stack = column(10., StackAlign::Center, StackAlign::Center);
        let (cells, run) = super::cells(&stack, [1000., 500.], &wide);
        assert_eq!(run, 300. + 10. + 30. + 10. + 24.);
        assert!(near(cells[0], [350., 63.]), "{cells:?}");
        assert!(near(cells[1], [375., 373.]), "{cells:?}");
        assert!(near(cells[3], [240., 413.]), "{cells:?}");

        // A tall one: the square is held by the width, and the fraction along
        // the axis is of the group, not of what's left.
        let tall = entries(&children, [400., 1000.], shown);
        assert_eq!(tall[0].size, [400., 400.]);
        assert_eq!(tall[1].size, [250., 30.]);
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
            Some(column(22., StackAlign::Center, StackAlign::Center))
        );
        let again = serde_json::to_value(&item).unwrap();
        assert_eq!(
            again["stack"],
            json!({ "axis": "y", "gap": 22.0, "align": "center", "justify": "center" })
        );

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
    }
}
