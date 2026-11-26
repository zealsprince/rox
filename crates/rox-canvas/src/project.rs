//! Where a canvas's items land on screen. World space is viewport pixels, x
//! right, y down, z toward the viewer. A frame's box runs from its origin to
//! `size`, so an anchor is a fraction of that box.

use crate::mat::Mat4;
use crate::state::{Camera, CanvasItem, Fit, Stage};

/// `focal - z` at or under this is behind the camera.
const BEHIND: f32 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    fn center(self) -> [f32; 2] {
        [self.width / 2., self.height / 2.]
    }
}

/// Where a parent leaves its children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub transform: Mat4,
    /// The box anchors and `Length::Stage` measure against, in units.
    pub size: [f32; 2],
    pub opacity: f32,
    /// Pixels per unit.
    pub k: f32,
}

impl Frame {
    pub fn stage(stage: &Stage, viewport: Viewport) -> Frame {
        let anchored = Frame {
            transform: Mat4::IDENTITY,
            size: [viewport.width, viewport.height],
            opacity: 1.,
            k: 1.,
        };

        // A stage with no area has nothing to scale by, so it lays out as
        // anchored rather than dividing by zero.
        if stage.width <= 0. || stage.height <= 0. {
            return anchored;
        }

        let ratios = [viewport.width / stage.width, viewport.height / stage.height];
        let k = match stage.fit {
            Fit::Anchor => return anchored,
            Fit::Contain => ratios[0].min(ratios[1]),
            Fit::Cover => ratios[0].max(ratios[1]),
        };

        let offset = [
            (viewport.width - stage.width * k) / 2.,
            (viewport.height - stage.height * k) / 2.,
            0.,
        ];

        Frame {
            transform: Mat4::translate(offset) * Mat4::scale(k),
            size: [stage.width, stage.height],
            opacity: 1.,
            k,
        }
    }
}

/// This frame's route offsets for one item. `Default` moves nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Live {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    /// Multiplies.
    pub scale: f32,
    /// Multiplies.
    pub opacity: f32,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            position: [0.; 3],
            rotation: [0.; 3],
            scale: 1.,
            opacity: 1.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projected {
    /// Clockwise from the item's top-left.
    pub corners: [[f32; 2]; 4],
    pub depth: f32,
    /// The depth scale at the item's center. Text sizes by it.
    pub scale: f32,
    pub opacity: f32,
}

/// None when any corner falls behind the camera.
pub fn project(
    item: &CanvasItem,
    parent: &Frame,
    camera: &Camera,
    viewport: Viewport,
    pointer: Option<[f32; 2]>,
    flat: bool,
    live: &Live,
) -> Option<Projected> {
    let view = View::new(camera, viewport, pointer, parent.k);
    let size = item_size(item, parent);
    let world = parent.transform * local(item, parent, live, flat);
    let center = world.point(center_offset(item, size));
    let (center_px, scale) = view.project(center)?;

    let corners = if flat {
        let half = [size[0] * parent.k / 2., size[1] * parent.k / 2.];
        let [x, y] = center_px;
        [
            [x - half[0], y - half[1]],
            [x + half[0], y - half[1]],
            [x + half[0], y + half[1]],
            [x - half[0], y + half[1]],
        ]
    } else {
        let [left, top] = box_origin(item, size);
        let local_corners = [
            [left, top],
            [left + size[0], top],
            [left + size[0], top + size[1]],
            [left, top + size[1]],
        ];
        let mut corners = [[0.; 2]; 4];
        for (out, [x, y]) in corners.iter_mut().zip(local_corners) {
            *out = view.project(world.point([x, y, 0.]))?.0;
        }
        corners
    };

    Some(Projected {
        corners,
        depth: center[2],
        scale,
        opacity: parent.opacity * item.opacity * live.opacity,
    })
}

/// The frame a group hands its children.
pub fn child_frame(item: &CanvasItem, parent: &Frame, live: &Live) -> Frame {
    let size = item_size(item, parent);
    let [left, top] = box_origin(item, size);

    Frame {
        transform: parent.transform
            * local(item, parent, live, false)
            * Mat4::translate([left, top, 0.]),
        size,
        opacity: parent.opacity * item.opacity * live.opacity,
        k: parent.k,
    }
}

/// Points on a frame's own box, in its units from its top-left, to pixels:
/// the corners of patches cut from one face, each projected once so two
/// patches sharing a corner share it exactly. None when any falls behind
/// the camera.
pub fn project_points<const N: usize>(
    frame: &Frame,
    camera: &Camera,
    viewport: Viewport,
    pointer: Option<[f32; 2]>,
    points: [[f32; 2]; N],
) -> Option<[[f32; 2]; N]> {
    let view = View::new(camera, viewport, pointer, frame.k);
    let mut out = [[0.; 2]; N];

    for (out, [x, y]) in out.iter_mut().zip(points) {
        *out = view.project(frame.transform.point([x, y, 0.]))?.0;
    }

    Some(out)
}

/// Indices, back to front, stable.
pub fn paint_order(depths: &[f32]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..depths.len()).collect();
    order.sort_by(|&a, &b| depths[a].total_cmp(&depths[b]));
    order
}

/// Point in a convex quad, corners in order. A point on an edge counts as
/// in, so a click on the seam between two items lands.
pub fn inside(corners: &[[f32; 2]; 4], point: [f32; 2]) -> bool {
    let (mut left, mut right) = (false, false);

    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let side = (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0]);
        left |= side > 0.;
        right |= side < 0.;
    }

    !(left && right)
}

/// Where `point` falls across a quad as the unit square: 0, 0 at the first
/// corner and 1, 1 at the third, so a tilted item maps the way it's drawn.
/// The inverse of the unit square onto the quad, after Heckbert. A point off
/// the quad maps outside 0 to 1. None for a degenerate quad, or a point past
/// its horizon.
pub fn unit_point(corners: &[[f32; 2]; 4], point: [f32; 2]) -> Option<[f32; 2]> {
    // Moved to the first corner and scaled to the quad's reach, so the
    // degeneracy checks hold the same at any place and size.
    let origin = corners[0].map(f64::from);
    let reach = corners
        .iter()
        .flat_map(|c| [f64::from(c[0]) - origin[0], f64::from(c[1]) - origin[1]])
        .fold(0., |reach: f64, d| reach.max(d.abs()));
    if !reach.is_finite() || reach <= 0. {
        return None;
    }
    let local = |p: [f32; 2]| {
        [
            (f64::from(p[0]) - origin[0]) / reach,
            (f64::from(p[1]) - origin[1]) / reach,
        ]
    };

    let inverse = invert(square_to_quad(corners.map(local))?)?;
    let [x, y] = local(point);
    let w = inverse[2][0] * x + inverse[2][1] * y + inverse[2][2];
    if w <= DEGENERATE {
        return None;
    }

    let u = (inverse[0][0] * x + inverse[0][1] * y + inverse[0][2]) / w;
    let v = (inverse[1][0] * x + inverse[1][1] * y + inverse[1][2]) / w;
    Some([u as f32, v as f32])
}

/// The point at `unit` across a quad, 0, 0 at its first corner and 1, 1 at
/// its third: [`unit_point`] the other way, so a part of a tilted face lands
/// where the face's own perspective puts it. None for a degenerate quad, or
/// a point past its horizon.
pub fn quad_point(corners: &[[f32; 2]; 4], unit: [f32; 2]) -> Option<[f32; 2]> {
    let origin = corners[0].map(f64::from);
    let reach = corners
        .iter()
        .flat_map(|c| [f64::from(c[0]) - origin[0], f64::from(c[1]) - origin[1]])
        .fold(0., |reach: f64, d| reach.max(d.abs()));
    if !reach.is_finite() || reach <= 0. {
        return None;
    }
    let local = |p: [f32; 2]| {
        [
            (f64::from(p[0]) - origin[0]) / reach,
            (f64::from(p[1]) - origin[1]) / reach,
        ]
    };

    let m = square_to_quad(corners.map(local))?;
    let [u, v] = unit.map(f64::from);
    let w = m[2][0] * u + m[2][1] * v + m[2][2];
    if w <= DEGENERATE {
        return None;
    }

    let x = (m[0][0] * u + m[0][1] * v + m[0][2]) / w;
    let y = (m[1][0] * u + m[1][1] * v + m[1][2]) / w;
    Some([
        (x * reach + origin[0]) as f32,
        (y * reach + origin[1]) as f32,
    ])
}

/// An item's size on screen at rest, in the pixels the canvas lays out in:
/// its box and its own scale through the parent's pixels per unit. Tilt,
/// the camera's perspective and what routes and sway do to it don't count,
/// so a render target sized from it holds still while its item moves.
pub fn rest_size(item: &CanvasItem, parent: &Frame) -> [f32; 2] {
    let [width, height] = item_size(item, parent);
    let k = parent.k * item.place.scale;

    [width * k, height * k]
}

/// Below this a quad scaled to unit reach has no area.
const DEGENERATE: f64 = 1e-9;

/// The unit square onto a quad, (u, v, 1) to (x, y, w).
fn square_to_quad([p0, p1, p2, p3]: [[f64; 2]; 4]) -> Option<[[f64; 3]; 3]> {
    let sx = p0[0] - p1[0] + p2[0] - p3[0];
    let sy = p0[1] - p1[1] + p2[1] - p3[1];

    let (g, h) = if sx.abs() < DEGENERATE && sy.abs() < DEGENERATE {
        (0., 0.)
    } else {
        let (dx1, dx2) = (p1[0] - p2[0], p3[0] - p2[0]);
        let (dy1, dy2) = (p1[1] - p2[1], p3[1] - p2[1]);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < DEGENERATE {
            return None;
        }
        ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
    };

    Some([
        [p1[0] - p0[0] + g * p1[0], p3[0] - p0[0] + h * p3[0], p0[0]],
        [p1[1] - p0[1] + g * p1[1], p3[1] - p0[1] + h * p3[1], p0[1]],
        [g, h, 1.],
    ])
}

fn invert(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let minor =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adjugate = [
        [minor(1, 1, 2, 2), -minor(0, 1, 2, 2), minor(0, 1, 1, 2)],
        [-minor(1, 0, 2, 2), minor(0, 0, 2, 2), -minor(0, 0, 1, 2)],
        [minor(1, 0, 2, 1), -minor(0, 0, 2, 1), minor(0, 0, 1, 1)],
    ];

    let det = m[0][0] * adjugate[0][0] + m[0][1] * adjugate[1][0] + m[0][2] * adjugate[2][0];
    if det.abs() < DEGENERATE {
        return None;
    }
    Some(adjugate.map(|row| row.map(|v| v / det)))
}

/// The move in the parent's units that shifts the item by `screen_delta`
/// pixels at its depth.
pub fn unproject_delta(
    item: &CanvasItem,
    parent: &Frame,
    camera: &Camera,
    viewport: Viewport,
    flat: bool,
    screen_delta: [f32; 2],
) -> [f32; 2] {
    let view = View::new(camera, viewport, None, parent.k);
    let size = item_size(item, parent);
    let world = parent.transform * local(item, parent, &Live::default(), flat);
    let center = world.point(center_offset(item, size));

    let Some((from, _)) = view.project(center) else {
        return [0., 0.];
    };
    let target = [from[0] + screen_delta[0], from[1] + screen_delta[1]];

    // A step of one unit along the parent's x and y, in world space. The
    // position enters the transform linearly, so the moved center is
    // `center + u * ax + v * ay`.
    let ax = parent.transform.vector([1., 0., 0.]);
    let ay = parent.transform.vector([0., 1., 0.]);

    // Projecting `center + u * ax + v * ay` onto `target` is linear in u and
    // v once both sides are multiplied by the depth term, so the solve is
    // exact, not a linearization.
    let (columns, rhs) = match view.focal {
        None => ([[ax[0], ax[1]], [ay[0], ay[1]]], screen_delta),

        Some(focal) => {
            let t = [target[0] - view.center[0], target[1] - view.center[1]];
            let column = |a: [f32; 3]| [focal * a[0] + t[0] * a[2], focal * a[1] + t[1] * a[2]];
            let rhs = [
                t[0] * (focal - center[2]) - focal * (center[0] - view.center[0]),
                t[1] * (focal - center[2]) - focal * (center[1] - view.center[1]),
            ];
            ([column(ax), column(ay)], rhs)
        }
    };

    let [a, b] = columns;
    let det = a[0] * b[1] - b[0] * a[1];

    // The parent is turned edge-on, so no move in its plane shows on screen.
    if det.abs() <= f32::EPSILON {
        return [0., 0.];
    }

    [
        (rhs[0] * b[1] - b[0] * rhs[1]) / det,
        (a[0] * rhs[1] - rhs[0] * a[1]) / det,
    ]
}

/// The camera for one frame: where it looks and how depth scales.
struct View {
    center: [f32; 2],
    /// None for an orthographic camera.
    focal: Option<f32>,
}

impl View {
    fn new(camera: &Camera, viewport: Viewport, pointer: Option<[f32; 2]>, k: f32) -> View {
        let mut center = viewport.center();

        if let Some(pointer) = pointer
            && camera.parallax > 0.
        {
            for axis in 0..2 {
                let half = center[axis];
                if half > 0. {
                    let offset = ((pointer[axis] - half) / half).clamp(-1., 1.);
                    center[axis] += camera.parallax * k * offset;
                }
            }
        }

        // Past 180 degrees the tangent turns over, so the widest view is held
        // just short of it.
        let focal = (camera.fov > 0.).then(|| {
            let half_fov = camera.fov.min(179.).to_radians() / 2.;
            (viewport.height / 2.) / half_fov.tan()
        });

        View { center, focal }
    }

    /// A world point to pixels, with its depth scale. None behind the
    /// camera.
    fn project(&self, p: [f32; 3]) -> Option<([f32; 2], f32)> {
        let scale = match self.focal {
            None => 1.,
            Some(focal) => {
                let distance = focal - p[2];
                if distance <= BEHIND {
                    return None;
                }
                focal / distance
            }
        };

        let [cx, cy] = self.center;
        Some(([cx + (p[0] - cx) * scale, cy + (p[1] - cy) * scale], scale))
    }
}

fn item_size(item: &CanvasItem, parent: &Frame) -> [f32; 2] {
    item.place.size_in(parent.size)
}

/// The item's top-left in its own space. Its anchor point is the origin.
fn box_origin(item: &CanvasItem, size: [f32; 2]) -> [f32; 2] {
    let [fx, fy] = item.place.anchor.fraction();
    [-fx * size[0], -fy * size[1]]
}

fn center_offset(item: &CanvasItem, size: [f32; 2]) -> [f32; 3] {
    let [left, top] = box_origin(item, size);
    [left + size[0] / 2., top + size[1] / 2., 0.]
}

/// Translate to the anchor's point plus the position, then rotate x then y
/// then z and scale about the item's centre. The anchor only places an item:
/// a corner-anchored one still turns about its middle. A flat item stops
/// after the translation.
fn local(item: &CanvasItem, parent: &Frame, live: &Live, flat: bool) -> Mat4 {
    let place = &item.place;
    let [fx, fy] = place.anchor.fraction();
    let offset = place.offset_in(parent.size);
    let at = [
        fx * parent.size[0] + offset[0] + live.position[0],
        fy * parent.size[1] + offset[1] + live.position[1],
        offset[2] + live.position[2],
    ];
    let moved = Mat4::translate(at);

    if flat {
        return moved;
    }

    let pivot = center_offset(item, item_size(item, parent));
    let back = [-pivot[0], -pivot[1], -pivot[2]];

    let turn = |axis: usize| place.rotation[axis] + live.rotation[axis];
    moved
        * Mat4::translate(pivot)
        * Mat4::rotate_x(turn(0))
        * Mat4::rotate_y(turn(1))
        * Mat4::rotate_z(turn(2))
        * Mat4::scale(place.scale * live.scale)
        * Mat4::translate(back)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::Position;
    use crate::state::{Anchor, Content, Length, Place};

    const VIEW: Viewport = Viewport {
        width: 800.,
        height: 600.,
    };

    const ORTHO: Camera = Camera {
        fov: 0.,
        parallax: 0.,
    };

    fn stage_frame() -> Frame {
        Frame::stage(&Stage::default(), VIEW)
    }

    fn item(anchor: Anchor, position: [f32; 3], size: [f32; 2]) -> CanvasItem {
        CanvasItem {
            place: Place {
                anchor,
                position: Position::units(position),
                size: [Length::Units(size[0]), Length::Units(size[1])],
                ..Place::default()
            },
            ..CanvasItem::default()
        }
    }

    fn group(position: [f32; 3], rotation: [f32; 3], size: [f32; 2]) -> CanvasItem {
        let mut group = item(Anchor::Center, position, size);
        group.place.rotation = rotation;
        group.content = Content::Group { items: Vec::new() };
        group
    }

    fn put(item: &CanvasItem, parent: &Frame, camera: &Camera, flat: bool) -> Projected {
        project(item, parent, camera, VIEW, None, flat, &Live::default())
            .expect("in front of the camera")
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    fn same(a: [[f32; 2]; 4], b: [[f32; 2]; 4]) -> bool {
        a.iter()
            .zip(b)
            .all(|(p, q)| near(p[0], q[0]) && near(p[1], q[1]))
    }

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> [[f32; 2]; 4] {
        [[left, top], [right, top], [right, bottom], [left, bottom]]
    }

    fn width(p: &Projected) -> f32 {
        p.corners[1][0] - p.corners[0][0]
    }

    /// The projected center: where the quad's diagonals cross, which a
    /// perspective projection keeps.
    fn center(corners: [[f32; 2]; 4]) -> [f32; 2] {
        let [p, _, r, _] = corners;
        let [_, q, _, s] = corners;
        let d1 = [r[0] - p[0], r[1] - p[1]];
        let d2 = [s[0] - q[0], s[1] - q[1]];
        let denom = d1[0] * d2[1] - d1[1] * d2[0];
        let t = ((q[0] - p[0]) * d2[1] - (q[1] - p[1]) * d2[0]) / denom;
        [p[0] + t * d1[0], p[1] + t * d1[1]]
    }

    #[test]
    fn an_item_turns_and_scales_about_its_centre_whatever_its_anchor() {
        let plain = item(Anchor::TopLeft, [40., 30., 0.], [100., 50.]);
        let before = put(&plain, &stage_frame(), &ORTHO, false);

        let mut turned = plain.clone();
        turned.place.rotation = [0., 0., 180.];
        turned.place.scale = 0.5;
        let after = put(&turned, &stage_frame(), &ORTHO, false);

        let [x, y] = center(before.corners);
        assert!(near(x, 90.) && near(y, 55.), "{x} {y}");
        let [tx, ty] = center(after.corners);
        assert!(
            near(tx, x) && near(ty, y),
            "the centre stays put: {tx} {ty}"
        );

        // Half the size, upside down, about the same middle.
        assert!(
            same(after.corners, rect(115., 67.5, 65., 42.5)),
            "{:?}",
            after.corners
        );
    }

    #[test]
    fn at_depth_zero_an_item_lands_one_to_one() {
        let p = put(
            &item(Anchor::TopLeft, [10., 20., 0.], [100., 50.]),
            &stage_frame(),
            &Camera::default(),
            false,
        );

        assert!(same(p.corners, rect(10., 20., 110., 70.)));
        assert!(near(p.scale, 1.));
        assert!(near(p.depth, 0.));
        assert!(near(p.opacity, 1.));
    }

    #[test]
    fn a_nearer_item_is_larger_and_a_farther_one_smaller() {
        let camera = Camera::default();
        let at = |z: f32| {
            put(
                &item(Anchor::Center, [0., 0., z], [100., 100.]),
                &stage_frame(),
                &camera,
                false,
            )
        };

        assert!(width(&at(100.)) > 100.);
        assert!(at(100.).scale > 1.);
        assert!(width(&at(-100.)) < 100.);
        assert!(at(-100.).scale < 1.);
    }

    #[test]
    fn an_orthographic_camera_ignores_depth() {
        let flat = put(
            &item(Anchor::Center, [0., 0., 0.], [100., 100.]),
            &stage_frame(),
            &ORTHO,
            false,
        );
        let deep = put(
            &item(Anchor::Center, [0., 0., 500.], [100., 100.]),
            &stage_frame(),
            &ORTHO,
            false,
        );

        assert!(same(flat.corners, deep.corners));
        assert!(near(deep.scale, 1.));
        assert!(near(deep.depth, 500.));
    }

    #[test]
    fn each_anchor_measures_from_its_own_point() {
        let anchors = [
            Anchor::TopLeft,
            Anchor::Top,
            Anchor::TopRight,
            Anchor::Left,
            Anchor::Center,
            Anchor::Right,
            Anchor::BottomLeft,
            Anchor::Bottom,
            Anchor::BottomRight,
        ];

        for anchor in anchors {
            let [fx, fy] = anchor.fraction();
            let p = put(
                &item(anchor, [5., 7., 0.], [100., 50.]),
                &stage_frame(),
                &ORTHO,
                false,
            );

            // The item's own anchor point, read off its top-left corner.
            let [left, top] = p.corners[0];
            let at = [left + fx * 100., top + fy * 50.];

            assert!(near(at[0], fx * VIEW.width + 5.), "{anchor:?} x");
            assert!(near(at[1], fy * VIEW.height + 7.), "{anchor:?} y");
        }
    }

    #[test]
    fn contain_fits_inside_and_cover_fills() {
        let viewport = Viewport {
            width: 960.,
            height: 1080.,
        };
        let mut whole = item(Anchor::Center, [0.; 3], [0., 0.]);
        whole.place.size = [Length::Stage(1.), Length::Stage(1.)];

        let mut stage = Stage {
            fit: Fit::Contain,
            ..Stage::default()
        };
        let contain = Frame::stage(&stage, viewport);
        let p = project(
            &whole,
            &contain,
            &ORTHO,
            viewport,
            None,
            false,
            &Live::default(),
        )
        .unwrap();

        assert!(near(contain.k, 0.5));
        assert_eq!(contain.size, [1920., 1080.]);
        assert!(same(p.corners, rect(0., 270., 960., 810.)));

        stage.fit = Fit::Cover;
        let cover = Frame::stage(&stage, viewport);
        let p = project(
            &whole,
            &cover,
            &ORTHO,
            viewport,
            None,
            false,
            &Live::default(),
        )
        .unwrap();

        assert!(near(cover.k, 1.));
        assert!(same(p.corners, rect(-480., 0., 1440., 1080.)));

        stage.fit = Fit::Anchor;
        let anchored = Frame::stage(&stage, viewport);
        assert_eq!(anchored.size, [960., 1080.]);
        assert!(near(anchored.k, 1.));
    }

    #[test]
    fn a_stage_length_follows_its_parent() {
        let mut half = item(Anchor::Center, [0.; 3], [0., 0.]);
        half.place.size = [Length::Stage(0.5), Length::Stage(1.)];

        let p = put(&half, &stage_frame(), &ORTHO, false);
        assert!(same(p.corners, rect(200., 0., 600., 600.)));

        let frame = child_frame(
            &group([0.; 3], [0.; 3], [200., 100.]),
            &stage_frame(),
            &Live::default(),
        );
        let p = put(&half, &frame, &ORTHO, false);
        assert!(same(p.corners, rect(350., 250., 450., 350.)));
    }

    #[test]
    fn a_fractional_position_follows_its_parent_and_a_limit_holds_the_size() {
        let mut item = item(Anchor::TopLeft, [0.; 3], [0., 0.]);
        item.place.position.xy = [Length::Stage(0.25), Length::Stage(0.5)];
        item.place.size = [Length::Stage(0.5), Length::Units(100.)];
        item.place.min_size[0] = Some(500.);

        let p = put(&item, &stage_frame(), &ORTHO, false);
        assert!(
            same(p.corners, rect(200., 300., 700., 400.)),
            "{:?}",
            p.corners
        );

        let wide = Viewport {
            width: 1600.,
            height: 900.,
        };
        let frame = Frame::stage(&Stage::default(), wide);
        let p = project(&item, &frame, &ORTHO, wide, None, false, &Live::default()).unwrap();
        assert!(
            same(p.corners, rect(400., 450., 1200., 550.)),
            "{:?}",
            p.corners
        );

        // In a group, the fraction is of the group's box.
        let frame = child_frame(
            &group([0.; 3], [0.; 3], [400., 200.]),
            &stage_frame(),
            &Live::default(),
        );
        let p = put(&item, &frame, &ORTHO, false);
        assert!(
            same(p.corners, rect(300., 300., 800., 400.)),
            "{:?}",
            p.corners
        );
    }

    #[test]
    fn projected_points_land_where_the_item_draws() {
        let mut tilted = item(Anchor::Center, [30., -20., 40.], [300., 200.]);
        tilted.place.rotation = [15., 30., 5.];
        let camera = Camera::default();

        let drawn = put(&tilted, &stage_frame(), &camera, false);
        let frame = child_frame(&tilted, &stage_frame(), &Live::default());
        let points = project_points(
            &frame,
            &camera,
            VIEW,
            None,
            [[0., 0.], [300., 0.], [300., 200.], [0., 200.]],
        )
        .expect("in front of the camera");

        assert!(
            same(points, drawn.corners),
            "{points:?} {:?}",
            drawn.corners
        );
    }

    #[test]
    fn a_child_moves_with_its_group() {
        let child = item(Anchor::Center, [0.; 3], [40., 40.]);
        let still = child_frame(
            &group([0.; 3], [0.; 3], [200., 200.]),
            &stage_frame(),
            &Live::default(),
        );
        let moved = child_frame(
            &group([50., 30., 0.], [0.; 3], [200., 200.]),
            &stage_frame(),
            &Live::default(),
        );

        let a = put(&child, &still, &ORTHO, false);
        let b = put(&child, &moved, &ORTHO, false);

        for (p, q) in a.corners.iter().zip(b.corners) {
            assert!(near(q[0] - p[0], 50.) && near(q[1] - p[1], 30.));
        }
    }

    #[test]
    fn a_child_of_a_turned_group_turns_with_it() {
        let frame = child_frame(
            &group([0.; 3], [0., 0., 90.], [200., 200.]),
            &stage_frame(),
            &Live::default(),
        );
        let p = put(
            &item(Anchor::Center, [50., 0., 0.], [40., 20.]),
            &frame,
            &ORTHO,
            false,
        );

        // A quarter turn about z takes the group's x to the screen's y, and
        // the child's own box turns with it.
        assert!(same(
            p.corners,
            [[410., 330.], [410., 370.], [390., 370.], [390., 330.]]
        ));
    }

    #[test]
    fn a_flat_child_of_a_turned_group_moves_and_stays_square() {
        let frame = child_frame(
            &group([0.; 3], [0., 0., 90.], [200., 200.]),
            &stage_frame(),
            &Live::default(),
        );
        let p = put(
            &item(Anchor::Center, [50., 0., 0.], [40., 20.]),
            &frame,
            &ORTHO,
            true,
        );

        assert!(same(p.corners, rect(380., 340., 420., 360.)));
    }

    #[test]
    fn a_groups_opacity_reaches_its_children() {
        let mut faded = group([0.; 3], [0.; 3], [200., 200.]);
        faded.opacity = 0.5;
        let live = Live {
            opacity: 0.5,
            ..Live::default()
        };
        let frame = child_frame(&faded, &stage_frame(), &live);

        let mut child = item(Anchor::Center, [0.; 3], [10., 10.]);
        child.opacity = 0.8;

        assert!(near(frame.opacity, 0.25));
        assert!(near(put(&child, &frame, &ORTHO, false).opacity, 0.2));
    }

    #[test]
    fn a_flat_item_keeps_its_size_and_ignores_rotation() {
        let mut tilted = item(Anchor::Center, [0., 0., 100.], [100., 50.]);
        tilted.place.rotation = [30., 40., 50.];
        tilted.place.scale = 2.;

        let p = put(&tilted, &stage_frame(), &Camera::default(), true);

        assert!(same(p.corners, rect(350., 275., 450., 325.)));
        assert!(p.scale > 1.);
        assert!(near(p.depth, 100.));
    }

    #[test]
    fn a_quarter_turn_about_y_narrows_to_a_line() {
        let mut turned = item(Anchor::Center, [0.; 3], [200., 100.]);
        turned.place.rotation = [0., 90., 0.];

        for camera in [Camera::default(), ORTHO] {
            let p = put(&turned, &stage_frame(), &camera, false);
            for [x, _] in p.corners {
                assert!(near(x, 400.), "{camera:?}: {x}");
            }
        }
    }

    #[test]
    fn an_item_behind_the_camera_is_skipped() {
        let camera = Camera::default();
        let behind = item(Anchor::Center, [0., 0., 5000.], [100., 100.]);

        for flat in [false, true] {
            assert!(
                project(
                    &behind,
                    &stage_frame(),
                    &camera,
                    VIEW,
                    None,
                    flat,
                    &Live::default()
                )
                .is_none()
            );
        }

        // The near edge of a turned item can cross the camera on its own.
        let focal = 300. / 17.5_f32.to_radians().tan();
        let mut crossing = item(Anchor::Center, [0., 0., focal - 50.], [400., 100.]);
        crossing.place.rotation = [0., 90., 0.];
        assert!(
            project(
                &crossing,
                &stage_frame(),
                &camera,
                VIEW,
                None,
                false,
                &Live::default()
            )
            .is_none()
        );
    }

    #[test]
    fn the_pointer_moves_near_items_more_than_far_ones() {
        let camera = Camera {
            parallax: 20.,
            ..Camera::default()
        };
        let shift = |z: f32| {
            let it = item(Anchor::Center, [0., 0., z], [50., 50.]);
            let rest = project(
                &it,
                &stage_frame(),
                &camera,
                VIEW,
                None,
                false,
                &Live::default(),
            )
            .unwrap();
            let moved = project(
                &it,
                &stage_frame(),
                &camera,
                VIEW,
                Some([800., 300.]),
                false,
                &Live::default(),
            )
            .unwrap();
            center(moved.corners)[0] - center(rest.corners)[0]
        };

        assert!(near(shift(0.), 0.));
        assert!(shift(-200.).abs() > 0.);
        assert!(shift(200.).abs() > shift(-200.).abs());
    }

    #[test]
    fn live_offsets_add_and_live_factors_multiply() {
        let mut base = item(Anchor::Center, [10., 0., 20.], [100., 50.]);
        base.place.rotation = [0., 10., 5.];
        base.place.scale = 2.;
        base.opacity = 0.5;

        let live = Live {
            position: [5., -3., 10.],
            rotation: [0., 20., 5.],
            scale: 1.5,
            opacity: 0.5,
        };

        let mut folded = base.clone();
        folded.place.position = Position::units([15., -3., 30.]);
        folded.place.rotation = [0., 30., 10.];
        folded.place.scale = 3.;

        let camera = Camera::default();
        let a = project(&base, &stage_frame(), &camera, VIEW, None, false, &live).unwrap();
        let b = put(&folded, &stage_frame(), &camera, false);

        assert!(same(a.corners, b.corners));
        assert!(near(a.depth, b.depth));
        assert!(near(a.opacity, 0.25));
    }

    #[test]
    fn paint_order_is_back_to_front_and_stable() {
        assert_eq!(paint_order(&[3., -1., 3., 0., -1.]), [1, 4, 3, 0, 2]);
        assert!(paint_order(&[]).is_empty());
    }

    #[test]
    fn inside_holds_its_edges_and_turned_quads() {
        let diamond = [[0., -10.], [10., 0.], [0., 10.], [-10., 0.]];

        assert!(inside(&diamond, [0., 0.]));
        assert!(inside(&diamond, [5., 5.]));
        assert!(!inside(&diamond, [8., 8.]));
    }

    /// An item turned on y and projected, so its quad is a real trapezoid.
    fn tilted_quad() -> [[f32; 2]; 4] {
        let mut tilted = item(Anchor::Center, [0., 0., 0.], [400., 240.]);
        tilted.place.rotation = [0., 35., 0.];
        put(&tilted, &stage_frame(), &Camera::default(), false).corners
    }

    fn near_point(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn the_corners_map_to_the_unit_square() {
        let quad = tilted_quad();
        let square = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];

        for (corner, unit) in quad.iter().zip(square) {
            let mapped = unit_point(&quad, *corner).expect("a real quad");
            assert!(near_point(mapped, unit), "{corner:?} -> {mapped:?}");
        }

        // Flat, it's a plain scale.
        let flat = [[100., 50.], [300., 50.], [300., 150.], [100., 150.]];
        let mapped = unit_point(&flat, [150., 125.]).expect("a real quad");
        assert!(near_point(mapped, [0.25, 0.75]), "{mapped:?}");
    }

    #[test]
    fn a_tilted_quads_centre_maps_inside() {
        let quad = tilted_quad();
        assert!(
            (quad[1][1] - quad[2][1]).abs() > (quad[0][1] - quad[3][1]).abs() + 1.
                || (quad[0][1] - quad[3][1]).abs() > (quad[1][1] - quad[2][1]).abs() + 1.,
            "the tilt foreshortens one side: {quad:?}"
        );

        // The diagonals cross at the item's own centre, which perspective
        // keeps off the quad's average of corners.
        let [a, b, c, d] = quad;
        let (r, s) = ([c[0] - a[0], c[1] - a[1]], [d[0] - b[0], d[1] - b[1]]);
        let t = ((b[0] - a[0]) * s[1] - (b[1] - a[1]) * s[0]) / (r[0] * s[1] - r[1] * s[0]);
        let crossing = [a[0] + t * r[0], a[1] + t * r[1]];

        let mapped = unit_point(&quad, crossing).expect("a real quad");
        assert!(near_point(mapped, [0.5, 0.5]), "{mapped:?}");
        assert!(inside(&quad, crossing));
    }

    #[test]
    fn a_point_outside_maps_outside() {
        let quad = tilted_quad();
        let [x, y] = center(quad);
        let width = quad[1][0] - quad[0][0];

        for point in [[x + width, y], [x - width, y], [x, y + 1000.]] {
            assert!(!inside(&quad, point));
            let [u, v] = unit_point(&quad, point).unwrap_or([f32::NAN; 2]);
            assert!(
                !(0. ..=1.).contains(&u) || !(0. ..=1.).contains(&v),
                "{point:?} -> {u}, {v}"
            );
        }
    }

    #[test]
    fn a_unit_point_goes_back_where_it_came_from() {
        let quad = tilted_quad();

        for unit in [[0., 0.], [1., 1.], [0.25, 0.75], [0.5, 0.5]] {
            let on = quad_point(&quad, unit).expect("a real quad");
            let back = unit_point(&quad, on).expect("on the quad");
            assert!(near_point(back, unit), "{unit:?} -> {on:?} -> {back:?}");
        }
        let corner = quad_point(&quad, [1., 0.]).unwrap();
        assert!(near_point(corner, quad[1]), "{corner:?}");
    }

    #[test]
    fn the_rest_size_ignores_tilt_and_routes() {
        let mut it = item(Anchor::Center, [0., 0., 0.], [200., 100.]);
        it.place.rotation = [0.4, 0.3, 0.];
        it.place.scale = 1.5;
        let parent = Frame {
            k: 2.,
            ..stage_frame()
        };

        assert_eq!(rest_size(&it, &parent), [600., 300.]);
    }

    #[test]
    fn a_flat_line_of_corners_maps_nowhere() {
        let line = [[0., 0.], [10., 0.], [20., 0.], [30., 0.]];
        assert_eq!(unit_point(&line, [5., 0.]), None);

        let folded = [[0., 0.], [10., 10.], [20., 20.], [10., 10.]];
        assert_eq!(unit_point(&folded, [5., 5.]), None);

        let point = [[4., 4.]; 4];
        assert_eq!(unit_point(&point, [4., 4.]), None);
    }

    #[test]
    fn unproject_undoes_project() {
        let camera = Camera::default();
        let drag = [30., -20.];

        let check = |it: &CanvasItem, parent: &Frame, flat: bool| {
            let before = center(put(it, parent, &camera, flat).corners);
            let [dx, dy] = unproject_delta(it, parent, &camera, VIEW, flat, drag);

            let mut moved = it.clone();
            moved.place.position = moved.place.position.shifted([dx, dy], parent.size);
            let after = center(put(&moved, parent, &camera, flat).corners);

            assert!(
                near(after[0] - before[0], drag[0]),
                "{flat}: x {before:?} -> {after:?}"
            );
            assert!(
                near(after[1] - before[1], drag[1]),
                "{flat}: y {before:?} -> {after:?}"
            );
        };

        let mut tilted = item(Anchor::TopLeft, [40., 30., 80.], [120., 60.]);
        tilted.place.rotation = [10., 25., 0.];

        check(&tilted, &stage_frame(), false);
        check(&tilted, &stage_frame(), true);

        let turned = child_frame(
            &group([60., -40., 50.], [20., 35., 30.], [300., 200.]),
            &stage_frame(),
            &Live::default(),
        );
        check(&tilted, &turned, false);
        check(&tilted, &turned, true);
    }
}
