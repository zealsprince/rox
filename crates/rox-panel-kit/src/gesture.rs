//! Gesture and scroll mechanics shared by the panels: scrub strips, flick
//! scrolling, glide-to-row, and slider painting.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Along, App, Axis, Bounds, Context, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollHandle, UniformListScrollHandle, Window, fill, point, px, size,
};
use rox_design::{palette, tokens};

/// A click-and-drag strip's bounds and drag state, behind Arcs so paint
/// closures and window-level handlers can hold it.
#[derive(Clone, Default)]
pub struct ScrubState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
    /// Kept apart from the drag so a plain hover shows the readout without
    /// seeking.
    hover: Arc<Mutex<Option<f32>>>,
}

impl ScrubState {
    /// Shared by every clone: which strip a readout edit belongs to.
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.bounds) as usize
    }

    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        *self.bounds.lock().unwrap() = Some(bounds);
    }

    pub fn width(&self) -> Option<f32> {
        let bounds = (*self.bounds.lock().unwrap())?;

        Some(f32::from(bounds.size.width)).filter(|w| *w > 0.0)
    }

    pub fn begin(&self) {
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    /// Clamps off the ends, so a drag can overshoot without letting go.
    pub fn fraction(&self, x: Pixels) -> Option<f32> {
        let bounds = (*self.bounds.lock().unwrap())?;
        let w = f32::from(bounds.size.width);
        if w <= 0.0 {
            return None;
        }
        Some((f32::from(x - bounds.origin.x) / w).clamp(0.0, 1.0))
    }

    /// Returns whether it changed, so the caller only notifies on a real move.
    pub fn set_hover(&self, fraction: Option<f32>) -> bool {
        let mut current = self.hover.lock().unwrap();
        if *current == fraction {
            return false;
        }
        *current = fraction;
        true
    }

    pub fn hover(&self) -> Option<f32> {
        *self.hover.lock().unwrap()
    }
}

/// `dimmed` fades the fill and keeps the knob, the volume strip's muted look.
pub fn paint_slider(fraction: f32, dimmed: bool, bounds: Bounds<Pixels>, window: &mut Window) {
    let track_h = tokens::SLIDER_TRACK_H;
    let knob = tokens::SLIDER_KNOB;

    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= knob || h <= 0.0 {
        return;
    }

    let knob_x = knob / 2.0 + fraction.clamp(0.0, 1.0) * (w - knob);
    let track_y = bounds.origin.y + px((h - track_h) / 2.0);
    // A core-role wash, not a surface read, so the track stays visible when
    // surface opacity thins the panel to nothing.
    window.paint_quad(
        fill(
            Bounds::new(point(bounds.origin.x, track_y), size(px(w), px(track_h))),
            palette::alpha(palette::accent(), 0x33),
        )
        .corner_radii(px(track_h / 2.0)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, track_y),
                size(px(knob_x), px(track_h)),
            ),
            if dimmed {
                palette::alpha(palette::accent(), 0x33)
            } else {
                palette::accent()
            },
        )
        .corner_radii(px(track_h / 2.0)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(knob_x - knob / 2.0),
                    bounds.origin.y + px((h - knob) / 2.0),
                ),
                size(px(knob), px(knob)),
            ),
            if dimmed {
                palette::text_dim()
            } else {
                palette::highlight()
            },
        )
        .corner_radii(px(knob / 2.0)),
    );
}

pub const RESUME_IDLE: Duration = Duration::from_secs(12);

/// The idle-resume clock a browse panel keeps to drift back to the playing
/// track. Only one wake task is ever in flight: a burst of events pushes the
/// stamp forward and that task re-sleeps until a full idle window has passed.
#[derive(Clone, Default)]
pub struct ResumeIdle {
    /// None until the first interaction, so the resume never fires before then.
    at: Arc<Mutex<Option<Instant>>>,
    armed: Arc<AtomicBool>,
}

impl ResumeIdle {
    pub fn touch<P: 'static>(&self, cx: &mut Context<P>, resume: fn(&mut P, &mut Context<P>)) {
        *self.at.lock().unwrap() = Some(Instant::now());
        if self.armed.swap(true, Ordering::AcqRel) {
            return;
        }
        let at = self.at.clone();
        let armed = self.armed.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let Some(last) = *at.lock().unwrap() else {
                    break;
                };
                let remaining = RESUME_IDLE.saturating_sub(last.elapsed());
                if remaining.is_zero() {
                    break;
                }
                cx.background_executor().timer(remaining).await;
            }
            armed.store(false, Ordering::Release);
            this.update(cx, resume).ok();
        })
        .detach();
    }
}

/// Drag past a dead zone to scroll, release to coast. Behind Arcs like
/// [`ScrubState`].
#[derive(Clone, Default)]
pub struct FlickState {
    inner: Arc<Mutex<FlickInner>>,
    dragging: Arc<AtomicBool>,
}

#[derive(Default)]
struct FlickInner {
    /// (y, when), newest last. Only this window sets the release velocity, so
    /// speed from earlier in the drag doesn't outlast a pause at the end.
    samples: VecDeque<(f32, Instant)>,
    /// Past the dead zone the drag is a scroll and the release swallows the
    /// click.
    travel: f32,
    /// px/s, downward-positive.
    velocity: f32,
}

/// px of travel that still counts as a click.
const FLICK_DEAD_ZONE: f32 = 4.0;
/// Velocity multiplies by this each second, so a flick settles in about a
/// second.
const FLICK_DECAY: f32 = 0.02;
/// px/s.
const FLICK_REST: f32 = 12.0;
/// Seconds. A pause before letting go empties the window, so the coast starts
/// from rest.
const FLICK_WINDOW: f32 = 0.1;

impl FlickState {
    pub fn begin(&self, y: Pixels) {
        let mut inner = self.inner.lock().unwrap();
        inner.samples.clear();
        inner.samples.push_back((f32::from(y), Instant::now()));
        inner.travel = 0.0;
        inner.velocity = 0.0;
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    pub fn scrolled(&self) -> bool {
        self.inner.lock().unwrap().travel > FLICK_DEAD_ZONE
    }

    /// The delta to scroll by, zero inside the dead zone.
    fn track(&self, y: Pixels) -> f32 {
        let mut inner = self.inner.lock().unwrap();
        let y = f32::from(y);
        let Some(&(last_y, _)) = inner.samples.back() else {
            return 0.0;
        };
        let now = Instant::now();
        inner.samples.push_back((y, now));
        while inner
            .samples
            .front()
            .is_some_and(|&(_, at)| now.duration_since(at).as_secs_f32() > FLICK_WINDOW)
        {
            inner.samples.pop_front();
        }
        let dy = y - last_y;
        inner.travel += dy.abs();
        if inner.travel > FLICK_DEAD_ZONE {
            dy
        } else {
            0.0
        }
    }

    fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        while inner
            .samples
            .front()
            .is_some_and(|&(_, at)| now.duration_since(at).as_secs_f32() > FLICK_WINDOW)
        {
            inner.samples.pop_front();
        }
        inner.velocity = match (inner.samples.front(), inner.samples.back()) {
            (Some(&(y0, t0)), Some(&(y1, t1))) if t1 > t0 => {
                (y1 - y0) / t1.duration_since(t0).as_secs_f32()
            }
            _ => 0.0,
        };
        inner.samples.clear();
    }

    /// None once settled or while still dragging.
    pub fn coast(&self, dt: f32) -> Option<f32> {
        if self.is_dragging() {
            return None;
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.velocity.abs() < FLICK_REST {
            inner.velocity = 0.0;
            return None;
        }
        let dy = inner.velocity * dt;
        inner.velocity *= FLICK_DECAY.powf(dt);
        Some(dy)
    }
}

/// Call from the surface's paint pass: window handlers last one frame.
/// `apply` must notify an entity so the next frame re-arms them.
pub fn flick_on_paint_axis(
    flick: &FlickState,
    axis: Axis,
    window: &mut Window,
    apply: impl Fn(f32, &mut App) + 'static,
) {
    if !flick.is_dragging() {
        return;
    }
    window.on_mouse_event({
        let flick = flick.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() || !flick.is_dragging() {
                return;
            }
            // A release outside the window never fires the up handler.
            if event.pressed_button != Some(MouseButton::Left) {
                flick.end();
                return;
            }
            let d = flick.track(event.position.along(axis));
            if d != 0.0 {
                apply(d, cx);
            }
        }
    });
    window.on_mouse_event({
        let flick = flick.clone();
        move |_: &MouseUpEvent, phase, _, _| {
            if phase.bubble() {
                flick.end();
            }
        }
    });
}

/// The offset that centers item `ix` of `count`. The handle's `item` size is
/// the viewport despite the name, so the item extent comes from the content
/// height. None before the first layout.
pub fn glide_target(handle: &UniformListScrollHandle, ix: usize, count: usize) -> Option<Pixels> {
    if count == 0 {
        return None;
    }
    let sizes = handle.0.borrow().last_item_size?;
    let item_h = sizes.contents.height / count as f32;
    let viewport_h = sizes.item.height;
    if viewport_h <= px(0.) {
        return None;
    }
    let y = item_h * ix as f32 - (viewport_h - item_h) * 0.5;
    let max = (sizes.contents.height - viewport_h).max(px(0.));
    Some(y.clamp(px(0.), max))
}

/// An exponential approach, done inside a pixel. Returns whether another
/// frame is needed.
pub fn glide_step(handle: &UniformListScrollHandle, target: Pixels, dt: f32) -> bool {
    let base = handle.0.borrow().base_handle.clone();
    let mut offset = base.offset();
    let current = -offset.y;
    let diff = target - current;
    if diff.abs() < px(1.) {
        offset.y = -target;
        base.set_offset(offset);
        return false;
    }
    // Cover 92% of the remaining distance every tenth of a second.
    let step = 1.0 - (0.08_f32).powf(dt * 10.0);
    offset.y = -(current + diff * step.clamp(0.0, 1.0));
    base.set_offset(offset);
    true
}

/// [`glide_target`] for a plain scroll handle along either axis.
pub fn glide_target_axis(
    handle: &ScrollHandle,
    axis: Axis,
    ix: usize,
    count: usize,
) -> Option<Pixels> {
    if count == 0 {
        return None;
    }
    let viewport = handle.bounds().size.along(axis);
    if viewport <= px(0.) {
        return None;
    }
    // max_offset is content minus viewport.
    let max = handle.max_offset().along(axis);
    let item = (max + viewport) / count as f32;
    let target = item * ix as f32 - (viewport - item) * 0.5;
    Some(target.clamp(px(0.), max))
}

/// [`glide_target_axis`] for non-uniform items, from the item's real position.
pub fn glide_target_at(
    handle: &ScrollHandle,
    axis: Axis,
    origin: Pixels,
    extent: Pixels,
) -> Option<Pixels> {
    let viewport = handle.bounds().size.along(axis);
    if viewport <= px(0.) {
        return None;
    }
    let max = handle.max_offset().along(axis);
    Some((origin - (viewport - extent) * 0.5).clamp(px(0.), max))
}

/// [`glide_step_axis`] without the easing. Offsets run negative as the list
/// scrolls.
pub fn glide_snap_axis(handle: &ScrollHandle, axis: Axis, target: Pixels) -> bool {
    let offset = handle.offset();
    if (-offset.along(axis) - target).abs() < px(1.) {
        return true;
    }
    handle.set_offset(offset.apply_along(axis, |_| -target));
    false
}

pub fn glide_step_axis(handle: &ScrollHandle, axis: Axis, target: Pixels, dt: f32) -> bool {
    let offset = handle.offset();
    let current = -offset.along(axis);
    let diff = target - current;
    if diff.abs() < px(1.) {
        handle.set_offset(offset.apply_along(axis, |_| -target));
        return false;
    }
    // Cover 92% of the remaining distance every tenth of a second.
    let step = 1.0 - (0.08_f32).powf(dt * 10.0);
    let next = current + diff * step.clamp(0.0, 1.0);
    handle.set_offset(offset.apply_along(axis, |_| -next));
    true
}

/// Call from the strip's paint pass: window handlers last one frame. `apply`
/// must notify an entity so the next frame re-arms them.
pub fn scrub_on_paint(
    scrub: &ScrubState,
    window: &mut Window,
    apply: impl Fn(f32, &mut App) + 'static,
) {
    if !scrub.is_dragging() {
        return;
    }
    window.on_mouse_event({
        let scrub = scrub.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() || !scrub.is_dragging() {
                return;
            }
            // A release outside the window never fires the up handler.
            if event.pressed_button != Some(MouseButton::Left) {
                scrub.end();
                return;
            }
            if let Some(fraction) = scrub.fraction(event.position.x) {
                apply(fraction, cx);
            }
        }
    });
    window.on_mouse_event({
        let scrub = scrub.clone();
        move |_: &MouseUpEvent, phase, _, _| {
            if phase.bubble() {
                scrub.end();
            }
        }
    });
}
