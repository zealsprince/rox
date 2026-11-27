//! The inspector's arrange tools as arithmetic: an item's place moved so its
//! box lines up with an edge or the middle of its parent's box, or sized to
//! fill or fit inside it. Pure, so every case is a test.
//!
//! The box is the one the item draws at depth 0, before any turn: its size
//! in the parent's units, times its scale for an item that takes
//! perspective. A flat item stores a scale and draws without it, so its box
//! is its size. A turned item aligns by that unturned box, which the
//! inspector says under the row. The anchor and the units of each side
//! stay as they were: only the position moves for an align, and the size
//! changes in its own unit for a fill or a fit. A fractional x or y is
//! written back as a fraction, and a fraction plus units moves its units.
//!
//! An item in a stack measures from its cell, where the stack puts its box
//! and gives it its size, which a child that grows takes, instead of from
//! its anchor. The editor keeps the tools that would move it along the
//! stack's axis switched off.

use rox_canvas::{Length, Place, Stack, StackCell};

/// Which line of the parent's box an align uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

impl Line {
    pub(crate) const ALL: [Line; 6] = [
        Line::Left,
        Line::CenterX,
        Line::Right,
        Line::Top,
        Line::CenterY,
        Line::Bottom,
    ];

    /// The axis it moves along, 0 for x, and where on that axis the item's
    /// box lands: 0 for the near edge, 0.5 the middle, 1 the far edge.
    pub(crate) fn axis_and_end(self) -> (usize, f32) {
        match self {
            Line::Left => (0, 0.),
            Line::CenterX => (0, 0.5),
            Line::Right => (0, 1.),
            Line::Top => (1, 0.),
            Line::CenterY => (1, 0.5),
            Line::Bottom => (1, 1.),
        }
    }
}

/// What one press of the row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arrange {
    Align(Line),
    /// Covers the parent's box edge to edge.
    Fill,
    /// The largest box with the item's own shape that fits inside, centred.
    Fit,
}

/// The place after `arrange`, or None when there's nothing to do it with:
/// an item with no area, or a parent with none.
pub(crate) fn arranged(
    place: &Place,
    parent: [f32; 2],
    cell: Option<StackCell>,
    flat: bool,
    arrange: Arrange,
) -> Option<Place> {
    let size = sizes(place, parent, cell);
    let scale = drawn_scale(place, flat);
    let area = |s: [f32; 2]| s[0] > 0. && s[1] > 0. && s[0].is_finite() && s[1].is_finite();
    if !area(size) || !area(parent) || scale <= 0. {
        return None;
    }

    let mut out = *place;
    match arrange {
        Arrange::Align(line) => {
            let (axis, end) = line.axis_and_end();
            let drawn = size[axis] * scale;
            let mut at = center(place, parent, cell, size);
            at[axis] = drawn / 2. + end * (parent[axis] - drawn);
            let units = position_for(place, parent, cell, size, at)[axis];
            out.position.xy[axis] = out.position.xy[axis].with_units(units, parent[axis]);
        }

        Arrange::Fill => {
            let want = [parent[0] / scale, parent[1] / scale];
            out.size = [
                resized(place.size[0], parent[0], want[0]),
                resized(place.size[1], parent[1], want[1]),
            ];
            let size = out.size_in(parent);
            let middle = [parent[0] / 2., parent[1] / 2.];
            let units = position_for(&out, parent, cell, size, middle);
            moved_to(&mut out, parent, units);
        }

        Arrange::Fit => {
            let k = (parent[0] / (size[0] * scale)).min(parent[1] / (size[1] * scale));
            out.size = [
                resized(place.size[0], parent[0], size[0] * k),
                resized(place.size[1], parent[1], size[1] * k),
            ];
            let size = out.size_in(parent);
            let middle = [parent[0] / 2., parent[1] / 2.];
            let units = position_for(&out, parent, cell, size, middle);
            moved_to(&mut out, parent, units);
        }
    }

    Some(out)
}

/// Whether `arrange` leaves a stacked item where the stack put it along
/// the stack's axis: only an align across it does. Fill and Fit centre the
/// box both ways.
pub(crate) fn free_in(arrange: Arrange, stack: Stack) -> bool {
    match arrange {
        Arrange::Align(line) => line.axis_and_end().0 != stack.axis.along(),
        Arrange::Fill | Arrange::Fit => false,
    }
}

/// The item's drawn box in the parent's units: left, top, width, height.
pub(crate) fn drawn_box(
    place: &Place,
    parent: [f32; 2],
    cell: Option<StackCell>,
    flat: bool,
) -> [f32; 4] {
    let size = sizes(place, parent, cell);
    let scale = drawn_scale(place, flat);
    let center = center(place, parent, cell, size);

    let drawn = [size[0] * scale, size[1] * scale];
    [
        center[0] - drawn[0] / 2.,
        center[1] - drawn[1] / 2.,
        drawn[0],
        drawn[1],
    ]
}

fn sizes(place: &Place, parent: [f32; 2], cell: Option<StackCell>) -> [f32; 2] {
    match cell {
        Some(cell) => cell.size,
        None => place.size_in(parent),
    }
}

/// x and y set to `units`, each axis written in the unit it already uses.
fn moved_to(place: &mut Place, parent: [f32; 2], units: [f32; 2]) {
    for axis in 0..2 {
        place.position.xy[axis] = place.position.xy[axis].with_units(units[axis], parent[axis]);
    }
}

fn drawn_scale(place: &Place, flat: bool) -> f32 {
    if flat { 1. } else { place.scale }
}

/// The box's middle: the anchor's point on the parent, then the item's own
/// anchor point placed on it, as the projection lays it out. In a stack's
/// cell the box's top-left sits at the cell.
fn center(place: &Place, parent: [f32; 2], cell: Option<StackCell>, size: [f32; 2]) -> [f32; 2] {
    let [x, y, _] = place.offset_in(parent);
    if let Some(StackCell { at, .. }) = cell {
        return [at[0] + x + size[0] / 2., at[1] + y + size[1] / 2.];
    }

    let [fx, fy] = place.anchor.fraction();
    [
        fx * parent[0] + x - fx * size[0] + size[0] / 2.,
        fy * parent[1] + y - fy * size[1] + size[1] / 2.,
    ]
}

/// The position that puts the box's middle at `at`, under the same anchor
/// or in the same cell.
fn position_for(
    place: &Place,
    parent: [f32; 2],
    cell: Option<StackCell>,
    size: [f32; 2],
    at: [f32; 2],
) -> [f32; 2] {
    if let Some(cell) = cell {
        return [
            at[0] - size[0] / 2. - cell.at[0],
            at[1] - size[1] / 2. - cell.at[1],
        ];
    }

    let [fx, fy] = place.anchor.fraction();
    [
        at[0] - size[0] / 2. + fx * size[0] - fx * parent[0],
        at[1] - size[1] / 2. + fy * size[1] - fy * parent[1],
    ]
}

/// `units` long, written in the unit the side already uses.
fn resized(length: Length, parent_axis: f32, units: f32) -> Length {
    match length {
        Length::Units(_) => Length::Units(units),
        Length::Stage(_) => Length::Stage(units / parent_axis),
        Length::Mixed { .. } => length.with_units(units, parent_axis),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_canvas::Anchor;

    const STAGE: [f32; 2] = [1000., 500.];

    fn place(anchor: Anchor, position: [f32; 2], size: [Length; 2]) -> Place {
        Place {
            anchor,
            position: rox_canvas::Position::units([position[0], position[1], 7.]),
            size,
            ..Place::default()
        }
    }

    fn units(w: f32, h: f32) -> [Length; 2] {
        [Length::Units(w), Length::Units(h)]
    }

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    fn after(place: &Place, flat: bool, arrange: Arrange) -> [f32; 4] {
        let out = arranged(place, STAGE, None, flat, arrange).expect("an item with an area");
        assert_eq!(out.position.z, 7., "an arrange never moves depth");
        assert_eq!(out.anchor, place.anchor, "nor the anchor");
        drawn_box(&out, STAGE, None, flat)
    }

    #[test]
    fn the_box_reads_the_anchor_and_the_units() {
        let centred = place(Anchor::Center, [0., 0.], units(200., 100.));
        assert!(close(
            drawn_box(&centred, STAGE, None, true),
            [400., 200., 200., 100.]
        ));

        let corner = place(Anchor::BottomRight, [-10., -20.], units(200., 100.));
        assert!(close(
            drawn_box(&corner, STAGE, None, true),
            [790., 380., 200., 100.]
        ));

        let percent = place(
            Anchor::TopLeft,
            [50., 0.],
            [Length::Stage(0.5), Length::Stage(0.2)],
        );
        assert!(close(
            drawn_box(&percent, STAGE, None, true),
            [50., 0., 500., 100.]
        ));
    }

    #[test]
    fn each_align_lands_on_its_line_from_any_anchor() {
        for anchor in [
            Anchor::TopLeft,
            Anchor::Center,
            Anchor::BottomRight,
            Anchor::Top,
        ] {
            let item = place(anchor, [33., -21.], units(200., 100.));
            let start = drawn_box(&item, STAGE, None, true);
            let cases = [
                (Line::Left, [0., start[1]]),
                (Line::CenterX, [400., start[1]]),
                (Line::Right, [800., start[1]]),
                (Line::Top, [start[0], 0.]),
                (Line::CenterY, [start[0], 200.]),
                (Line::Bottom, [start[0], 400.]),
            ];

            for (line, [left, top]) in cases {
                let got = after(&item, true, Arrange::Align(line));
                assert!(
                    close(got, [left, top, 200., 100.]),
                    "{anchor:?} {line:?}: {got:?}"
                );
            }
        }
    }

    #[test]
    fn an_align_keeps_percent_sizes_and_moves_only_the_position() {
        let item = place(
            Anchor::Center,
            [120., 40.],
            [Length::Stage(0.25), Length::Stage(0.5)],
        );

        let out = arranged(&item, STAGE, None, true, Arrange::Align(Line::Right)).unwrap();
        assert_eq!(out.size, item.size);
        assert_eq!(
            out.position.xy[1],
            Length::Units(40.),
            "the other axis stays"
        );
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [750., 165., 250., 250.]
        ));
    }

    #[test]
    fn an_align_writes_a_fractional_position_as_a_fraction() {
        let mut item = place(Anchor::TopLeft, [0., 0.], units(200., 100.));
        item.position.xy = [Length::Stage(0.1), Length::Stage(0.3)];

        let out = arranged(&item, STAGE, None, true, Arrange::Align(Line::Right)).unwrap();
        assert_eq!(out.position.xy[0], Length::Stage(0.8));
        assert_eq!(
            out.position.xy[1],
            Length::Stage(0.3),
            "the other axis stays"
        );
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [800., 150., 200., 100.]
        ));

        let out = arranged(&item, STAGE, None, true, Arrange::Fill).unwrap();
        assert_eq!(out.position.xy, [Length::Stage(0.), Length::Stage(0.)]);
    }

    #[test]
    fn a_scaled_item_aligns_by_what_it_draws_and_a_flat_one_ignores_scale() {
        let mut item = place(Anchor::TopLeft, [0., 0.], units(200., 100.));
        item.scale = 2.;

        // It draws 400 by 200 about the middle of its 200 by 100 box.
        assert!(close(
            drawn_box(&item, STAGE, None, false),
            [-100., -50., 400., 200.]
        ));
        assert!(close(
            after(&item, false, Arrange::Align(Line::Left)),
            [0., -50., 400., 200.]
        ));
        assert!(close(
            after(&item, false, Arrange::Align(Line::Bottom)),
            [-100., 300., 400., 200.]
        ));
        assert!(close(
            after(&item, true, Arrange::Align(Line::Right)),
            [800., 0., 200., 100.]
        ));
    }

    #[test]
    fn fill_covers_the_stage_in_the_items_own_units() {
        let item = place(Anchor::BottomRight, [-5., -5.], units(200., 100.));
        let out = arranged(&item, STAGE, None, true, Arrange::Fill).unwrap();
        assert_eq!(out.size, units(1000., 500.));
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [0., 0., 1000., 500.]
        ));

        let percent = place(
            Anchor::Center,
            [9., 9.],
            [Length::Stage(0.1), Length::Units(40.)],
        );
        let out = arranged(&percent, STAGE, None, true, Arrange::Fill).unwrap();
        assert_eq!(out.size, [Length::Stage(1.), Length::Units(500.)]);
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [0., 0., 1000., 500.]
        ));

        // A scaled item's drawn box covers it, not its size.
        let mut scaled = place(Anchor::TopLeft, [0., 0.], units(10., 10.));
        scaled.scale = 2.;
        assert!(close(
            after(&scaled, false, Arrange::Fill),
            [0., 0., 1000., 500.]
        ));
    }

    #[test]
    fn fit_keeps_the_shape_and_centres_it() {
        // Wider than the stage's shape: it meets the sides.
        let wide = place(Anchor::TopLeft, [70., 10.], units(400., 100.));
        assert!(close(
            after(&wide, true, Arrange::Fit),
            [0., 125., 1000., 250.]
        ));

        // Taller: it meets the top and bottom.
        let tall = place(
            Anchor::BottomRight,
            [0., 0.],
            [Length::Stage(0.1), Length::Stage(0.4)],
        );
        let out = arranged(&tall, STAGE, None, true, Arrange::Fit).unwrap();
        assert!(matches!(out.size, [Length::Stage(_), Length::Stage(_)]));
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [375., 0., 250., 500.]
        ));
    }

    #[test]
    fn nothing_arranges_without_an_area() {
        let flat = place(Anchor::Center, [0., 0.], units(0., 100.));
        assert!(arranged(&flat, STAGE, None, true, Arrange::Fit).is_none());

        let item = place(Anchor::Center, [0., 0.], units(10., 10.));
        assert!(arranged(&item, [0., 500.], None, true, Arrange::Fill).is_none());
    }

    #[test]
    fn in_a_stack_an_item_measures_from_its_cell_and_moves_only_across() {
        let item = place(Anchor::BottomRight, [5., 3.], units(40., 20.));
        let cell = Some(StackCell {
            at: [100., 50.],
            size: [40., 20.],
        });
        assert!(close(
            drawn_box(&item, STAGE, cell, true),
            [105., 53., 40., 20.]
        ));

        let left = arranged(&item, STAGE, cell, true, Arrange::Align(Line::Left)).unwrap();
        assert_eq!(left.position.xy[0], Length::Units(-100.));
        assert!(close(
            drawn_box(&left, STAGE, cell, true),
            [0., 53., 40., 20.]
        ));

        let middle = arranged(&item, STAGE, cell, true, Arrange::Align(Line::CenterY)).unwrap();
        assert!(close(
            drawn_box(&middle, STAGE, cell, true),
            [105., 240., 40., 20.]
        ));

        // A column lets the x aligns through and holds everything else.
        let column = Stack::default();
        assert!(free_in(Arrange::Align(Line::Right), column));
        assert!(!free_in(Arrange::Align(Line::Top), column));
        assert!(!free_in(Arrange::Fill, column));
        let row = Stack {
            axis: rox_canvas::StackAxis::X,
            ..Stack::default()
        };
        assert!(free_in(Arrange::Align(Line::Bottom), row));
        assert!(!free_in(Arrange::Align(Line::CenterX), row));

        // A child that grew measures by the size its cell gives it.
        let grown = Some(StackCell {
            at: [100., 50.],
            size: [40., 300.],
        });
        assert!(close(
            drawn_box(&item, STAGE, grown, true),
            [105., 53., 40., 300.]
        ));
        let right = arranged(&item, STAGE, grown, true, Arrange::Align(Line::Right)).unwrap();
        assert!(close(
            drawn_box(&right, STAGE, grown, true),
            [960., 53., 40., 300.]
        ));
    }

    #[test]
    fn an_align_moves_a_mixed_axis_by_its_units() {
        let mut item = place(Anchor::TopLeft, [0., 0.], units(100., 50.));
        item.position.xy[0] = Length::Mixed {
            stage: 0.5,
            units: 20.,
        };

        let out = arranged(&item, STAGE, None, true, Arrange::Align(Line::Right)).unwrap();
        assert_eq!(
            out.position.xy[0],
            Length::Mixed {
                stage: 0.5,
                units: 400.
            }
        );
        assert!(close(
            drawn_box(&out, STAGE, None, true),
            [900., 0., 100., 50.]
        ));
    }
}
