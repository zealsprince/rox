//! Nine-slice geometry: an image cut by four insets into corners, edges
//! and a middle, laid on a face of any size. A corner keeps its size, an
//! edge stretches along its side, and the middle stretches both ways. An
//! inset is image pixels and draws as that many of the item's units, so a
//! corner follows the item's scale and the stage's, never the stretch.
//!
//! When the face is smaller than two corners side by side, every inset
//! shrinks by the same factor, so the corners keep their shape and meet.
//! Pure, so the patches test without a window.

/// Left, top, right, bottom.
pub(crate) type Insets = [u32; 4];

/// One of the nine pieces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Patch {
    /// The column and row, 0 to 2: which of the face's grid lines bound it.
    pub cell: [usize; 2],
    /// The texels it samples: left, top, width, height.
    pub texels: [u32; 4],
    /// The sides (left, top, right, bottom) another patch meets.
    pub inner: [bool; 4],
}

/// The cut: the face's grid lines in its units from its top-left, and the
/// patches between them. Patches with no texels or no area are left out.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slices {
    pub xs: [f32; 4],
    pub ys: [f32; 4],
    pub patches: Vec<Patch>,
}

/// Insets that fit the image: the two on a side leave at least one texel
/// between them, given back in proportion when they don't.
pub(crate) fn held(insets: Insets, image: [u32; 2]) -> Insets {
    let side = |near: u32, far: u32, length: u32| {
        let room = length.saturating_sub(1);
        let sum = u64::from(near) + u64::from(far);
        if sum <= u64::from(room) {
            return (near, far);
        }

        let near = (u64::from(near) * u64::from(room) / sum) as u32;
        (near, room - near)
    };

    let (left, right) = side(insets[0], insets[2], image[0]);
    let (top, bottom) = side(insets[1], insets[3], image[1]);
    [left, top, right, bottom]
}

/// The cut of an `image` (pixels) by `insets` on a face of `face` units.
pub(crate) fn cut(insets: Insets, image: [u32; 2], face: [f32; 2]) -> Slices {
    let [left, top, right, bottom] = held(insets, image);

    // One factor for both axes keeps a corner's shape when it shrinks.
    let fits = |near: u32, far: u32, length: f32| match near + far {
        0 => 1.,
        sum => (length / sum as f32).clamp(0., 1.),
    };
    let k = fits(left, right, face[0]).min(fits(top, bottom, face[1]));

    let lines =
        |near: u32, far: u32, length: f32| [0., near as f32 * k, length - far as f32 * k, length];
    let xs = lines(left, right, face[0].max(0.));
    let ys = lines(top, bottom, face[1].max(0.));

    let texel_lines = |near: u32, far: u32, length: u32| [0, near, length - far, length];
    let tx = texel_lines(left, right, image[0]);
    let ty = texel_lines(top, bottom, image[1]);

    let present = |lines: &[f32; 4], texels: &[u32; 4], i: usize| {
        lines[i + 1] > lines[i] && texels[i + 1] > texels[i]
    };
    let columns: Vec<bool> = (0..3).map(|i| present(&xs, &tx, i)).collect();
    let rows: Vec<bool> = (0..3).map(|i| present(&ys, &ty, i)).collect();

    let before = |shown: &[bool], i: usize| shown[..i].iter().any(|s| *s);
    let after = |shown: &[bool], i: usize| shown[i + 1..].iter().any(|s| *s);

    let mut patches = Vec::new();
    for row in (0..3).filter(|r| rows[*r]) {
        for column in (0..3).filter(|c| columns[*c]) {
            patches.push(Patch {
                cell: [column, row],
                texels: [
                    tx[column],
                    ty[row],
                    tx[column + 1] - tx[column],
                    ty[row + 1] - ty[row],
                ],
                inner: [
                    before(&columns, column),
                    before(&rows, row),
                    after(&columns, column),
                    after(&rows, row),
                ],
            });
        }
    }

    Slices { xs, ys, patches }
}

/// A texel count as the 0 to 1 fraction `paint_image_quad` takes for its
/// source. It multiplies back by the image's side and truncates to whole
/// texels, so the quarter texel keeps an exact count from rounding one
/// down.
pub(crate) fn fraction(texels: u32, side: u32) -> f32 {
    if side == 0 {
        return 0.;
    }

    (texels as f32 + 0.25) / side as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(lines: [f32; 4]) -> [f32; 3] {
        [
            lines[1] - lines[0],
            lines[2] - lines[1],
            lines[3] - lines[2],
        ]
    }

    #[test]
    fn corners_keep_their_size_and_the_middle_takes_the_rest() {
        let slices = cut([16, 12, 16, 12], [64, 48], [400., 100.]);

        assert_eq!(widths(slices.xs), [16., 368., 16.]);
        assert_eq!(widths(slices.ys), [12., 76., 12.]);
        assert_eq!(slices.patches.len(), 9);

        let corner = slices.patches[0];
        assert_eq!(corner.cell, [0, 0]);
        assert_eq!(corner.texels, [0, 0, 16, 12]);
        assert_eq!(corner.inner, [false, false, true, true]);

        let middle = slices.patches[4];
        assert_eq!(middle.cell, [1, 1]);
        assert_eq!(middle.texels, [16, 12, 32, 24]);
        assert_eq!(middle.inner, [true; 4]);

        let bottom_right = slices.patches[8];
        assert_eq!(bottom_right.texels, [48, 36, 16, 12]);
        assert_eq!(bottom_right.inner, [true, true, false, false]);
    }

    #[test]
    fn every_texel_is_drawn_once() {
        let slices = cut([5, 7, 9, 3], [40, 30], [123.5, 77.25]);
        let mut seen = vec![0; 40 * 30];

        for patch in &slices.patches {
            let [x, y, w, h] = patch.texels;
            for ty in y..y + h {
                for tx in x..x + w {
                    seen[(ty * 40 + tx) as usize] += 1;
                }
            }
        }

        assert!(seen.iter().all(|n| *n == 1));
    }

    #[test]
    fn the_face_is_covered_edge_to_edge_with_no_gap() {
        for face in [[300., 200.], [33.3, 47.9], [12., 900.]] {
            let slices = cut([10, 10, 20, 5], [64, 64], face);

            assert_eq!(slices.xs[0], 0.);
            assert_eq!(slices.xs[3], face[0]);
            assert_eq!(slices.ys[3], face[1]);
            assert!(slices.xs.windows(2).all(|w| w[1] >= w[0]), "{slices:?}");
            assert!(slices.ys.windows(2).all(|w| w[1] >= w[0]), "{slices:?}");
        }
    }

    #[test]
    fn a_face_smaller_than_its_corners_shrinks_them_together() {
        // 20 wide holds half the 40 the corners ask for, so every inset
        // halves, the tall axis's included, and the middle column is gone.
        let slices = cut([20, 10, 20, 10], [64, 64], [20., 300.]);

        assert_eq!(widths(slices.xs), [10., 0., 10.]);
        assert_eq!(widths(slices.ys), [5., 290., 5.]);
        assert_eq!(slices.patches.len(), 6);
        assert!(slices.patches.iter().all(|p| p.cell[0] != 1));

        // The two columns still meet, and their seam is still inner.
        let left = slices.patches.iter().find(|p| p.cell == [0, 1]).unwrap();
        assert_eq!(left.inner, [false, true, true, true]);
    }

    #[test]
    fn a_zero_inset_drops_its_row_and_leaves_no_inner_side_there() {
        let slices = cut([0, 8, 0, 8], [32, 32], [200., 100.]);

        assert_eq!(slices.patches.len(), 3);
        for patch in &slices.patches {
            assert_eq!(patch.cell[0], 1);
            assert!(!patch.inner[0] && !patch.inner[2], "{patch:?}");
        }
        assert_eq!(slices.patches[1].inner, [false, true, false, true]);
    }

    #[test]
    fn insets_past_the_image_are_given_back_in_proportion() {
        assert_eq!(held([30, 0, 30, 0], [40, 10]), [19, 0, 20, 0]);
        assert_eq!(held([5, 5, 5, 5], [40, 10]), [5, 4, 5, 5]);
        assert_eq!(held([5, 5, 5, 5], [1, 1]), [0; 4]);

        // A middle texel is always left to stretch.
        let slices = cut([30, 0, 30, 0], [40, 10], [500., 10.]);
        assert_eq!(slices.patches.len(), 3);
    }

    #[test]
    fn a_source_fraction_truncates_back_to_its_texels() {
        for side in 1..=4096_u32 {
            for texels in [0, 1, side / 3, side / 2, side - 1, side] {
                let back = (fraction(texels, side) * side as f32) as u32;
                assert_eq!(back, texels, "{texels} of {side}");
            }
        }
    }
}
