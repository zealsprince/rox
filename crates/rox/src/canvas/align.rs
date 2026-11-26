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
//! written back as a fraction.

use rox_canvas::{Length, Place};

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
    fn axis_and_end(self) -> (usize, f32) {
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
    flat: bool,
    arrange: Arrange,
) -> Option<Place> {
    let size = sizes(place, parent);
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
            let mut at = center(place, parent, size);
            at[axis] = drawn / 2. + end * (parent[axis] - drawn);
            let units = position_for(place, parent, size, at)[axis];
            out.position.xy[axis] = out.position.xy[axis].with_units(units, parent[axis]);
        }

        Arrange::Fill => {
            let want = [parent[0] / scale, parent[1] / scale];
            out.size = [
                resized(place.size[0], parent[0], want[0]),
                resized(place.size[1], parent[1], want[1]),
            ];
            let size = sizes(&out, parent);
            let middle = [parent[0] / 2., parent[1] / 2.];
            let units = position_for(&out, parent, size, middle);
            moved_to(&mut out, parent, units);
        }

        Arrange::Fit => {
            let k = (parent[0] / (size[0] * scale)).min(parent[1] / (size[1] * scale));
            out.size = [
                resized(place.size[0], parent[0], size[0] * k),
                resized(place.size[1], parent[1], size[1] * k),
            ];
            let size = sizes(&out, parent);
            let middle = [parent[0] / 2., parent[1] / 2.];
            let units = position_for(&out, parent, size, middle);
            moved_to(&mut out, parent, units);
        }
    }

    Some(out)
}

fn sizes(place: &Place, parent: [f32; 2]) -> [f32; 2] {
    place.size_in(parent)
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
/// anchor point placed on it, as the projection lays it out.
fn center(place: &Place, parent: [f32; 2], size: [f32; 2]) -> [f32; 2] {
    let [fx, fy] = place.anchor.fraction();
    let [x, y, _] = place.offset_in(parent);
    [
        fx * parent[0] + x - fx * size[0] + size[0] / 2.,
        fy * parent[1] + y - fy * size[1] + size[1] / 2.,
    ]
}

/// The position that puts the box's middle at `at`, under the same anchor.
fn position_for(place: &Place, parent: [f32; 2], size: [f32; 2], at: [f32; 2]) -> [f32; 2] {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_canvas::Anchor;

    /// The item's drawn box in the parent's units: left, top, width, height.
    fn drawn_box(place: &Place, parent: [f32; 2], flat: bool) -> [f32; 4] {
        let size = sizes(place, parent);
        let scale = drawn_scale(place, flat);
        let center = center(place, parent, size);

        let drawn = [size[0] * scale, size[1] * scale];
        [
            center[0] - drawn[0] / 2.,
            center[1] - drawn[1] / 2.,
            drawn[0],
            drawn[1],
        ]
    }

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
        let out = arranged(place, STAGE, flat, arrange).expect("an item with an area");
        assert_eq!(out.position.z, 7., "an arrange never moves depth");
        assert_eq!(out.anchor, place.anchor, "nor the anchor");
        drawn_box(&out, STAGE, flat)
    }

    #[test]
    fn the_box_reads_the_anchor_and_the_units() {
        let centred = place(Anchor::Center, [0., 0.], units(200., 100.));
        assert!(close(
            drawn_box(&centred, STAGE, true),
            [400., 200., 200., 100.]
        ));

        let corner = place(Anchor::BottomRight, [-10., -20.], units(200., 100.));
        assert!(close(
            drawn_box(&corner, STAGE, true),
            [790., 380., 200., 100.]
        ));

        let percent = place(
            Anchor::TopLeft,
            [50., 0.],
            [Length::Stage(0.5), Length::Stage(0.2)],
        );
        assert!(close(
            drawn_box(&percent, STAGE, true),
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
            let start = drawn_box(&item, STAGE, true);
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

        let out = arranged(&item, STAGE, true, Arrange::Align(Line::Right)).unwrap();
        assert_eq!(out.size, item.size);
        assert_eq!(
            out.position.xy[1],
            Length::Units(40.),
            "the other axis stays"
        );
        assert!(close(
            drawn_box(&out, STAGE, true),
            [750., 165., 250., 250.]
        ));
    }

    #[test]
    fn an_align_writes_a_fractional_position_as_a_fraction() {
        let mut item = place(Anchor::TopLeft, [0., 0.], units(200., 100.));
        item.position.xy = [Length::Stage(0.1), Length::Stage(0.3)];

        let out = arranged(&item, STAGE, true, Arrange::Align(Line::Right)).unwrap();
        assert_eq!(out.position.xy[0], Length::Stage(0.8));
        assert_eq!(
            out.position.xy[1],
            Length::Stage(0.3),
            "the other axis stays"
        );
        assert!(close(
            drawn_box(&out, STAGE, true),
            [800., 150., 200., 100.]
        ));

        let out = arranged(&item, STAGE, true, Arrange::Fill).unwrap();
        assert_eq!(out.position.xy, [Length::Stage(0.), Length::Stage(0.)]);
    }

    #[test]
    fn a_scaled_item_aligns_by_what_it_draws_and_a_flat_one_ignores_scale() {
        let mut item = place(Anchor::TopLeft, [0., 0.], units(200., 100.));
        item.scale = 2.;

        // It draws 400 by 200 about the middle of its 200 by 100 box.
        assert!(close(
            drawn_box(&item, STAGE, false),
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
        let out = arranged(&item, STAGE, true, Arrange::Fill).unwrap();
        assert_eq!(out.size, units(1000., 500.));
        assert!(close(drawn_box(&out, STAGE, true), [0., 0., 1000., 500.]));

        let percent = place(
            Anchor::Center,
            [9., 9.],
            [Length::Stage(0.1), Length::Units(40.)],
        );
        let out = arranged(&percent, STAGE, true, Arrange::Fill).unwrap();
        assert_eq!(out.size, [Length::Stage(1.), Length::Units(500.)]);
        assert!(close(drawn_box(&out, STAGE, true), [0., 0., 1000., 500.]));

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
        let out = arranged(&tall, STAGE, true, Arrange::Fit).unwrap();
        assert!(matches!(out.size, [Length::Stage(_), Length::Stage(_)]));
        assert!(close(drawn_box(&out, STAGE, true), [375., 0., 250., 500.]));
    }

    #[test]
    fn nothing_arranges_without_an_area() {
        let flat = place(Anchor::Center, [0., 0.], units(0., 100.));
        assert!(arranged(&flat, STAGE, true, Arrange::Fit).is_none());

        let item = place(Anchor::Center, [0., 0.], units(10., 10.));
        assert!(arranged(&item, [0., 500.], true, Arrange::Fill).is_none());
    }
}
