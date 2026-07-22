//! Gesture and scroll mechanics shared by the panels: the click-and-drag
//! scrub strip, the flick/momentum scroller, glide-to-row animation, and the
//! slider painting. Self-contained; consumed by panels, not the framework.

use super::*;

/// The shared state of a click-and-drag strip: where it painted and
/// whether a drag is live. Behind Arcs so the panel, its paint closures,
/// and the window-level mouse handlers can all hold it.
#[derive(Clone, Default)]
pub struct ScrubState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
    /// The pointer's fraction along the strip while hovering, None with the
    /// pointer off it. Drives the seek preview label, kept apart from the
    /// drag state so a plain hover shows the readout without seeking.
    hover: Arc<Mutex<Option<f32>>>,
}

impl ScrubState {
    /// Remember where the strip landed, from its prepaint.
    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        *self.bounds.lock().unwrap() = Some(bounds);
    }

    /// A drag started (mouse down on the strip).
    pub fn begin(&self) {
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    /// Where `x` lands along the strip, 0 to 1; positions off the ends
    /// clamp, so a drag can overshoot without letting go of the value.
    pub fn fraction(&self, x: Pixels) -> Option<f32> {
        let bounds = (*self.bounds.lock().unwrap())?;
        let w = f32::from(bounds.size.width);
        if w <= 0.0 {
            return None;
        }
        Some((f32::from(x - bounds.origin.x) / w).clamp(0.0, 1.0))
    }

    /// Remember where the pointer hovers, 0 to 1, or None off the strip.
    /// Returns whether it changed, so the caller only notifies on a real
    /// move.
    pub fn set_hover(&self, fraction: Option<f32>) -> bool {
        let mut current = self.hover.lock().unwrap();
        if *current == fraction {
            return false;
        }
        *current = fraction;
        true
    }

    /// The hovered fraction, None with the pointer off the strip.
    pub fn hover(&self) -> Option<f32> {
        *self.hover.lock().unwrap()
    }
}

/// A horizontal slider's paint: a rounded track, the fraction as the
/// accent-filled side, a round knob at the position. `dimmed` keeps the
/// knob where it is and fades the fill, the volume strip's muted look.
pub fn paint_slider(fraction: f32, dimmed: bool, bounds: Bounds<Pixels>, window: &mut Window) {
    let track_h = tokens::SLIDER_TRACK_H;
    let knob = tokens::SLIDER_KNOB;

    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= knob || h <= 0.0 {
        return;
    }

    // The knob's travel is inset by its radius so it never clips the ends.
    let knob_x = knob / 2.0 + fraction.clamp(0.0, 1.0) * (w - knob);
    let track_y = bounds.origin.y + px((h - track_h) / 2.0);
    window.paint_quad(
        fill(
            Bounds::new(point(bounds.origin.x, track_y), size(px(w), px(track_h))),
            palette::bg_control(),
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

/// How long a browse panel waits after the last interaction before it
/// slides back to the playing track, when the resume behavior is on.
pub const RESUME_IDLE: Duration = Duration::from_secs(12);

/// The idle-resume clock a browse panel keeps so it can drift back to the
/// playing track once the user has left it alone. Panels with the behavior
/// off never touch it. Behind an Arc like [`FlickState`] so the wake task
/// can read the last-interaction stamp without bouncing through the panel
/// each tick. A single wake stays in flight at a time: a scroll fires a
/// burst of events, but only the first arms the task, the rest just push
/// the stamp forward and the one task re-sleeps until a full window has
/// passed since the last of them.
#[derive(Clone, Default)]
pub struct ResumeIdle {
    /// When the panel was last scrolled, dragged, or keyed. None until the
    /// first interaction, so the resume never fires before then.
    at: Arc<Mutex<Option<Instant>>>,
    /// A wake task is already counting down; keeps a burst of interactions
    /// from arming one apiece.
    armed: Arc<AtomicBool>,
}

impl ResumeIdle {
    /// Note an interaction and, unless one is already counting down, arm a
    /// wake. The wake sleeps until a full window has passed since the last
    /// interaction, then calls `resume` once on the panel.
    pub fn touch<P: 'static>(&self, cx: &mut Context<P>, resume: fn(&mut P, &mut Context<P>)) {
        *self.at.lock().unwrap() = Some(Instant::now());
        if self.armed.swap(true, Ordering::AcqRel) {
            return;
        }
        let at = self.at.clone();
        let armed = self.armed.clone();
        cx.spawn(async move |this, cx| {
            // Re-sleep for whatever is left of the window after the newest
            // interaction, so a gesture mid-countdown pushes the wake out
            // instead of stacking a second task.
            loop {
                let Some(last) = *at.lock().unwrap() else { break };
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

/// The shared state of a drag-to-scroll surface: press, drag past a dead
/// zone to scroll, release to let the built-up velocity coast. Behind
/// Arcs like [`ScrubState`], so the view, its paint closures, and the
/// window-level handlers can all hold it.
#[derive(Clone, Default)]
pub struct FlickState {
    inner: Arc<Mutex<FlickInner>>,
    dragging: Arc<AtomicBool>,
}

#[derive(Default)]
struct FlickInner {
    /// The pointer's recent path, (y, when) with the newest last. The
    /// release reads its velocity off this window, so speed built up
    /// earlier in the drag can't survive a pause at the end.
    samples: VecDeque<(f32, Instant)>,
    /// Total pointer travel this drag; past the dead zone it counts as a
    /// scroll and the release swallows the click.
    travel: f32,
    /// Coasting speed after release, px/s downward-positive.
    velocity: f32,
}

/// Pointer travel below this stays a click, in px. Matches the slop a
/// finger or a twitchy mouse needs before a press means "scroll".
const FLICK_DEAD_ZONE: f32 = 4.0;
/// The coast's exponential decay: velocity multiplies by this each
/// second, so a flick settles in about a second.
const FLICK_DECAY: f32 = 0.02;
/// Coasting below this speed stops, px/s.
const FLICK_REST: f32 = 12.0;
/// How far back the release looks for its velocity, in seconds. Only
/// motion inside this window coasts: a pause before letting go leaves
/// the window empty, and a jittery hold nets out to nearly zero.
const FLICK_WINDOW: f32 = 0.1;

impl FlickState {
    /// A press landed: start tracking, stop any coast.
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

    /// Whether the drag left the dead zone, so the release is a scroll's
    /// end and not a click.
    pub fn scrolled(&self) -> bool {
        self.inner.lock().unwrap().travel > FLICK_DEAD_ZONE
    }

    /// Track a move to `y`: the pointer's delta comes back for the host
    /// to scroll by (zero inside the dead zone), and the sample joins
    /// the velocity window.
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

    /// The release: done dragging, the coast's velocity is the net
    /// motion across the sample window. A pause before letting go has
    /// aged every sample out, so the coast starts from rest.
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

    /// One coast step: the distance to scroll this frame, decayed toward
    /// rest. None once settled (or while still dragging).
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

/// Keep a live drag-scroll following the pointer along `axis`: scroll by the
/// pointer's travel on every move, end the drag on release. Call from the
/// surface's paint pass, the [`scrub_on_paint`] idiom - window handlers only
/// live one frame. Applying must notify an entity so the next frame re-arms
/// the handlers.
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
            // A release outside the window never reaches the up handler;
            // a move without the button still held ends the drag instead.
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

/// Where a uniform list's offset should sit to center item `ix` of
/// `count`, for the follow-playing glide: item extent times index, pulled
/// back by half the viewport, clamped to the scrollable range. The item
/// extent derives from the content height (the handle's `item` size is
/// the viewport, despite the name). None before the list's first layout.
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

/// One glide step toward `target`: an exponential approach, done inside
/// a pixel. Returns whether another frame is needed; the caller requests
/// it and re-renders.
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

/// [`glide_target`] for a virtual list's plain scroll handle: where the
/// offset should sit to center item `ix` of `count` along `axis`. The
/// viewport and content extents come off the handle rather than a uniform
/// list's item size, so it fits either scroll axis. None before the list's
/// first layout gives it a viewport.
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
    // max_offset is content minus viewport, so content is the two summed;
    // the item extent is that content over the item count.
    let max = handle.max_offset().along(axis);
    let item = (max + viewport) / count as f32;
    let target = item * ix as f32 - (viewport - item) * 0.5;
    Some(target.clamp(px(0.), max))
}

/// [`glide_snap`] on a plain scroll handle along `axis`: pin the offset to
/// `target` in one move, true once already there. Offsets run negative as
/// the list scrolls, so the stored position is the negated axis component.
pub fn glide_snap_axis(handle: &ScrollHandle, axis: Axis, target: Pixels) -> bool {
    let offset = handle.offset();
    if (-offset.along(axis) - target).abs() < px(1.) {
        return true;
    }
    handle.set_offset(offset.apply_along(axis, |_| -target));
    false
}

/// [`glide_step`] on a plain scroll handle along `axis`: one eased step
/// toward `target`, returning whether another frame is still needed.
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

/// Keep a live drag following the pointer: apply the strip fraction on
/// every move, end the drag on release. Call from the strip's paint pass -
/// window handlers only live one frame, the same idiom the dock's resize
/// handles use. Applying must notify an entity so the next frame re-arms
/// the handlers.
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
            // A release outside the window never reaches the up handler;
            // a move without the button still held ends the drag instead.
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
