//! Classic 3D Perlin noise, hand-rolled. The prototype needs a smooth scalar
//! field to take a curl from; which noise library to depend on is a decision
//! for the real visualizer, not for this measurement.

pub struct Perlin {
    perm: [u8; 512],
}

impl Perlin {
    pub fn new(seed: u64) -> Self {
        let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut s = seed | 1;
        for i in (1..256).rev() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            table.swap(i, (s as usize) % (i + 1));
        }
        let perm = std::array::from_fn(|i| table[i & 255]);
        Self { perm }
    }

    fn fade(t: f32) -> f32 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + t * (b - a)
    }

    fn grad(hash: u8, x: f32, y: f32, z: f32) -> f32 {
        let h = hash & 15;
        let u = if h < 8 { x } else { y };
        let v = if h < 4 {
            y
        } else if h == 12 || h == 14 {
            x
        } else {
            z
        };
        (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
    }

    /// Scalar noise in roughly [-1, 1].
    pub fn noise(&self, x: f32, y: f32, z: f32) -> f32 {
        let (fx, fy, fz) = (x.floor(), y.floor(), z.floor());
        let (xi, yi, zi) = (
            fx as i32 as usize & 255,
            fy as i32 as usize & 255,
            fz as i32 as usize & 255,
        );
        let (x, y, z) = (x - fx, y - fy, z - fz);
        let (u, v, w) = (Self::fade(x), Self::fade(y), Self::fade(z));

        let p = &self.perm;
        let a = p[xi] as usize + yi;
        let aa = p[a] as usize + zi;
        let ab = p[a + 1] as usize + zi;
        let b = p[xi + 1] as usize + yi;
        let ba = p[b] as usize + zi;
        let bb = p[b + 1] as usize + zi;

        Self::lerp(
            Self::lerp(
                Self::lerp(
                    Self::grad(p[aa], x, y, z),
                    Self::grad(p[ba], x - 1.0, y, z),
                    u,
                ),
                Self::lerp(
                    Self::grad(p[ab], x, y - 1.0, z),
                    Self::grad(p[bb], x - 1.0, y - 1.0, z),
                    u,
                ),
                v,
            ),
            Self::lerp(
                Self::lerp(
                    Self::grad(p[aa + 1], x, y, z - 1.0),
                    Self::grad(p[ba + 1], x - 1.0, y, z - 1.0),
                    u,
                ),
                Self::lerp(
                    Self::grad(p[ab + 1], x, y - 1.0, z - 1.0),
                    Self::grad(p[bb + 1], x - 1.0, y - 1.0, z - 1.0),
                    u,
                ),
                v,
            ),
            w,
        )
    }

    /// 2D curl of the scalar field at (x, y), with z as evolution time.
    /// Divergence-free by construction, which is what makes it read as flow.
    pub fn curl(&self, x: f32, y: f32, z: f32) -> (f32, f32) {
        const E: f32 = 0.01;
        let dy = (self.noise(x, y + E, z) - self.noise(x, y - E, z)) / (2.0 * E);
        let dx = (self.noise(x + E, y, z) - self.noise(x - E, y, z)) / (2.0 * E);
        (dy, -dx)
    }
}
