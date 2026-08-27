//! One card's geometry, laid off a u64 seed: deterministic procedural tile
//! art, quiet enough to sit under a label. The genre wall and the stats
//! window both draw it, so a seed draws the same geometry wherever it
//! turns up.

use gpui::{
    canvas, point, prelude::*, px, quad, size, transparent_black, AnyElement, BorderStyle, Bounds,
    Pixels, Window,
};
use rox_design::palette;

/// Sixteen motifs built from quads alone (a circle is a full-corner quad,
/// a ring a border-only one), one canvas layer painted under the caller's
/// content.
///
/// Beyond the motif pick, the seed places the layout in a corner, scales
/// it a touch, and picks a symmetry: the motif alone, its mirror twin
/// across either axis, or all four reflections at once, which folds a
/// single shape into a pattern. The ink thins as the reflections multiply,
/// so a four-fold card shows more geometry without more weight.
/// `ink` comes in solid; the alpha is this function's to set. The caller's
/// overflow_hidden clips the bleed.
pub fn motif(seed: u64, ink: gpui::Rgba) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            let side = f32::from(bounds.size.width.min(bounds.size.height));
            if side <= 0. {
                return;
            }
            // The per-genre variation beyond the motif itself: mirror
            // bits for each axis, a size jitter of 0.85 to 1.15, and the
            // symmetry pick.
            let flip_x = (seed >> 33) & 1 == 0;
            let flip_y = (seed >> 34) & 1 == 0;
            let scale = 0.85 + ((seed >> 37) % 32) as f32 / 32.0 * 0.30;
            // Which reflections paint: the base placement always, then
            // per symmetry its twin across x, across y, or both plus the
            // diagonal to close the pattern.
            // Symmetry stays the exception: five of eight seeds paint
            // the motif alone, the mirrored pairs and the four-fold
            // each take one. A wall full of symmetric cards is itself
            // a pattern, and the eye finds it.
            let passes: &[(bool, bool)] = match (seed >> 42) % 8 {
                0..=4 => &[(false, false)],
                5 => &[(false, false), (true, false)],
                6 => &[(false, false), (false, true)],
                _ => &[(false, false), (true, false), (false, true), (true, true)],
            };
            let ink = palette::alpha(
                ink,
                match passes.len() {
                    1 => 0x1A,
                    2 => 0x14,
                    _ => 0x0F,
                },
            );
            let pick = (seed >> 13) % 16;
            // The arrangement's rotation, in 15-degree steps around the
            // tile center. Quads can't rotate, but circles don't care:
            // every disc- and ring-built motif spins its center points,
            // which frees those layouts from the four corners entirely.
            // Edge-anchored compositions (pills, bars, rules, the big
            // corner square) stay square to the tile they hang off.
            let theta: f32 = match pick {
                3 | 10 | 11 | 13 => 0.,
                _ => ((seed >> 55) % 24) as f32 * std::f32::consts::PI / 12.,
            };
            let spin = move |cx: f32, cy: f32| -> (f32, f32) {
                if theta == 0. {
                    return (cx, cy);
                }
                let (sin, cos) = theta.sin_cos();
                let (dx, dy) = (cx - 0.5, cy - 0.5);
                (0.5 + dx * cos - dy * sin, 0.5 + dx * sin + dy * cos)
            };
            // A rounded rect centered at (cx, cy), everything in tile
            // fractions; radius at half height makes discs and pills.
            let shape = |cx: f32, cy: f32, w: f32, h: f32, r: f32, window: &mut Window| {
                let (cx, cy) = spin(cx, cy);
                let (w, h) = (side * w * scale, side * h * scale);
                let origin = point(
                    bounds.origin.x + px(side * cx - w / 2.),
                    bounds.origin.y + px(side * cy - h / 2.),
                );
                window.paint_quad(quad(
                    Bounds::new(origin, size(px(w), px(h))),
                    px(h * r),
                    ink,
                    px(0.),
                    transparent_black(),
                    BorderStyle::default(),
                ));
            };
            let disc = |cx: f32, cy: f32, d: f32, window: &mut Window| {
                shape(cx, cy, d, d, 0.5, window);
            };
            let ring = |cx: f32, cy: f32, d: f32, stroke: f32, window: &mut Window| {
                let (cx, cy) = spin(cx, cy);
                let d = side * d * scale;
                let origin = point(
                    bounds.origin.x + px(side * cx - d / 2.),
                    bounds.origin.y + px(side * cy - d / 2.),
                );
                window.paint_quad(quad(
                    Bounds::new(origin, size(px(d), px(d))),
                    px(d / 2.),
                    transparent_black(),
                    px((side * stroke * scale).max(1.5)),
                    ink,
                    BorderStyle::default(),
                ));
            };
            // Each reflection re-paints the motif with the axes folded:
            // the effective flips are the base placement XOR the pass's
            // mirrors, so the twin ends up exactly opposite its original.
            for &(mx, my) in passes {
                let ex = flip_x != mx;
                let ey = flip_y != my;
                let x = |f: f32| if ex { f } else { 1. - f };
                let y = |f: f32| if ey { f } else { 1. - f };
                // Bits 13-16, never a `% 8` or `% 16` of the raw seed:
                // both divide 360, so that would be the hue's own low
                // bits and same-colored cards would always share a motif,
                // the stamped-twin look this field exists to prevent.
                match pick {
                    // A disc bleeding past a corner.
                    0 => disc(x(0.9), y(0.86), 1.0, window),
                    // Two concentric rings.
                    1 => {
                        ring(x(0.76), y(0.3), 0.66, 0.018, window);
                        ring(x(0.76), y(0.3), 0.36, 0.018, window);
                    }
                    // A diagonal run of shrinking dots.
                    2 => {
                        disc(x(0.8), y(0.22), 0.3, window);
                        disc(x(0.6), y(0.48), 0.18, window);
                        disc(x(0.44), y(0.68), 0.1, window);
                    }
                    // Two pills running off one edge.
                    3 => {
                        let pill = |cy: f32, len: f32, window: &mut Window| {
                            let cx = if ex { 1.05 - len / 2. } else { len / 2. - 0.05 };
                            shape(cx, cy, len, 0.14, 0.5, window);
                        };
                        pill(y(0.24), 0.75, window);
                        pill(y(0.78), 0.55, window);
                    }
                    // One wide halo off a corner, a heavier stroke than
                    // the ring pair so it reads at its size.
                    4 => ring(x(0.94), y(0.12), 1.1, 0.05, window),
                    // Two discs overlapping into a venn; the ink stacks
                    // where they cross, which is the point.
                    5 => {
                        disc(x(0.62), y(0.76), 0.42, window);
                        disc(x(0.86), y(0.76), 0.42, window);
                    }
                    // Dots along a quarter arc around a corner.
                    6 => {
                        let (ccx, ccy) = (x(1.0), y(0.0));
                        for i in 0..5 {
                            let angle = std::f32::consts::FRAC_PI_2 * (0.12 + i as f32 * 0.19);
                            let (dx, dy) = (angle.cos() * 0.62, angle.sin() * 0.62);
                            let cx = if ex { ccx - dx } else { ccx + dx };
                            let cy = if ey { ccy + dy } else { ccy - dy };
                            disc(cx, cy, 0.09, window);
                        }
                    }
                    // Three rounded squares stepping down a diagonal.
                    7 => {
                        shape(x(0.78), y(0.2), 0.26, 0.26, 0.2, window);
                        shape(x(0.55), y(0.44), 0.19, 0.19, 0.2, window);
                        shape(x(0.37), y(0.63), 0.13, 0.13, 0.2, window);
                    }
                    // A ring with a small disc on its rim, an orbit.
                    8 => {
                        ring(x(0.7), y(0.35), 0.6, 0.02, window);
                        disc(x(0.49), y(0.14), 0.11, window);
                    }
                    // A 3x3 block of dots in a corner.
                    9 => {
                        for i in 0..3 {
                            for j in 0..3 {
                                disc(
                                    x(0.62 + i as f32 * 0.16),
                                    y(0.18 + j as f32 * 0.16),
                                    0.07,
                                    window,
                                );
                            }
                        }
                    }
                    // Three bars rising off an edge, the equalizer.
                    10 => {
                        for (i, h) in [(0., 0.38), (1., 0.6), (2., 0.28)] {
                            shape(x(0.22 + i * 0.16), y(1.04 - h / 2.), 0.1, h, 0.5, window);
                        }
                    }
                    // Three thin full-width rules.
                    11 => {
                        shape(0.5, y(0.24), 1.2, 0.045, 0.5, window);
                        shape(0.5, y(0.4), 1.2, 0.045, 0.5, window);
                        shape(0.5, y(0.56), 1.2, 0.045, 0.5, window);
                    }
                    // A bullseye: a ring around its own solid center.
                    12 => {
                        ring(x(0.76), y(0.7), 0.5, 0.018, window);
                        disc(x(0.76), y(0.7), 0.16, window);
                    }
                    // One big rounded square bleeding past a corner, the
                    // disc's blunter sibling.
                    13 => shape(x(0.88), y(0.84), 0.8, 0.8, 0.18, window),
                    // Two rounded squares on opposite corners; four-fold
                    // symmetry folds these into a checker.
                    14 => {
                        shape(x(0.25), y(0.25), 0.28, 0.28, 0.2, window);
                        shape(x(0.75), y(0.75), 0.28, 0.28, 0.2, window);
                    }
                    // A run of beads along an edge.
                    _ => {
                        for i in 0..4 {
                            disc(x(0.2 + i as f32 * 0.2), y(0.9), 0.09, window);
                        }
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
