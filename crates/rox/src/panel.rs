//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    anchored, canvas, deferred, div, fill, point, prelude::*, px, size, svg, AbsoluteLength,
    AnyElement, App, Bounds,
    Context, DismissEvent, Div, Element, Entity, Focusable as _, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, SharedString, Subscription, TitlebarOptions, UniformListScrollHandle,
    WeakEntity, Window, WindowBounds, WindowOptions,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Root};
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::lastfm::Scrobbler;
use crate::panels::library::Library;
use crate::player::Player;
use crate::selection::Selection;
use crate::thumbs::Thumbs;

/// The shared entities every panel renders over: one player, one catalog,
/// and one selection per workspace. Cloning shares the handles, not the
/// state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
    pub selection: Entity<Selection>,
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

/// Keep a live drag-scroll following the pointer: scroll by the pointer's
/// delta on every move, end the drag on release. Call from the surface's
/// paint pass, the [`scrub_on_paint`] idiom - window handlers only live
/// one frame. Applying must notify an entity so the next frame re-arms
/// the handlers.
pub fn flick_on_paint(
    flick: &FlickState,
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
            let dy = flick.track(event.position.y);
            if dy != 0.0 {
                apply(dy, cx);
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

/// Pin a uniform list's offset to `target` in one move. Returns true once
/// already there - callers keep re-pinning until the target holds still,
/// which rides out the stale first layouts around a launch, where item
/// extents shift as the measured width lands.
pub fn glide_snap(handle: &UniformListScrollHandle, target: Pixels) -> bool {
    let base = handle.0.borrow().base_handle.clone();
    let mut offset = base.offset();
    if (-offset.y - target).abs() < px(1.) {
        return true;
    }
    offset.y = -target;
    base.set_offset(offset);
    false
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

/// A panel's tab and title text: the rename when one is set, the built-in
/// name otherwise.
pub fn title_text(custom: Option<&str>, default: &'static str) -> SharedString {
    match custom {
        Some(name) => SharedString::from(name.to_owned()),
        None => default.into(),
    }
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

/// The Pop Out entry for a panel's dropdown menu: moves the panel out of its
/// dock into an OS window. Pass the tab panel the panel currently sits in
/// (from `on_added_to`); the state is what Dock Back later reaches the
/// workspace through.
pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
) -> PopupMenu {
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Pop Out")
            .icon(Icon::default().path(icons::EXTERNAL_LINK))
            .on_click(move |_, window, cx| {
                pop_out(panel.clone(), tab_panel.clone(), state.clone(), window, cx);
            }),
    )
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
        .unwrap_or_else(|| panel.panel_name(cx).into());
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
                _backdrop_changed,
            }
        });
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the panel window");
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

    /// The rename override, shown as the tab and title text in place of
    /// the panel's built-in name. Lives on the panel's config, so the
    /// layout dump persists it and Duplicate copies it.
    fn custom_title(&self) -> Option<&str>;

    /// Store an edited rename: the next render shows it, the layout dump
    /// persists it. None goes back to the built-in name. Implementations
    /// must repaint their hosting tab panel ([`refresh_tab_panel`]), which
    /// is what draws the title.
    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>);

    /// The panel's palette override, the Appearance page's subject.
    fn theme(&self) -> PanelTheme;

    /// Store an edited override: the next render picks it up, the layout
    /// dump persists it.
    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>);

    /// The panel's own rows for the shared Appearance page, rendered as
    /// a section between the frame and the colors: looks that live on
    /// the panel's config rather than its theme, like the grid's art
    /// rounding. None keeps the page to the shared knobs.
    fn appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
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
/// shows through that gap. An empty theme skips all of it.
pub fn themed(theme: &PanelTheme, build: impl FnOnce() -> Div) -> AnyElement {
    let frame = {
        let (margin, padding, rounding, border) =
            (theme.margin, theme.padding, theme.rounding, theme.border);
        move || {
            let mut body = build();
            if let Some(padding) = padding {
                body = body.p(px(padding));
            }
            if let Some(radius) = rounding {
                body = body.rounded(px(radius));
            }
            if let Some(width) = border {
                let width: AbsoluteLength = px(width).into();
                let widths = &mut body.style().border_widths;
                widths.top = Some(width);
                widths.right = Some(width);
                widths.bottom = Some(width);
                widths.left = Some(width);
                body = body.border_color(palette::border());
            }
            match margin {
                Some(margin) => div()
                    .size_full()
                    .p(px(margin))
                    .child(body)
                    .into_any_element(),
                None => body.into_any_element(),
            }
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
/// to one item per line.
pub fn setting_block(
    label: &'static str,
    description: Option<&'static str>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(label)
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

/// The alignment setting row the panels' customize windows share.
pub fn align_row<P: 'static>(
    current: Align,
    on_pick: impl Fn(&mut P, Align, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        "alignment",
        Some("where the content sits when the panel has room to spare"),
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
        div()
            .flex()
            .flex_col()
            .size_full()
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
    }
}
