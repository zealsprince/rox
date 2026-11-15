//! A 4x4 affine transform, row-major, acting on column vectors: `a * b`
//! applies `b` first. Small enough that it isn't worth a math dependency.

use std::ops::Mul;

/// Row-major. The crate's own, so it takes no math dependency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f32; 4]; 4]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]);

    pub fn translate(v: [f32; 3]) -> Mat4 {
        let mut m = Self::IDENTITY;
        m.0[0][3] = v[0];
        m.0[1][3] = v[1];
        m.0[2][3] = v[2];
        m
    }

    pub fn scale(s: f32) -> Mat4 {
        let mut m = Self::IDENTITY;
        m.0[0][0] = s;
        m.0[1][1] = s;
        m.0[2][2] = s;
        m
    }

    pub fn rotate_x(degrees: f32) -> Mat4 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Mat4([
            [1., 0., 0., 0.],
            [0., cos, -sin, 0.],
            [0., sin, cos, 0.],
            [0., 0., 0., 1.],
        ])
    }

    pub fn rotate_y(degrees: f32) -> Mat4 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Mat4([
            [cos, 0., sin, 0.],
            [0., 1., 0., 0.],
            [-sin, 0., cos, 0.],
            [0., 0., 0., 1.],
        ])
    }

    pub fn rotate_z(degrees: f32) -> Mat4 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Mat4([
            [cos, -sin, 0., 0.],
            [sin, cos, 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ])
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        let row = |r: usize| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
        [row(0), row(1), row(2)]
    }

    /// A direction: the linear part only, translation left out.
    pub fn vector(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        let row = |r: usize| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2];
        [row(0), row(1), row(2)]
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul for Mat4 {
    type Output = Mat4;

    fn mul(self, rhs: Mat4) -> Mat4 {
        let mut out = [[0.; 4]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|i| self.0[r][i] * rhs.0[i][c]).sum();
            }
        }
        Mat4(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn the_right_hand_factor_applies_first() {
        let m = Mat4::translate([10., 0., 0.]) * Mat4::scale(2.);

        assert!(close(m.point([1., 1., 1.]), [12., 2., 2.]));
        assert!(close(m.vector([1., 0., 0.]), [2., 0., 0.]));
    }

    #[test]
    fn a_quarter_turn_about_z_takes_x_to_y() {
        assert!(close(Mat4::rotate_z(90.).point([1., 0., 0.]), [0., 1., 0.]));
        assert!(close(Mat4::rotate_y(90.).point([0., 0., 1.]), [1., 0., 0.]));
        assert!(close(Mat4::rotate_x(90.).point([0., 1., 0.]), [0., 0., 1.]));
    }
}
