//! Small chart elements over gpui's paint primitives, shared by
//! whichever views want one (the stats window today). A bar chart over
//! counts with a hover pick, and a donut of fractional slices. Both draw
//! with quads and path fans inside a canvas - cheap at any plausible
//! size - and stay palette-agnostic: the caller passes colors, so they
//! ride panel and song theming wherever they land. Text stays out of the
//! paint closures (labels need the text system); the caller reads the
//! hover pick back and writes its own readout.

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, Bounds, Context, Div, MouseMoveEvent, Path,
    Pixels, Rgba, Stateful,
};

use crate::design::palette;

/// The hover state a bar chart shares between paint, which knows the
/// chart's bounds, and the mouse handlers, which know the pointer: the
/// hovered bucket's index, for the caller's readout. Behind Arcs so the
/// paint closure, the handlers, and the owning view all hold it.
#[derive(Clone, Default)]
pub struct BarHover {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    index: Arc<Mutex<Option<usize>>>,
}

impl BarHover {
    /// The hovered bucket, None with the pointer off the chart.
    pub fn index(&self) -> Option<usize> {
        *self.index.lock().unwrap()
    }
}

/// A bar chart over the counts: one bar per bucket, heights against the
/// busiest, each colored along the `lo` to `hi` ramp by its own height,
/// so the busy stretches read at a glance. Hovering washes the bucket's
/// column, recolors its bar to `pick`, and reports the index through
/// `hover`; the caller sizes the returned element and renders any
/// readout itself.
pub fn bars<V: 'static>(
    values: Vec<u64>,
    hover: &BarHover,
    lo: Rgba,
    hi: Rgba,
    pick: Rgba,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let count = values.len();
    let moved = hover.clone();
    let left = hover.clone();
    let paint = hover.clone();
    div()
        // The id makes the element stateful, which hover tracking needs.
        .id("bar-chart")
        .size_full()
        .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            let Some(bounds) = *moved.bounds.lock().unwrap() else {
                return;
            };
            let x = f32::from(event.position.x) - f32::from(bounds.origin.x);
            let w = f32::from(bounds.size.width);
            let index = (count > 0 && w > 0.0 && (0.0..w).contains(&x))
                .then(|| ((x / w * count as f32) as usize).min(count - 1));
            let mut current = moved.index.lock().unwrap();
            if *current != index {
                *current = index;
                cx.notify();
            }
        }))
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            if !hovered && left.index.lock().unwrap().take().is_some() {
                cx.notify();
            }
        }))
        .child(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                *paint.bounds.lock().unwrap() = Some(bounds);
                let picked = *paint.index.lock().unwrap();
                paint_bars(&values, picked, lo, hi, pick, bounds, window);
            })
            .size_full(),
        )
}

/// The bars into their bounds, a hairline gap once they are wide enough
/// to afford one; the hovered bucket gets a full-height wash behind its
/// bar so even an empty one marks the pick.
fn paint_bars(
    values: &[u64],
    picked: Option<usize>,
    lo: Rgba,
    hi: Rgba,
    pick: Rgba,
    bounds: Bounds<Pixels>,
    window: &mut gpui::Window,
) {
    let peak = values.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return;
    }
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let x0 = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let step = w / values.len() as f32;
    let gap = if step >= 3.0 { 1.0 } else { 0.0 };
    if let Some(ix) = picked {
        window.paint_quad(fill(
            Bounds::new(
                point(px(x0 + ix as f32 * step), px(top)),
                size(px((step - gap).max(1.0)), px(h)),
            ),
            palette::alpha(pick, 0x14),
        ));
    }
    for (i, &count) in values.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let t = count as f32 / peak as f32;
        // Even one listen gets a visible sliver.
        let height = (t * h).max(2.0);
        let color = if picked == Some(i) {
            pick
        } else {
            palette::mix(lo, hi, t)
        };
        window.paint_quad(fill(
            Bounds::new(
                point(px(x0 + i as f32 * step), px(top + h - height)),
                size(px((step - gap).max(1.0)), px(height)),
            ),
            color,
        ));
    }
}

/// A donut of fractional slices, clockwise from noon, each its own
/// color; the caller sizes the returned element square. Fractions are
/// of the whole ring, so an under-full set leaves a gap rather than
/// stretching.
pub fn donut(slices: Vec<(f32, Rgba)>) -> Div {
    div().size_full().child(
        canvas(|_, _, _| {}, move |bounds, _, window, _| {
            paint_donut(&slices, bounds, window);
        })
        .size_full(),
    )
}

/// The donut's slices into their bounds: ring segments as triangle fans
/// between the inner and outer radius.
fn paint_donut(slices: &[(f32, Rgba)], bounds: Bounds<Pixels>, window: &mut gpui::Window) {
    let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
    let center = bounds.center();
    let (cx, cy) = (f32::from(center.x), f32::from(center.y));
    let outer = side / 2.0;
    let inner = outer * 0.62;
    let mut angle = -std::f32::consts::FRAC_PI_2;
    for &(fraction, color) in slices {
        let sweep = fraction.clamp(0.0, 1.0) * std::f32::consts::TAU;
        if sweep <= 0.0 {
            continue;
        }
        // Enough steps that the arc reads round at this size.
        let steps = ((sweep / std::f32::consts::TAU * 96.0).ceil() as usize).max(2);
        let at = |a: f32, r: f32| point(px(cx + a.cos() * r), px(cy + a.sin() * r));
        let mut path = Path::new(at(angle, outer));
        for i in 0..steps {
            let a0 = angle + sweep * i as f32 / steps as f32;
            let a1 = angle + sweep * (i + 1) as f32 / steps as f32;
            let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
            path.push_triangle((at(a0, outer), at(a0, inner), at(a1, outer)), solid);
            path.push_triangle((at(a0, inner), at(a1, outer), at(a1, inner)), solid);
        }
        window.paint_path(path, color);
        angle += sweep;
    }
}
