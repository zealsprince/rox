//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    anchored, canvas, deferred, div, fill, point, prelude::*, px, relative, size, svg,
    AbsoluteLength, Along, AnyElement, App, Axis, Bounds, Context, DismissEvent, Div, Element,
    Entity, FocusHandle, Focusable as _, GlobalElementId, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollHandle, SharedString,
    Stateful, Subscription, TitlebarOptions, UniformListScrollHandle, WeakEntity, Window,
    WindowBounds, WindowOptions,
};
use gpui_component::button::Button;
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{h_flex, Icon, IconName, Root, Sizable};
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::history::History;
use crate::lastfm::Scrobbler;
use crate::panels::library::Library;
use crate::player::{fmt_time, Player};
use crate::selection::Selection;
use crate::query::shared_query::SharedQuery;
use crate::thumbs::Thumbs;
use crate::workspace::{SeekBackward, SeekForward, TogglePlayback};

/// The shared entities every panel renders over: one player, one catalog,
/// and one selection per workspace. Cloning shares the handles, not the
/// state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
    pub selection: Entity<Selection>,
    /// The app-wide search query the global-following panels share.
    pub query: Entity<SharedQuery>,
    pub tab_hosts: Entity<TabHosts>,
    /// The playing track's art baked into the window backdrop, one bake
    /// shared by every window over this player.
    pub now_art: Entity<NowPlayingArt>,
    /// The artwork service's texture cache, shared by every view that
    /// draws cover thumbnails.
    pub thumbs: Entity<Thumbs>,
    /// The last.fm scrobbler over this workspace's player; also where the
    /// live scrobble config lives, for the panels' threshold markers.
    pub scrobbler: Entity<Scrobbler>,
    /// The listen recorder riding the scrobbler's listen signal; history
    /// views subscribe to it for the refresh when an event lands.
    pub history: Entity<History>,
}

/// Every tab panel that has hosted one of our panels, reported from each
/// panel's `on_added_to`. Dragging a tab into a split makes the dock create
/// tab panels on its own and nothing announces them to the workspace, so
/// this registry is how it finds them, to pick a live tab panel for
/// Panels-menu additions.
#[derive(Default)]
pub struct TabHosts {
    hosts: Vec<WeakEntity<TabPanel>>,
}

impl TabHosts {
    /// Record a hosting tab panel.
    pub fn report(&mut self, tabs: WeakEntity<TabPanel>) {
        if self.hosts.iter().any(|t| t.entity_id() == tabs.entity_id()) {
            return;
        }
        self.hosts.push(tabs);
    }

    /// The newest recorded tab panel that is still alive and showing panels.
    pub fn last_live(&self, cx: &App) -> Option<Entity<TabPanel>> {
        self.hosts.iter().rev().find_map(|tabs| {
            let tabs = tabs.upgrade()?;
            tabs.read(cx).visible(cx).then_some(tabs)
        })
    }
}

/// Jump to an open panel by its built-in name across every tab group that has
/// hosted our panels: make the first live match the active, focused tab, and
/// return whether one was found. The queue widget uses it to reach an open
/// queue panel before falling back to a window. Popped-out panels live in
/// their own windows rather than the dock, so they are not matched here.
pub fn focus_panel_named(
    hosts: &Entity<TabHosts>,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let groups = hosts.read(cx).hosts.clone();
    for tabs in groups {
        let Some(tabs) = tabs.upgrade() else { continue };
        let target = tabs
            .read(cx)
            .panels()
            .iter()
            .find(|panel| panel.panel_name(cx) == name && panel.visible(cx))
            .cloned();
        if let Some(panel) = target {
            tabs.update(cx, |tabs, cx| tabs.focus_panel(&panel, window, cx));
            return true;
        }
    }
    false
}

/// The flat icon button the transport panels share so the button style
/// never forks: the icon alone at rest, a soft pill behind it on hover.
/// Icon paths come from [`crate::assets::icons`].
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: Rgba,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    icon_control_sized(icon, px(16.), color, on_click, cx)
}

/// [`icon_control`] with the icon size exposed, for spots like the menubar
/// where the transport-scale glyph reads too heavy.
pub fn icon_control_sized<V: 'static>(
    icon: &'static str,
    size: Pixels,
    color: Rgba,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .p(tokens::ICON_PAD)
        .rounded(tokens::RADIUS)
        .hover(|d| d.bg(palette::bg_control()))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_click(this, cx)),
        )
        .child(svg().path(icon).size(size).text_color(color))
}

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

/// Map a strip fraction to an absolute seek on the playing track, the
/// seek strip's and the waveform's shared apply.
pub fn seek_fraction(player: &Entity<Player>, fraction: f32, cx: &App) {
    let player = player.read(cx);
    let Some(now) = player.now_playing() else {
        return;
    };
    let Some(duration) = now.duration_secs else {
        return;
    };
    player.seek_to(fraction as f64 * duration);
}

/// A seek preview for a scrub strip: the time under the pointer as a small
/// pill that follows the cursor while hovering. Tracks the pointer across
/// `scrub`'s painted bounds and maps it against `duration`. Drop it as a
/// child over the strip's relative container - it covers the strip to catch
/// every move, and a click through it bubbles to the strip's own seek
/// handler underneath.
pub fn seek_hover<V: 'static>(
    scrub: &ScrubState,
    duration: f64,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let moved = scrub.clone();
    let left = scrub.clone();
    let hover = scrub.hover();
    div()
        // The id makes the element stateful, which the hover-leave catch
        // below needs.
        .id("seek-hover")
        .absolute()
        .inset_0()
        .cursor_pointer()
        .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            if moved.set_hover(moved.fraction(event.position.x)) {
                cx.notify();
            }
        }))
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            // The pointer left the strip: no more move events fire, so the
            // leave has to clear the readout itself.
            if !hovered && left.set_hover(None) {
                cx.notify();
            }
        }))
        .when_some(hover, |d, fraction| d.child(seek_pill(fraction, duration)))
}

/// The seek preview label: the time at `fraction` along the track, a pill
/// centered over that point near the top of the strip. A zero-width column
/// at the fraction centers the pill on the cursor line.
fn seek_pill(fraction: f32, duration: f64) -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(relative(fraction))
        .w_0()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                // The zero-width column above gives the text no room, so a
                // multi-digit time would wrap to one glyph per line without
                // this.
                .whitespace_nowrap()
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded(tokens::RADIUS)
                .bg(palette::bg_menu_opaque())
                .border_1()
                .border_color(palette::border())
                .text_sm()
                .text_color(palette::text())
                .child(fmt_time(fraction as f64 * duration)),
        )
}

/// A panel's tab and title text: the rename when one is set, the built-in
/// name otherwise.
pub fn title_text(custom: Option<&str>, default: &'static str) -> SharedString {
    match custom {
        Some(name) => SharedString::from(name.to_owned()),
        None => default.into(),
    }
}

/// Title-case a panel's built-in name for display. The name is a
/// serialized identifier (lowercase, space separated); tab and window
/// titles want it capitalized. No panel name contains "rox" or an
/// acronym, so a plain per-word capitalize is right here.
pub fn display_name(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Repaint the tab panel hosting a renamed panel. The tab bar draws the
/// title, and that row only repaints when the tab panel itself is
/// notified; the panel's own notify never reaches it.
pub fn refresh_tab_panel(tab_panel: &Option<WeakEntity<TabPanel>>, cx: &mut App) {
    if let Some(tabs) = tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |_, cx| cx.notify());
    }
}

/// Read a panel's config back out of a dumped panel state; anything
/// missing or malformed falls back to defaults.
pub fn config_from_info<C: Default + serde::de::DeserializeOwned>(info: &PanelInfo) -> C {
    match info {
        PanelInfo::Panel(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        _ => C::default(),
    }
}

/// The Pop Out and Close tail of a panel's dropdown menu: out of the dock
/// into an OS window, or out of the layout entirely. Pass the tab panel
/// the panel currently sits in (from `on_added_to`); the state is what
/// Dock Back later reaches the workspace through.
///
/// Close lives on this tail rather than the dock's menus so every panel
/// carries it everywhere its menu shows - for a solo content panel (no
/// tab chrome, and its content's own context menu replaces the dock's
/// body menu) this is the only close there is, and the empty window it
/// can leave behind offers the way back in. Popped out there is no Close:
/// closing the OS window is the close. Pinned panels keep the dock menus'
/// guard and the click no-ops.
pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
) -> PopupMenu {
    let pop_panel = panel.clone();
    let pop_tabs = tab_panel.clone();
    let menu = menu.item(
        PopupMenuItem::new("Pop Out")
            .icon(Icon::default().path(icons::EXTERNAL_LINK))
            .on_click(move |_, window, cx| {
                pop_out(
                    pop_panel.clone(),
                    pop_tabs.clone(),
                    state.clone(),
                    window,
                    cx,
                );
            }),
    );
    let Some(tabs) = tab_panel else {
        return menu;
    };
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Close")
            .icon(Icon::default().path(icons::CLOSE))
            .on_click(move |_, window, cx| {
                let Some(tabs) = tabs.upgrade() else {
                    return;
                };
                if panel.read(cx).locked(cx) {
                    return;
                }
                tabs.update(cx, |tabs, cx| {
                    tabs.remove_panel(Arc::new(panel.clone()), window, cx);
                });
            }),
    )
}

/// The Reveal in File Browser entry for a track context menu: shows the
/// track's file in the platform file manager, which lands in its album
/// folder. The id resolves to its path at click time, so the reveal
/// follows a file the library has since re-scanned elsewhere; None (an
/// empty selection) appends nothing.
pub fn reveal_item(menu: PopupMenu, state: AppState, id: Option<i64>) -> PopupMenu {
    let Some(id) = id else {
        return menu;
    };
    menu.item(
        PopupMenuItem::new("Reveal in File Browser")
            .icon(Icon::default().path(icons::FOLDER))
            .on_click(move |_, _, cx| {
                let path = state
                    .library
                    .read(cx)
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                if let Some(path) = path {
                    cx.reveal_path(&path);
                }
            }),
    )
}

/// A checkable flyout row whose tick tracks the live panel value instead of
/// one baked in when the menu was built. Pair it with [`follow_panel`] in the
/// submenu builder: the flyout re-renders on the click, this row re-reads the
/// value, and the tick swaps in place.
///
/// Plain `.checked(..)` rows go stale in an open flyout, our hand-built
/// submenus never dismiss on click (they carry no link back to the root menu,
/// so there is no reopen to rebuild them), so a static tick would sit wrong
/// until the whole menu is closed and reopened.
///
/// `is_on` reads the state each render, `toggle` flips it. A left `icon`
/// keeps the row looking like its plain sibling, with the tick pushed to the
/// right so the icon is not replaced. Without an icon the tick takes the left
/// slot, matching the default check side.
pub fn check_row<P: 'static>(
    label: impl Into<SharedString>,
    icon: Option<&'static str>,
    is_on: impl Fn(&P) -> bool + 'static,
    toggle: impl Fn(&mut P, &mut Context<P>) + 'static,
    panel: &Entity<P>,
) -> PopupMenuItem {
    let label: SharedString = label.into();
    let read = panel.clone();
    let weak = panel.downgrade();
    PopupMenuItem::element(move |_, cx| {
        let on = is_on(read.read(cx));
        if let Some(icon) = icon {
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_x_1()
                        .items_center()
                        .child(Icon::default().path(icon).xsmall())
                        .child(label.clone()),
                )
                .when(on, |row| row.child(Icon::new(IconName::Check).xsmall()))
        } else {
            h_flex()
                .gap_x_1()
                .items_center()
                .child(if on {
                    Icon::new(IconName::Check).xsmall().into_any_element()
                } else {
                    Icon::empty().xsmall().into_any_element()
                })
                .child(label.clone())
        }
    })
    .on_click(move |_, _, cx| {
        let Some(this) = weak.upgrade() else { return };
        this.update(cx, |this, cx| {
            toggle(this, cx);
            cx.notify();
        });
    })
}

/// Re-render an open flyout whenever `panel` changes, so its [`check_row`]s
/// pick up the flip without the menu closing. Call once in the submenu
/// builder, where `cx` is the submenu's own context.
pub fn follow_panel<P: 'static>(panel: &Entity<P>, cx: &mut Context<PopupMenu>) {
    cx.observe(panel, |_, _, cx| cx.notify()).detach();
}

/// Resolve track ids to paths and hand them to the player: after the playing
/// track when `next`, at the tail otherwise. Shared by the context-menu
/// actions across every song surface.
pub fn queue_tracks(state: &AppState, ids: &[i64], next: bool, cx: &mut App) {
    let paths = match state.library.read(cx).paths_for(ids) {
        Ok(paths) if !paths.is_empty() => paths,
        _ => return,
    };
    state.player.update(cx, |player, cx| {
        if next {
            player.play_next(paths, cx);
        } else {
            player.enqueue(paths, cx);
        }
    });
}

/// The track actions every song surface's right-click shares: Play under
/// the caller's label, the selection into the tag and cover editors, and
/// Reveal in File Browser. What playing queues differs per panel (the
/// view from a row, the highlighted set, whole albums), so the caller
/// hands the click over; everything after acts on the ids, resolved at
/// build time so the editors get this set even if another panel
/// publishes over the shared selection before the click lands. Reveal
/// follows the first id; empty ids appends no Reveal.
pub fn track_actions(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    play_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    on_play: impl Fn(&mut Window, &mut App) + 'static,
) -> PopupMenu {
    let reveal = ids.first().copied();
    let tag_ids = ids.clone();
    let tag_state = state.clone();
    let cover_state = state.clone();
    let next_state = state.clone();
    let next_ids = ids.clone();
    let queue_state = state.clone();
    let queue_ids = ids.clone();
    let playlist_state = state.clone();
    let playlist_ids = ids.clone();
    let menu = menu
        .item(
            PopupMenuItem::new(play_label)
                .icon(Icon::default().path(icons::PLAY))
                .on_click(move |_, window, cx| on_play(window, cx)),
        )
        // Queue the selection right after the playing track, or start it when
        // nothing plays. Paths resolve here so the queue holds the same set
        // even if the selection moves before the click lands.
        .item(
            PopupMenuItem::new("Play Next")
                .icon(Icon::default().path(icons::SKIP_FORWARD))
                .on_click(move |_, _, cx| {
                    queue_tracks(&next_state, &next_ids, true, cx);
                }),
        )
        .item(
            PopupMenuItem::new("Add to Queue")
                .icon(Icon::default().path(icons::LIST_MUSIC))
                .on_click(move |_, _, cx| {
                    queue_tracks(&queue_state, &queue_ids, false, cx);
                }),
        );
    // The favourites toggle: off to on when any of the set is not favourited,
    // on to off only when the whole set already is, so a mixed selection lands
    // everything in favourites first. Reads its state at open time.
    let favourites = state.library.read(cx).favourite_ids();
    let all_fav = !ids.is_empty() && ids.iter().all(|id| favourites.contains(id));
    let fav_state = state.clone();
    let fav_ids = ids.clone();
    let (fav_label, fav_icon) = if all_fav {
        ("Remove from Favourites", icons::HEART_FILLED)
    } else {
        ("Add to Favourites", icons::HEART)
    };
    let menu = menu.item(
        PopupMenuItem::new(fav_label)
            .icon(Icon::default().path(fav_icon))
            .on_click(move |_, _, cx| {
                let ids = fav_ids.clone();
                fav_state
                    .library
                    .update(cx, |library, cx| library.set_favourites(&ids, !all_fav, cx));
            }),
    );
    // Add to Playlist flies out the existing playlists with Create New at the
    // top. Built at open time, so it reflects playlists made this session.
    let submenu = PopupMenu::build(window, cx, move |mut submenu, _window, cx| {
        let new_state = playlist_state.clone();
        let new_ids = playlist_ids.clone();
        submenu = submenu.item(
            PopupMenuItem::new("New Playlist...")
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    crate::playlist_create::open(new_state.clone(), new_ids.clone(), cx);
                }),
        );
        let playlists = playlist_state.library.read(cx).playlists();
        if !playlists.is_empty() {
            submenu = submenu.separator();
        }
        for playlist in playlists {
            let add_state = playlist_state.clone();
            let add_ids = playlist_ids.clone();
            let id = playlist.id;
            submenu = submenu.item(
                PopupMenuItem::new(SharedString::from(playlist.name)).on_click(move |_, _, cx| {
                    let add_ids = add_ids.clone();
                    add_state.library.update(cx, |library, cx| {
                        library.add_to_playlist(id, &add_ids, cx);
                    });
                }),
            );
        }
        submenu
    });
    let menu = menu.item(
        PopupMenuItem::submenu("Add to Playlist", submenu)
            .icon(Icon::default().path(icons::LIST_MUSIC)),
    );
    let menu = menu
        // The primary editing flow: the selection into the tag editor
        // window; the metadata panel's inline pencil stays the quick path.
        .item(
            PopupMenuItem::new("Edit Tags...")
                .icon(Icon::default().path(icons::PENCIL))
                .on_click(move |_, _, cx| {
                    crate::tags::editor::open(tag_state.clone(), tag_ids.clone(), cx);
                }),
        )
        // Covers get their own window: the tag editor edits text per
        // track, this stamps one image across the selection.
        .item(
            PopupMenuItem::new("Edit Cover Art...")
                .icon(Icon::default().path(icons::IMAGE))
                .on_click(move |_, _, cx| {
                    crate::cover::editor::open(cover_state.clone(), ids.clone(), cx);
                }),
        );
    reveal_item(menu, state, reveal)
}

/// Move a docked panel into its own OS window. The panel entity itself moves,
/// so it keeps rendering the same shared state; closing the window drops it.
pub fn pop_out<P: Panel>(
    panel: Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &mut Window,
    cx: &mut App,
) {
    // Detach from the dock first; the new window's host keeps the entity
    // alive from here on.
    if let Some(tabs) = tab_panel.and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(Arc::new(panel.clone()), window, cx);
        });
    }
    pop_out_view(Arc::new(panel), state, cx);
}

/// Open an OS window hosting an already-detached panel. Also the dock's
/// middle-drag-out hook: dragging a panel out of the window lands here.
/// The window title comes from the panel's rename when one is set, its
/// built-in name otherwise.
pub fn pop_out_view(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    let name = panel
        .tab_name(cx)
        .unwrap_or_else(|| display_name(panel.panel_name(cx)).into());
    let title = SharedString::from(format!("rox - {name}"));
    let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.clone()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title(&title);
        let host = cx.new(|cx| {
            // A popped-out window pumps its own frames, so the backdrop
            // needs its own wake on a new bake.
            let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
            PopoutHost {
                panel_view: panel,
                state,
                backdrop: WindowBackdrop::default(),
                context_menu: None,
                focus: cx.focus_handle(),
                _backdrop_changed,
            }
        });
        // Anchor the window on the fallback focus so the Workspace-scoped
        // playback bindings have a dispatch path before the panel grabs
        // focus, same as the main workspace's fallback.
        host.read(cx).focus.clone().focus(window);
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the panel window");
}

/// The frame-level config every panel carries, flattened into each
/// panel's own config struct with `#[serde(flatten)]`. These are the
/// knobs that mean the same thing on any panel: the rename, the palette
/// override, and the two placement locks. Panel-specific fields (a
/// grid's tile size, a spectrum's bands) stay on the panel's own config;
/// `align` lives there too since only some panels lay out along a row.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PanelChrome {
    /// The rename shown as the tab and title text; None shows the
    /// built-in name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The panel's palette and frame override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
    /// Pin the panel in place: the dock won't let it be dragged to
    /// another spot or rearranged. Off by default. Resizing is a separate
    /// concern the dock handles at the split level.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Turn the panel body into a window-move handle: a drag anywhere on
    /// it moves the OS window, so a decorations-off layout can be moved by
    /// a toolbar strip. Off by default; meant for the quiet panels, since
    /// on an interactive one it competes with the controls.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anchor: bool,
    /// Cap the panel's width in px. Set, the dock won't grow the panel wider
    /// than this, and a growing window hands the extra room to its
    /// neighbors instead, so a toolbar pinned narrow stays narrow. None
    /// leaves the width free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    /// Cap the panel's height in px, the vertical twin of
    /// [`max_width`](Self::max_width): what keeps a menu bar or footer from
    /// stretching when the window gets taller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    /// Hold the panel's width to at least this many px, so a resize can't
    /// squeeze it narrower. Raised over the panel's built-in floor, never
    /// below it. None leaves the width at that floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    /// Hold the panel's height to at least this many px, the vertical twin of
    /// [`min_width`](Self::min_width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
}

/// The panel's size cap as a [`Size`], reading the chrome's optional
/// width/height limits over `floor` (the panel's minimum, so a cap can
/// never drop below what the panel needs). An unset axis stays unbounded.
/// Every panel returns this from its `Panel::max_size`, so the cap is a
/// generic panel setting rather than a per-panel opt-in.
pub fn chrome_max_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let axis = |cap: Option<f32>, floor: Pixels| match cap {
        Some(px_value) => px(px_value).max(floor),
        None => Pixels::MAX,
    };
    gpui::size(
        axis(chrome.max_width, floor.width),
        axis(chrome.max_height, floor.height),
    )
}

/// The panel's minimum size as a [`Size`], the chrome's optional min
/// width/height raised over `floor` (the panel's built-in minimum, what its
/// controls need). A user min can only tighten the floor upward, never below
/// it. An unset axis stays at the floor. Every panel returns this from its
/// `Panel::min_size`, the mirror of [`chrome_max_size`].
pub fn chrome_min_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let axis = |min: Option<f32>, floor: Pixels| match min {
        Some(px_value) => px(px_value).max(floor),
        None => floor,
    };
    gpui::size(
        axis(chrome.min_width, floor.width),
        axis(chrome.min_height, floor.height),
    )
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A panel whose per-view config is edited in its own settings window
/// (see [`crate::panel_settings`]): the panel's own pages of control
/// rows, then the shared Appearance page editing the panel's palette
/// override. New knobs go on the panel's config struct and get a row on
/// one of its pages.
pub trait PanelSettings: Panel {
    /// The shared state, so the settings window can back itself with
    /// the playing track's art like every other window.
    fn state(&self) -> AppState;

    /// The panel's own pages as name and sidebar icon pairs, listed
    /// above the shared Appearance page. Empty means the panel has no
    /// knobs beyond its appearance.
    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// One of the panel's own pages: control rows editing the config in
    /// place. Changes apply live; the layout dump persists them.
    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (page, window, cx);
        div().into_any_element()
    }

    /// The panel's frame-level config. Every panel stores a
    /// [`PanelChrome`] on its own config (flattened into the layout dump),
    /// so the shared knobs - rename, theme, the placement locks - read and
    /// write through here rather than a method per field.
    fn chrome(&self) -> &PanelChrome;

    /// The mutable frame config, so the settings window and quick toggles
    /// edit the shared knobs in place.
    fn chrome_mut(&mut self) -> &mut PanelChrome;

    /// The rename override, shown as the tab and title text in place of
    /// the panel's built-in name.
    fn custom_title(&self) -> Option<&str> {
        self.chrome().title.as_deref()
    }

    /// Store an edited rename: the next render shows it, the layout dump
    /// persists it. None goes back to the built-in name. Implementations
    /// must repaint their hosting tab panel ([`refresh_tab_panel`]), which
    /// is what draws the title, so this stays panel-provided.
    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>);

    /// Whether the panel draws its own font control on its pages, so the
    /// shared Appearance page leaves off the generic theme-font row rather
    /// than showing a second family picker. The lyrics panel does, pairing
    /// the family with its own weight and size knobs.
    fn has_own_font(&self) -> bool {
        false
    }

    /// The panel's palette override, the Appearance page's subject.
    fn theme(&self) -> PanelTheme {
        self.chrome().theme.clone()
    }

    /// Store an edited override: the next render picks it up, the layout
    /// dump persists it.
    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.chrome_mut().theme = theme;
        cx.notify();
    }

    /// Pin or unpin the panel in the dock (no drag or rearrange). The dock
    /// reads the flag through [`Panel::locked`] on its next paint, so a
    /// repaint settles the toggle. The current value reads off
    /// `chrome().locked` directly, which also sidesteps the name clash
    /// with the dock trait's own `locked`.
    fn set_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().locked = on;
        cx.notify();
    }

    /// Turn the window-move handle on or off; `chrome().anchor` reads it.
    fn set_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().anchor = on;
        cx.notify();
    }

    /// Store the panel's width cap in px (None clears it). Repainting the
    /// dock re-reads the cap when it rebuilds the split's size range, so a
    /// repaint settles the change.
    fn set_max_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_width = px;
        cx.notify();
    }

    /// Store the panel's height cap in px (None clears it), the twin of
    /// [`set_max_width`](Self::set_max_width).
    fn set_max_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_height = px;
        cx.notify();
    }

    /// Store the panel's minimum width in px (None clears it), the floor a
    /// resize can't squeeze it below. Same repaint-settles-it path as the
    /// caps.
    fn set_min_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_width = px;
        cx.notify();
    }

    /// Store the panel's minimum height in px (None clears it), the twin of
    /// [`set_min_width`](Self::set_min_width).
    fn set_min_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_height = px;
        cx.notify();
    }

    /// The panel's own rows for the shared Appearance page, rendered as
    /// a section between the frame and the colors: looks that live on
    /// the panel's config rather than its theme, like the grid's art
    /// rounding. None keeps the page to the shared knobs.
    fn appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }

    /// The panel's own rows for the shared Behavior page, rendered under
    /// the shared lock and anchor toggles: knobs about how the panel acts
    /// rather than how it looks, like the grid's follow-playing. None
    /// keeps the page to the shared knobs.
    fn behavior(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }
}

/// Build a panel body under its palette override and keep the override
/// active through every element phase. Building under the scope covers
/// the style reads that resolve eagerly (`.bg(palette::x())` runs as the
/// div chain is built); the wrapper element re-enters it for layout,
/// prepaint, and paint, which is when hover styles and canvas paint
/// closures actually read the palette. The theme's frame knobs apply
/// here too: padding, rounding, and border style the body's root div -
/// the radius must land on the body's own background quad, since gpui
/// content masks stay rectangular and a wrapper's corners would be
/// painted over, and padding on the body keeps the gap in the panel's
/// own background - while margin wraps outside it, so the backdrop
/// shows through that gap. Each knob the theme leaves unset falls back to
/// the app-wide default; an app with no frame set draws none, the look an
/// unthemed panel carried before the knobs were lifted.
pub fn themed(chrome: &PanelChrome, build: impl FnOnce() -> Div) -> AnyElement {
    let theme = &chrome.theme;
    let anchor = chrome.anchor;
    let frame = {
        // The panel's own knob wins where it sets one; unset, the panel
        // takes the app-wide default. Zero reads as no knob either way, so
        // an explicit zero over a rounded app default squares this one
        // panel back off, the same as rounding's absence.
        let app = crate::settings::app_frame();
        let margin = theme.margin.unwrap_or(app.margin);
        let padding = theme.padding.unwrap_or(app.padding);
        let rounding = theme.rounding.unwrap_or(app.rounding);
        let border = theme.border.unwrap_or(app.border);
        let font = theme.font.clone();
        move || {
            let mut body = build();
            // The panel's own font layers over the app font the window root
            // cascades in; unset leaves the app font showing through.
            if let Some(font) = font {
                body = body.font_family(font);
            }
            if padding > 0.0 {
                body = body.p(px(padding));
            }
            if rounding > 0.0 {
                body = body.rounded(px(rounding));
            }
            if border > 0.0 {
                let width: AbsoluteLength = px(border).into();
                let widths = &mut body.style().border_widths;
                widths.top = Some(width);
                widths.right = Some(width);
                widths.bottom = Some(width);
                widths.left = Some(width);
                body = body.border_color(palette::border());
            }
            // The outer element takes layout and, when the panel is an
            // anchor, the window-move drag. A margin wraps the body in an
            // outer cell; without one the body itself is the root.
            let mut root = if margin > 0.0 {
                div().size_full().p(px(margin)).child(body)
            } else {
                body
            };
            if anchor {
                root = root
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move());
            }
            root.into_any_element()
        }
    };
    let Some(scope) = theme.scope() else {
        return frame();
    };
    let child = palette::scoped(&scope, frame);
    Themed { scope, child }.into_any_element()
}

/// The element that carries a panel's palette scope through the render
/// phases. A pure pass-through otherwise: the child's layout is its
/// layout.
struct Themed {
    scope: palette::Scope,
    child: AnyElement,
}

impl Element for Themed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout_id = palette::scoped(&self.scope, || self.child.request_layout(window, cx));
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::scoped(&self.scope, || {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::scoped(&self.scope, || self.child.paint(window, cx));
    }
}

impl IntoElement for Themed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Wraps a window's whole body in its player's art tint, the mirror of
/// [`Themed`] one level up: the palette accessors answer from the tint
/// while the tree is built and again through every paint phase, so a
/// window's panels and canvases read its own playback's colors. Built with
/// [`window_body`], which snapshots the tint and runs the body inside it.
pub struct WindowTint {
    tint: palette::Tint,
    child: AnyElement,
}

/// Build a window body under its player's art tint. The body closure runs
/// with the tint pushed so render-time color reads see it, and the tint
/// rides along into the paint phases through the returned element.
pub fn window_body(player: gpui::EntityId, body: impl FnOnce() -> AnyElement) -> WindowTint {
    let tint = palette::window_tint(player);
    let child = palette::tinted(tint, body);
    WindowTint { tint, child }
}

impl Element for WindowTint {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout_id = palette::tinted(self.tint, || self.child.request_layout(window, cx));
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::tinted(self.tint, || {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::tinted(self.tint, || self.child.paint(window, cx));
    }
}

impl IntoElement for WindowTint {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// One labeled row of a customize window: the setting's name and its
/// control on one line, an optional dimmed description wrapping below.
pub fn setting_row(
    label: &'static str,
    description: Option<&'static str>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label)
                .child(div().flex_none().child(control)),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
}

/// A labeled block of a customize window: like [`setting_row`] but the
/// control spans the full width below the description instead of sitting
/// inline. Wrapping controls need this - the row's control slot is
/// content-sized, and a wrap container without a definite width collapses
/// to one item per line. An optional trailing control rides the label
/// row's right edge, where a section's reset button lives.
pub fn setting_block(
    label: &'static str,
    description: Option<&'static str>,
    trailing: Option<AnyElement>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label)
                .when_some(trailing, |d, trailing| {
                    d.child(div().flex_none().child(trailing))
                }),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
        .child(div().mt(tokens::SPACE_XS).child(control))
}

/// The settings-page sliders' strip width and the readout beside them.
pub const SLIDER_W: Pixels = px(150.);
pub const READOUT_W: Pixels = px(60.);

/// One scalar's slider row: the shared slider chrome over a scrub strip,
/// applying the strip fraction live on click and drag, the readout riding
/// alongside. Callers map the fraction into their own range and format
/// their own readout.
pub fn value_slider<P: 'static>(
    scrub: &ScrubState,
    fraction: f32,
    readout: String,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let entity = cx.entity();
    let strip = div()
        .w(SLIDER_W)
        .h(tokens::CONTROL_H)
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let scrub = scrub.clone();
                let apply = apply.clone();
                move |this: &mut P, event: &MouseDownEvent, _, cx| {
                    scrub.begin();
                    if let Some(fraction) = scrub.fraction(event.position.x) {
                        apply(this, fraction, cx);
                    }
                    cx.notify();
                }
            }),
        )
        .child(
            canvas(
                {
                    let scrub = scrub.clone();
                    move |bounds, _, _| scrub.set_bounds(bounds)
                },
                {
                    let scrub = scrub.clone();
                    move |bounds, _, window, _| {
                        paint_slider(fraction, false, bounds, window);
                        scrub_on_paint(&scrub, window, {
                            let entity = entity.clone();
                            let apply = apply.clone();
                            move |fraction, cx| {
                                entity.update(cx, |this, cx| apply(this, fraction, cx));
                            }
                        });
                    }
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(strip)
        .child(
            div()
                .w(READOUT_W)
                .flex_none()
                .text_right()
                .text_color(palette::text_muted())
                .child(readout),
        )
}

/// An on/off switch: a pill track, the knob in the accent on the far side
/// while on.
pub fn toggle<P: 'static>(
    on: bool,
    on_change: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    div()
        .w(px(34.))
        .h(px(18.))
        .flex_none()
        .rounded_full()
        .bg(palette::bg_control())
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .p(px(2.))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_change(this, !on, cx)),
        )
        .child(div().size(px(14.)).rounded_full().bg(if on {
            palette::accent()
        } else {
            palette::text_faint()
        }))
}

/// The shared "tracking" section for a panel's Behavior page: the
/// follow-playing toggle and, while it is on, the smooth-scrolling toggle,
/// under one header so the library, the grids, and the art shelf all read
/// the same. The wording of what it follows (a row, an album, the center)
/// differs per panel, so both descriptions are passed in; the toggles carry
/// each panel's own follow and glide handlers.
#[allow(clippy::too_many_arguments)]
pub fn tracking_section<P: 'static>(
    follow: bool,
    follow_desc: &'static str,
    on_follow: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    resume: bool,
    resume_desc: &'static str,
    on_resume: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    smooth: bool,
    smooth_desc: &'static str,
    on_smooth: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> AnyElement {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_MD)
        .child(setting_row(
            "Follow Playing",
            Some(follow_desc),
            toggle(follow, on_follow, cx),
        ))
        .child(setting_row(
            "Resume When Idle",
            Some(resume_desc),
            toggle(resume, on_resume, cx),
        ));
    // Both the follow and the resume ride the same glide, so the motion
    // toggle earns its place the moment either is on.
    if follow || resume {
        body = body.child(setting_row(
            "Smooth Scrolling",
            Some(smooth_desc),
            toggle(smooth, on_smooth, cx),
        ));
    }
    crate::settings_ui::section("Tracking", None, body).into_any_element()
}

/// A font-family picker: a small dropdown labeled with the current
/// choice, its menu the installed families over a Default that clears the
/// override back to the app font. `current` is the panel's stored family,
/// None meaning inherit; `apply` stores the pick. Shared so any panel that
/// carries a font override draws the same control - the lyrics panel's
/// typeface knob is the first.
pub fn font_picker<P: 'static>(
    id: &'static str,
    current: Option<String>,
    apply: impl Fn(&mut P, Option<String>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
    let label: SharedString = current
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| "Default".into());
    let mut fonts = cx.text_system().all_font_names();
    fonts.sort();
    fonts.dedup();
    let weak = cx.entity().downgrade();
    Button::new(id)
        .label(label)
        .small()
        .outline()
        .dropdown_menu(move |menu, _, _| {
            let clear = weak.clone();
            let clear_apply = apply.clone();
            let mut menu = menu.item(
                PopupMenuItem::new("Default")
                    .checked(current.is_none())
                    .on_click(move |_, _, cx| {
                        if let Some(this) = clear.upgrade() {
                            let apply = clear_apply.clone();
                            this.update(cx, |this, cx| apply(this, None, cx));
                        }
                    }),
            );
            for name in &fonts {
                let name = name.clone();
                let checked = current.as_deref() == Some(name.as_str());
                let pick = weak.clone();
                let apply = apply.clone();
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        let name = name.clone();
                        let apply = apply.clone();
                        if let Some(this) = pick.upgrade() {
                            this.update(cx, |this, cx| apply(this, Some(name), cx));
                        }
                    },
                ));
            }
            menu
        })
}

/// The chrome shared by the segmented pickers: a joined group of segments,
/// the picked one filled with the accent, hairline gaps between the rest.
fn segments<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    render: impl Fn(&'static str, bool) -> AnyElement,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let last = options.len().saturating_sub(1);
    let mut group = div().flex().flex_row().flex_none().items_center();
    for (i, (key, value)) in options.iter().enumerate() {
        let value = *value;
        let picked = value == current;
        let on_pick = on_pick.clone();
        group = group.child(
            div()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .when(i > 0, |d| d.ml(px(1.)))
                .when(i == 0, |d| d.rounded_l(tokens::RADIUS))
                .when(i == last, |d| d.rounded_r(tokens::RADIUS))
                .bg(if picked {
                    palette::accent()
                } else {
                    palette::bg_control()
                })
                .when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                )
                .child(render(key, picked)),
        );
    }
    group
}

/// A segmented picker of exclusive choices, labeled with text.
pub fn choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |label, picked| {
            div()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .child(label)
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// A segmented picker of exclusive choices, labeled with icons; each option
/// pairs an icon path from [`crate::assets::icons`] with its value.
pub fn icon_choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |icon, picked| {
            svg()
                .path(icon)
                .size_4()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// Where a panel's content sits horizontally, the cross-panel
/// customization knob.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Apply an alignment along a row's main axis.
pub fn justify(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::Right => d.justify_end(),
    }
}

/// Apply an alignment along the cross axis, so a column's children sit
/// left, center, or right the way `justify` places a row's.
pub fn items(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.items_start(),
        Align::Center => d.items_center(),
        Align::Right => d.items_end(),
    }
}

/// The alignment setting row the panels' customize windows share.
pub fn align_row<P: 'static>(
    current: Align,
    on_pick: impl Fn(&mut P, Align, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        "Alignment",
        Some("Where the content sits when the panel has room to spare"),
        icon_choices(
            &[
                (icons::ALIGN_LEFT, Align::Left),
                (icons::ALIGN_CENTER, Align::Center),
                (icons::ALIGN_RIGHT, Align::Right),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}

/// A popped-out panel's window content: the moved panel view, full-size, on
/// the same base styling the workspace root applies. Right-click offers the
/// way back into the dock.
struct PopoutHost {
    panel_view: Arc<dyn PanelView>,
    state: AppState,
    /// This window's slice of the backdrop: what it painted last, for
    /// retiring the texture on a new bake.
    backdrop: WindowBackdrop,
    /// The open right-click menu: its anchor position, the menu, and the
    /// dismiss subscription that clears it.
    context_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Fallback focus so the Workspace-scoped playback bindings keep a
    /// dispatch path in this window even before the hosted panel takes
    /// focus. Mirrors the main workspace's fallback focus.
    focus: FocusHandle,
    _backdrop_changed: Subscription,
}

impl PopoutHost {
    /// Open the right-click menu. Dock Back moves the panel into the newest
    /// live tab group of the workspace and closes this window; cross-window
    /// drags can't work (a held button pins pointer events to its window,
    /// and Wayland hides window positions), so this is the way home.
    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.panel_view.clone();
        let hosts = self.state.tab_hosts.clone();
        let dockable = hosts.read(cx).last_live(cx).is_some();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(
                PopupMenuItem::new("Dock Back")
                    .disabled(!dockable)
                    .on_click(move |_, window, cx| {
                        let Some(tabs) = hosts.read(cx).last_live(cx) else {
                            return;
                        };
                        tabs.update(cx, |tabs, cx| {
                            tabs.add_panel(panel.clone(), window, cx);
                        });
                        window.remove_window();
                    }),
            )
        });
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((position, menu, subscription));
        cx.notify();
    }
}

impl Render for PopoutHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A popped-out window shares its parent's player, so it renders
        // under that playback's tint, and claims the widget theme while it
        // holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                // Same Workspace context and playback actions as the main
                // window, so space and the seek arrows work in a popout too.
                // The panel's own SearchInput context still carves the keys
                // back for its search box.
                .track_focus(&self.focus)
                .key_context("Workspace")
                .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.toggle_pause());
                }))
                .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                    this.state.player.update(cx, |player, _| player.seek_by(-5.0));
                }))
                .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                    this.state.player.update(cx, |player, _| player.seek_by(5.0));
                }))
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.open_menu(event.position, window, cx);
                    }),
                )
                // The backdrop paints first, under the panel; how much shows
                // through is the surfaces' call (ADR 10's strength scalar).
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(self.panel_view.view())
                // Same overlay structure as the dock's context menu: an
                // occluding layer swallows the dismissing click, the anchored
                // child pins the menu to the pointer.
                .when_some(self.context_menu.as_ref(), |this, (position, menu, _)| {
                    this.child(
                        deferred(
                            anchored().child(
                                div()
                                    .w(window.bounds().size.width)
                                    .h(window.bounds().size.height)
                                    .occlude()
                                    .child(
                                        anchored()
                                            .position(*position)
                                            .snap_to_window_with_margin(px(8.))
                                            .child(menu.clone()),
                                    ),
                            ),
                        )
                        .with_priority(1),
                    )
                })
                .into_any_element()
        })
    }
}
