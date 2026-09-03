//! Small chart elements over gpui's paint primitives, shared by
//! whichever views need one: a bar chart over counts with a hover pick
//! (the stats window), and a ring showing one share of a whole (the
//! health window's overview). They draw with quads and paths inside a
//! canvas, cheap at any plausible size, and stay palette-agnostic: the
//! caller passes colors, so they pick up panel and song theming wherever
//! they're used. Text stays out of the paint closure (labels need the
//! text system); the caller reads the hover pick back and writes its own
//! readout, and lays its own number over the ring's hole.

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, Bounds, ClickEvent, Context, Div,
    MouseMoveEvent, Path, PathBuilder, Pixels, Point, Rgba, Stateful,
};

use rox_design::palette;

/// The hover state a bar chart shares between paint, which has the
/// chart's bounds, and the mouse handlers, which have the pointer: the
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

    /// Drop the pick, for a chart whose buckets are about to change under
    /// a still pointer.
    pub fn clear(&self) {
        *self.index.lock().unwrap() = None;
    }
}

/// A bar chart over the counts: one bar per bucket, heights against the
/// busiest, each colored along the `lo` to `hi` ramp by its own height,
/// so the busy stretches read at a glance. Hovering washes the bucket's
/// column, recolors its bar to `pick`, and reports the index through
/// `hover`; a click hands the hovered bucket to `on_pick`, and a chart
/// with nothing to open under a bar passes None and keeps the plain
/// cursor. The caller sizes the returned element and renders any
/// readout itself.
pub fn bars<V: 'static>(
    values: Vec<u64>,
    hover: &BarHover,
    lo: Rgba,
    hi: Rgba,
    pick: Rgba,
    on_pick: Option<impl Fn(&mut V, usize, &mut Context<V>) + 'static>,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let count = values.len();
    let moved = hover.clone();
    let left = hover.clone();
    let clicked = hover.clone();
    let paint = hover.clone();
    div()
        // The id makes the element stateful, which hover tracking needs.
        .id("bar-chart")
        .size_full()
        .when_some(on_pick, |d, on_pick| {
            d.cursor_pointer()
                .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                    if let Some(ix) = clicked.index() {
                        on_pick(view, ix, cx);
                    }
                }))
        })
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
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    *paint.bounds.lock().unwrap() = Some(bounds);
                    let picked = *paint.index.lock().unwrap();
                    paint_bars(&values, picked, lo, hi, pick, bounds, window);
                },
            )
            .size_full(),
        )
}

/// The bars into their bounds, a hairline gap once they're wide enough
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

/// The widest arc drawn in one SVG arc command. Ninety degrees keeps every
/// arc a short one, so the large-arc flag is always false and there's no
/// case where the sweep could pick the wrong half of the circle.
const ARC_STEP: f32 = 90.;

/// Where the ring starts and which way it runs: twelve o'clock, clockwise,
/// the way a progress dial reads.
const RING_START: f32 = -90.;

/// A ring showing one share of a whole: a full track ring in `track`, with
/// `fraction` of it drawn over in `value`, running clockwise from twelve.
///
/// One share, not a slice per category, on purpose. The health overview's
/// checks overlap (a track missing genre and year fails two of them), so
/// slices would add up to more than the library and read as a lie. The
/// caller sizes and centres its own readout over the hole; text can't be
/// painted from inside a canvas closure without the text system.
pub fn ring(fraction: f32, diameter: Pixels, thickness: Pixels, track: Rgba, value: Rgba) -> Div {
    div().w(diameter).h(diameter).flex_none().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_ring(fraction, thickness, track, value, bounds, window);
            },
        )
        .size_full(),
    )
}

/// The arc spans a fraction covers, in degrees, each no wider than
/// [`ARC_STEP`]. Empty at zero: nothing to draw is not the same as a
/// zero-width arc, which tessellates to a stray sliver.
fn ring_spans(fraction: f32) -> Vec<(f32, f32)> {
    let sweep = fraction.clamp(0., 1.) * 360.;
    if sweep <= 0. {
        return Vec::new();
    }
    let steps = (sweep / ARC_STEP).ceil() as usize;
    let step = sweep / steps as f32;
    (0..steps)
        .map(|i| {
            let start = RING_START + i as f32 * step;
            (start, start + step)
        })
        .collect()
}

/// The track ring and the value arc into their bounds, both as annulus
/// paths. The ring squares itself off the shorter side, so an over-wide
/// slot leaves it centred rather than stretching it into an ellipse.
fn paint_ring(
    fraction: f32,
    thickness: Pixels,
    track: Rgba,
    value: Rgba,
    bounds: Bounds<Pixels>,
    window: &mut gpui::Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let outer = w.min(h) / 2.;
    let inner = (outer - f32::from(thickness)).max(0.);
    if outer <= 0. {
        return;
    }
    let centre = point(
        px(f32::from(bounds.origin.x) + w / 2.),
        px(f32::from(bounds.origin.y) + h / 2.),
    );
    if let Some(path) = annulus(centre, outer, inner, &ring_spans(1.)) {
        window.paint_path(path, track);
    }
    if let Some(path) = annulus(centre, outer, inner, &ring_spans(fraction)) {
        window.paint_path(path, value);
    }
}

/// One closed contour running out along the outer radius and back along
/// the inner one: the outer arcs clockwise, a step inward, then the inner
/// arcs back counter-clockwise. A full ring's step inward is a hairline
/// slit at twelve o'clock, which is the standard way to cut a hole with a
/// single contour and tessellates cleanly either fill rule.
///
/// None when there's nothing to draw, or when the builder refuses the
/// path: a chart that can't tessellate paints nothing rather than taking
/// the frame down.
fn annulus(
    centre: Point<Pixels>,
    outer: f32,
    inner: f32,
    spans: &[(f32, f32)],
) -> Option<Path<Pixels>> {
    let (&(first, _), &(_, last)) = (spans.first()?, spans.last()?);
    let mut builder = PathBuilder::fill();
    builder.move_to(on_circle(centre, outer, first));
    for (_, end) in spans {
        builder.arc_to(
            point(px(outer), px(outer)),
            px(0.),
            false,
            true,
            on_circle(centre, outer, *end),
        );
    }
    builder.line_to(on_circle(centre, inner, last));
    for (start, _) in spans.iter().rev() {
        builder.arc_to(
            point(px(inner), px(inner)),
            px(0.),
            false,
            false,
            on_circle(centre, inner, *start),
        );
    }
    builder.close();
    builder.build().ok()
}

/// A point on the circle at an angle in degrees, zero at three o'clock and
/// growing clockwise, which is what screen coordinates give for free.
fn on_circle(centre: Point<Pixels>, radius: f32, degrees: f32) -> Point<Pixels> {
    let radians = degrees.to_radians();
    point(
        px(f32::from(centre.x) + radius * radians.cos()),
        px(f32::from(centre.y) + radius * radians.sin()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ring's arcs: none at empty, and a quarter circle apiece
    /// otherwise, so no single arc command ever has to guess which half of
    /// the circle the sweep meant.
    #[test]
    fn a_ring_splits_its_sweep_into_quarter_circles() {
        assert!(ring_spans(0.).is_empty());
        assert_eq!(ring_spans(0.5).len(), 2);
        assert_eq!(ring_spans(1.).len(), 4);
        // A share that doesn't divide evenly still gets equal steps, so
        // the arcs meet without a seam.
        let spans = ring_spans(0.3);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].1, spans[1].0);
        assert!((spans[1].1 - (RING_START + 108.)).abs() < 0.001);
    }

    /// Out-of-range shares are clamped rather than wrapped: a fraction over
    /// one would otherwise draw more than a full ring and paint over its
    /// own start.
    #[test]
    fn a_ring_clamps_shares_outside_zero_to_one() {
        assert!(ring_spans(-0.5).is_empty());
        assert_eq!(ring_spans(2.).len(), 4);
    }

    /// The arcs actually tessellate on the pinned gpui, at every share and
    /// at a thickness thicker than the ring itself. Worth a test rather than
    /// a look: `build` swallows a tessellation failure into an Err, and the
    /// only symptom on screen would be a ring that silently isn't there.
    #[test]
    fn every_share_builds_a_path() {
        let centre = point(px(60.), px(60.));
        for share in [0.01, 0.25, 0.5, 0.75, 0.99, 1.] {
            assert!(
                annulus(centre, 50., 38., &ring_spans(share)).is_some(),
                "{share} tessellates"
            );
        }
        // A band thicker than the radius collapses the hole rather than
        // inverting it, and still builds.
        assert!(annulus(centre, 50., 0., &ring_spans(1.)).is_some());
        // Nothing to draw is None, not an empty path.
        assert!(annulus(centre, 50., 38., &ring_spans(0.)).is_none());
    }
}
