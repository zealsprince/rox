//! The shared signal-pool and route-binding UI, split between two kinds of
//! host. The signals window tends the pool and implements
//! [`SignalHost`] alone; a panel with bindable knobs implements
//! [`RouteHost`] on top of it for its own route list. Both embed a
//! [`SignalUi`] for the widget state: [`bindable_row`] wraps a settings row
//! so a route can drive its knob, [`signals_page`] is the pool editor,
//! [`meter`] is the live readout the tuning rows share, and
//! [`apply_routes`] resolves the routes into whatever [`RouteTargets`] the
//! host exposes each frame. The pool itself is app-wide in [`SignalHub`];
//! edits write through to settings, so a relaunch finds what every open
//! panel was riding.
//!
//! Shader slots don't route through [`bindable_row`] - a slot has no knob
//! of its own to hang a route under, and three different windows edit the
//! same list. That editor is [`routes`], built over a borrowed slice and a
//! write-back closure rather than a host trait.

pub mod routes;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    canvas, div, point, prelude::*, px, size, svg, AnyElement, BorderStyle, Bounds, Context, Div,
    Entity, Focusable as _, MouseButton, MouseDownEvent, Pixels, Rgba, SharedString, Subscription,
    Window,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenuItem};
use gpui_component::{Disableable as _, Icon, Sizable as _};

use rox_viz::signal::{Route, Signal, SignalHub, Source, AGGREGATE_RATE_MAX};

use crate::panel::{self, setting_row, toggle, ScrubState, ValueEdit};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{self as settings_ui, section, SECTION_GAP};

/// The frequency band a signal's tuning sliders pick between, and the
/// smallest span they keep between their bounds: tight enough for a kick,
/// wide enough that the mapping never inverts.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;
const MIN_RATIO: f32 = 1.2;

/// How far past its own setting a route may push a knob: the span reads
/// as a share of what the slider says, and a route is allowed to overshoot
/// it before the knob's own range clamps the result.
const SPAN_OVER: f32 = 4.0;

/// The knobs a host lays open to routes: what is bindable and how a
/// resolved factor lands. What a target id means stays with the host, so
/// a static table and a list read off a live config implement it the
/// same way.
pub trait RouteTargets {
    /// Every bindable target as `(id, label)`, in display order. The shader
    /// panel's Bindings page is built straight off this listing, since its
    /// slots and their names come from the source rather than a table in
    /// the code.
    fn targets(&self) -> Vec<(String, String)>;

    /// Land one resolved factor on the target `id` names. Unknown ids do
    /// nothing, so a config carrying one goes quiet rather than
    /// misfiring.
    fn apply(&mut self, id: &str, value: f32);
}

/// Resolve routes against the hub's live signals into the host's
/// targets. A route's span maps through the same range its target's
/// slider covers, so it can do exactly what a hand on the slider could
/// and nothing more. Later routes to the same target win; routes whose
/// signal is gone contribute nothing.
pub fn apply_routes(routes: &[Route], hub: &SignalHub, targets: &mut impl RouteTargets) {
    for route in routes {
        if !route.enabled {
            continue;
        }
        let Some(signal) = hub.value(route.signal) else {
            continue;
        };
        // The span is a share of the knob's own setting: at full signal a
        // route reaches `to` of what the slider says, at silence `from`.
        // Overshoot past 100% is allowed and the knob's own accessor clamps
        // it to the range the host will take.
        let factor = (route.from + (route.to - route.from) * signal).max(0.0);
        targets.apply(&route.target, factor);
    }
}

/// What the shared widgets need from a hosting panel: the app's hub, the
/// panel's own route list, and the [`SignalUi`] it embeds. The value
/// edit is the host's one panel-wide readout edit, shared so a route
/// slider and the host's own sliders never type at once.
pub trait SignalHost: 'static + Sized {
    fn hub(&self) -> &Arc<SignalHub>;
    fn signal_ui(&self) -> &SignalUi;
    fn signal_ui_mut(&mut self) -> &mut SignalUi;
    fn value_edit(&self) -> &ValueEdit;
    /// The routes this host owns, for the surfaces that report on them.
    /// Defaults to none, which is what the signals window is: it tends
    /// the pool every panel's routes ride and owns no route itself.
    fn routes(&self) -> &[Route] {
        &[]
    }
}

/// A host that owns routes and lets them be edited: what [`bindable_row`]
/// and the inline route editor need on top of [`SignalHost`]. A panel with
/// bindable knobs implements both; a pool-only surface implements the one.
pub trait RouteHost: SignalHost {
    fn routes_mut(&mut self) -> &mut Vec<Route>;
}

/// One route's span sliders, index-aligned with the host's list.
#[derive(Default)]
struct RouteScrubs {
    from: ScrubState,
    to: ScrubState,
}

/// One pool signal's tuning sliders, keyed by signal id since the same
/// signal can be edited from several surfaces.
#[derive(Default)]
struct SignalScrubs {
    lo: ScrubState,
    hi: ScrubState,
    smooth: ScrubState,
    threshold: ScrubState,
    rate: ScrubState,
}

/// The widget state a hosting panel embeds, kept in step with the
/// host's lists by [`sync`].
#[derive(Default)]
pub struct SignalUi {
    route_scrubs: Vec<RouteScrubs>,
    signal_scrubs: HashMap<u64, SignalScrubs>,
    /// The one signal being renamed: the input holding the draft and the
    /// subscription that commits it on Enter. The bounds cell backs the
    /// click-outside cancel, since nothing else in the settings window
    /// takes focus and blur alone never fires.
    rename: Option<(u64, Entity<InputState>, Subscription)>,
    rename_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// The target whose route is expanded inline under its settings row.
    open_bind: Option<String>,
    /// The signal blocks showing their tuning. A pool of any size is a long
    /// page of sliders otherwise, and the name with its meter is what
    /// reading the list is for; the tuning is what editing one is for. A
    /// freshly added signal opens, since adding one is asking to tune it.
    open: HashSet<u64>,
}

/// Keep the route and signal slider state in step with the host's lists.
/// Runs at the top of every settings render, whatever the page: any page
/// can host a route's tuning rows, so a route created from one must find
/// its scrubs on the very next render.
pub fn sync<P: SignalHost>(host: &mut P) {
    let count = host.routes().len();
    let pool = host.hub().pool();
    let ui = host.signal_ui_mut();
    if ui.route_scrubs.len() != count {
        ui.route_scrubs.resize_with(count, RouteScrubs::default);
    }
    ui.signal_scrubs
        .retain(|id, _| pool.iter().any(|s| s.id == *id));
    ui.open.retain(|id| pool.iter().any(|s| s.id == *id));
    for signal in &pool {
        ui.signal_scrubs.entry(signal.id).or_default();
    }
}

/// One open signal's band, for a spectrum to mark: the bounds it listens
/// between, its name, and whether one of those bounds is being dragged
/// right now.
pub struct BandMark {
    pub label: String,
    pub lo: f32,
    pub hi: f32,
    pub dragging: bool,
}

/// The bands the open signal blocks listen between, in pool order. A window
/// drawing a spectrum lays these over it, so the bounds under the sliders
/// are the ones on screen and a band gets picked by eye. Folding a signal
/// away takes its band with it, which is how the spectrum stays readable
/// with a pool of any size. Level and Total have no band of their own and
/// mark nothing.
pub fn open_bands<P: SignalHost>(host: &P) -> Vec<BandMark> {
    let ui = host.signal_ui();
    host.hub()
        .pool()
        .iter()
        .filter(|signal| ui.open.contains(&signal.id))
        .filter_map(|signal| {
            let (lo, hi) = match signal.source {
                Source::Band { lo, hi } | Source::Onset { lo, hi } => (lo, hi),
                Source::Level | Source::Aggregate { .. } => return None,
            };
            let scrubs = ui.signal_scrubs.get(&signal.id);
            Some(BandMark {
                label: signal.label(),
                lo,
                hi,
                dragging: scrubs.is_some_and(|s| s.lo.is_dragging() || s.hi.is_dragging()),
            })
        })
        .collect()
}

/// A strip fraction (0 to 1) as a log-spaced frequency across the slider
/// band, and back. Log so an octave takes the same travel anywhere, the way
/// the spectrum's bounds sliders map.
fn frac_to_hz(fraction: f32) -> f32 {
    SLIDER_MIN_HZ * (SLIDER_MAX_HZ / SLIDER_MIN_HZ).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / SLIDER_MIN_HZ).ln() / (SLIDER_MAX_HZ / SLIDER_MIN_HZ).ln()
}

/// A bound's Hz for the slider readout, compact enough for the strip.
fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz.round())
    }
}

/// The source picker's face for [`Source`], which carries band bounds the
/// segmented control can't.
#[derive(Clone, Copy, PartialEq)]
enum SourceKind {
    Band,
    Level,
    Onset,
    Aggregate,
}

const SOURCE_CHOICES: &[(&str, SourceKind)] = &[
    ("Band", SourceKind::Band),
    ("Level", SourceKind::Level),
    ("Onset", SourceKind::Onset),
    ("Total", SourceKind::Aggregate),
];

/// What a fresh aggregate rides and how fast, for a signal switched to
/// Total with nothing picked yet: the first other signal in the pool at a
/// wrap per second, so the row does something the moment it appears.
const AGGREGATE_RATE: f32 = 1.0;

/// Write the shared pool through to settings once the edit burst settles,
/// the hub's one persistence path, so a relaunch finds what every open
/// panel was riding. The hub already carries every edit live (routes and
/// meters follow the drag instantly), so only the file write waits, the
/// same store-then-settle shape the EQ's curve uses: a settings write
/// reloads and reserializes every shard, and doing that on each tick of a
/// slider scrub stutters the whole app. The generation is global because
/// the pool is: whoever edits, the write they race is the same one.
pub fn persist_pool_soon(hub: &Arc<SignalHub>, cx: &mut gpui::App) {
    static GEN: AtomicU64 = AtomicU64::new(0);
    let mine = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let hub = hub.clone();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(200))
            .await;
        if GEN.load(Ordering::Relaxed) != mine {
            return;
        }
        let pool = hub.pool();
        rox_core::settings::Settings::update(move |s| s.look.bundle.signals = pool);
    })
    .detach();
}

/// Apply one edit to a pool signal through the hub and persist the result.
/// Editing tunes the signal for every route riding it, which is the point
/// of sharing.
fn edit_signal(hub: &Arc<SignalHub>, id: u64, edit: impl FnOnce(&mut Signal), cx: &mut gpui::App) {
    hub.edit(|pool| {
        if let Some(signal) = pool.iter_mut().find(|s| s.id == id) {
            edit(signal);
        }
    });
    persist_pool_soon(hub, cx);
}

/// A thin live meter for the customize window: one signal's value read off
/// the hub at paint time, so tuning happens against what the music is
/// actually sending. The host owns the frame cadence: every meter host
/// re-renders on the pump's notify while audio moves (the signals window
/// observes the player, a panel's settings window observes the panel) and
/// runs its own decay tail, so the meter never asks for frames itself. A
/// self-request here spun every hosting window at monitor refresh for
/// values that only change at the pump's clock.
///
/// The bar is the value before the gate, dimmed while the gate is eating
/// it. A bar that just vanished under the threshold would be no help at
/// all for placing the threshold, which is the one thing this meter is
/// looked at for.
pub fn meter(hub: Arc<SignalHub>, id: u64, fill: Rgba, marker: Option<f32>) -> Div {
    div().h(px(6.)).w_full().child(
        canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let value = hub.raw_value(id).unwrap_or(0.0).clamp(0.0, 1.0);
                // How far the gate is open, read back off the two values
                // rather than asked for: what leaves over what the engine
                // holds is exactly the gate. The bar fades with it, so the
                // ramp shows as the bar dimming rather than a switch.
                let open = if value > 1e-4 {
                    (hub.value(id).unwrap_or(0.0) / value).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let radius = bounds.size.height / 2.0;
                window.paint_quad(gpui::quad(
                    bounds,
                    radius,
                    palette::bg_control(),
                    0.,
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
                if value > 0.0 {
                    window.paint_quad(gpui::quad(
                        Bounds::new(
                            bounds.origin,
                            size(bounds.size.width * value, bounds.size.height),
                        ),
                        radius,
                        palette::alpha(fill, (70.0 + 140.0 * open) as u8),
                        0.,
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
                if let Some(marker) = marker {
                    window.paint_quad(gpui::quad(
                        Bounds::new(
                            point(
                                bounds.origin.x + bounds.size.width * marker - px(0.75),
                                bounds.origin.y,
                            ),
                            size(px(1.5), bounds.size.height),
                        ),
                        0.,
                        palette::text_faint(),
                        0.,
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            },
        )
        .size_full(),
    )
}

/// One chip of a binding's scope row: the segmented control's look, built
/// by hand because the scope list follows the live pool, which the static
/// segmented options can't carry. Open to any view, not just a
/// [`SignalHost`]: the panel settings window's Shader page picks slots and
/// signals with the same chips while holding its panel weakly.
pub fn scope_chip<P: 'static>(
    label: String,
    picked: bool,
    on_pick: impl Fn(&mut P, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    div()
        .px(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(if picked {
            palette::accent()
        } else {
            palette::bg_control()
        })
        .when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
        .text_color(if picked {
            palette::text_on_accent()
        } else {
            palette::text()
        })
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_pick(this, cx)),
        )
        .child(label)
}

/// Attach the row's route to ride `signal`, repointing an existing
/// route rather than stacking a second, and open its editor.
fn attach_signal<P: RouteHost>(host: &mut P, target: String, signal: u64, cx: &mut Context<P>) {
    if let Some(route) = host
        .routes_mut()
        .iter_mut()
        .rev()
        .find(|r| r.target == target)
    {
        route.signal = signal;
    } else {
        host.routes_mut().push(Route {
            signal,
            target: target.clone(),
            ..Route::default()
        });
    }
    host.signal_ui_mut().open_bind = Some(target);
    cx.notify();
}

/// The context menu's deliberate "Add Signal": a fresh pool signal,
/// routed to the row on the spot.
fn attach_new_signal<P: RouteHost>(host: &mut P, target: String, cx: &mut Context<P>) {
    let (id, _) = host.hub().add(
        Source::Band {
            lo: 30.0,
            hi: 120.0,
        },
        0.3,
    );
    persist_pool_soon(host.hub(), cx);
    attach_signal(host, target, id, cx);
}

/// Start renaming a signal: an input seeded with the given name (not
/// the derived label, so clearing the field is how a name goes back
/// to following the source). Enter commits, clicking away cancels.
fn begin_rename<P: SignalHost>(host: &mut P, id: u64, window: &mut Window, cx: &mut Context<P>) {
    let current = host
        .hub()
        .pool()
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Signal name")
            .default_value(current)
    });
    let sub = cx.subscribe_in(
        &input,
        window,
        move |this: &mut P, input, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().trim().to_string();
                edit_signal(this.hub(), id, |signal| signal.name = name, cx);
                this.signal_ui_mut().rename = None;
                cx.notify();
            }
            InputEvent::Blur => {
                this.signal_ui_mut().rename = None;
                cx.notify();
            }
            _ => {}
        },
    );
    window.focus(&input.read(cx).focus_handle(cx));
    host.signal_ui_mut().rename = Some((id, input, sub));
    cx.notify();
}

/// Fold a signal's tuning away, or bring it back.
fn toggle_signal<P: SignalHost>(host: &mut P, id: u64, cx: &mut Context<P>) {
    let ui = host.signal_ui_mut();
    if !ui.open.remove(&id) {
        ui.open.insert(id);
    }
    cx.notify();
}

fn remove_route<P: RouteHost>(host: &mut P, index: usize, cx: &mut Context<P>) {
    if index < host.routes().len() {
        host.routes_mut().remove(index);
        cx.notify();
    }
}

/// Drop a signal from the shared pool. Routes riding it stay where
/// they are and go quiet, so re-adding or repointing restores them.
fn remove_signal<P: SignalHost>(host: &mut P, id: u64, cx: &mut Context<P>) {
    host.hub().edit(|pool| pool.retain(|s| s.id != id));
    persist_pool_soon(host.hub(), cx);
    cx.notify();
}

/// The Signals page: the app's shared pool, which is why it hangs off a
/// window of its own rather than one panel's settings. Routes live inline
/// under the knobs they drive; this page is where the signals themselves
/// are tuned, and an edit lands on every route riding the signal, in
/// every panel.
pub fn signals_page<P: SignalHost>(host: &P, cx: &mut Context<P>) -> Div {
    let pool = host.hub().pool();
    let add = settings_ui::small_button(
        "Add Signal",
        icons::PLUS,
        false,
        cx.listener(|this: &mut P, _, _, cx| {
            let (id, _) = this.hub().add(
                Source::Band {
                    lo: 30.0,
                    hi: 120.0,
                },
                0.3,
            );
            // Open on arrival: a new signal is a band nobody has picked
            // yet, and a collapsed row of defaults is nothing to look at.
            this.signal_ui_mut().open.insert(id);
            persist_pool_soon(this.hub(), cx);
            cx.notify();
        }),
    );
    let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
    if pool.is_empty() {
        list = list.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child("No signals yet - add one, or right-click any bindable knob."),
        );
    }
    for signal in &pool {
        list = list.child(signal_block(host, signal.id, cx));
    }
    div().flex().flex_col().gap(SECTION_GAP).child(section(
        "Signals",
        Some(add.into_any_element()),
        list,
    ))
}

/// One pool signal's block on the Signals page: its derived name, the
/// live meter, its tuning, how many of this panel's routes ride it,
/// and the delete that lets those routes go quiet.
///
/// The name and the meter always show and the tuning folds under them. A
/// pool gets read by name and watched by meter, and it only ever gets
/// edited one signal at a time.
fn signal_block<P: SignalHost>(host: &P, id: u64, cx: &mut Context<P>) -> Div {
    let pool = host.hub().pool();
    let Some(signal) = pool.iter().find(|s| s.id == id) else {
        return div();
    };
    let riders = host.routes().iter().filter(|r| r.signal == id).count();
    let open = host.signal_ui().open.contains(&id);
    let renaming = matches!(&host.signal_ui().rename, Some((rid, _, _)) if *rid == id);
    // While this signal is being renamed the label swaps for the
    // input; committing or clicking away swaps it back. A one-frame
    // window handler cancels on any press outside the field.
    let name: AnyElement = match &host.signal_ui().rename {
        Some((rid, input, _)) if *rid == id => {
            let entity = cx.entity();
            let cell = host.signal_ui().rename_bounds.clone();
            div()
                .relative()
                .w(px(180.))
                .child(
                    canvas(
                        {
                            let cell = cell.clone();
                            move |bounds, _, _| *cell.lock().unwrap() = Some(bounds)
                        },
                        move |_, _, window, _| {
                            let cell = cell.clone();
                            let entity = entity.clone();
                            window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                                if !phase.bubble() {
                                    return;
                                }
                                let inside = cell
                                    .lock()
                                    .unwrap()
                                    .is_some_and(|b| b.contains(&event.position));
                                if inside {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if this.signal_ui().rename.is_some() {
                                        this.signal_ui_mut().rename = None;
                                        cx.notify();
                                    }
                                });
                            });
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(Input::new(input).small().w_full())
                .into_any_element()
        }
        _ => div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(signal.label())
            .into_any_element(),
    };
    // The chevron and the name take the fold's click; the controls at the
    // other edge stay out of it, since a press anywhere in this strip
    // would otherwise fold the block on its way to the pencil. A rename
    // takes the click away entirely, or clicking into the field would fold
    // the block away under it.
    let name = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(
            svg()
                .path(if open {
                    icons::CHEVRON_DOWN
                } else {
                    icons::CHEVRON_RIGHT
                })
                .size(px(12.))
                .flex_none()
                .text_color(palette::text_muted()),
        )
        .child(name)
        .when(!renaming, |d| {
            d.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this: &mut P, _, _, cx| toggle_signal(this, id, cx)),
            )
        });
    let header = settings_ui::block_header(
        name,
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            // Only where there are routes to count. A host with none is
            // either a panel that hasn't bound anything, which has nothing
            // to report, or the signals window, which owns no routes at
            // all and would be claiming the pool goes nowhere.
            .when(riders > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(match riders {
                            1 => "1 route in this panel".to_string(),
                            n => format!("{n} routes in this panel"),
                        }),
                )
            })
            .child(settings_ui::icon_button(
                icons::PENCIL,
                false,
                cx.listener(move |this: &mut P, _, window, cx| begin_rename(this, id, window, cx)),
            ))
            .child(settings_ui::icon_button(
                icons::TRASH,
                false,
                cx.listener(move |this: &mut P, _, _, cx| remove_signal(this, id, cx)),
            )),
    );
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(header)
        // The gate rides the meter as a mark, so it gets placed against
        // the level it's judging rather than by the percentage alone.
        .child(meter(
            host.hub().clone(),
            id,
            palette::accent(),
            gate_mark(signal),
        ))
        .when(open, |d| d.child(signal_tuning(host, id, cx)))
}

/// One shared signal's tuning rows: what it listens to and how it
/// responds. Edits go through the hub, so every route riding it, in
/// every panel, follows.
fn signal_tuning<P: SignalHost>(host: &P, id: u64, cx: &mut Context<P>) -> Div {
    let pool = host.hub().pool();
    let Some(signal) = pool.iter().find(|s| s.id == id) else {
        return div();
    };
    let Some(scrubs) = host.signal_ui().signal_scrubs.get(&id) else {
        return div();
    };
    let (kind, freq_lo, freq_hi) = match signal.source {
        Source::Band { lo, hi } => (SourceKind::Band, lo, hi),
        Source::Onset { lo, hi } => (SourceKind::Onset, lo, hi),
        Source::Level => (SourceKind::Level, 30.0, 120.0),
        Source::Aggregate { .. } => (SourceKind::Aggregate, 30.0, 120.0),
    };
    let smooth = signal.smooth.clamp(0.0, 1.0);
    let threshold = signal.threshold();
    // A total watches a signal rather than a spectrum, so the band, the
    // response and the gate all belong to the signal it follows; its own
    // rows are what it follows and how fast.
    let spectral = kind != SourceKind::Aggregate;
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            "Source",
            Some(
                "What the signal listens to: Band follows one frequency range, \
                 Level the whole mix, Onset pulses on each hit in the range, \
                 Total adds up another signal over time",
            ),
            panel::choices(
                SOURCE_CHOICES,
                kind,
                move |this: &mut P, kind, cx| {
                    // Switching kinds carries the band along, so Band
                    // to Onset keeps the range the ear already picked. A
                    // fresh Total rides whatever else is in the pool,
                    // since one following nothing would sit at zero with
                    // no hint why.
                    let first_other = this
                        .hub()
                        .pool()
                        .iter()
                        .find(|s| s.id != id && s.aggregate().is_none())
                        .map(|s| s.id)
                        .unwrap_or(0);
                    edit_signal(
                        this.hub(),
                        id,
                        |signal| {
                            let (lo, hi) = match signal.source {
                                Source::Band { lo, hi } | Source::Onset { lo, hi } => (lo, hi),
                                Source::Level | Source::Aggregate { .. } => (30.0, 120.0),
                            };
                            let (of, rate) = match signal.source {
                                Source::Aggregate { of, rate } => (of, rate),
                                _ => (first_other, AGGREGATE_RATE),
                            };
                            signal.source = match kind {
                                SourceKind::Band => Source::Band { lo, hi },
                                SourceKind::Onset => Source::Onset { lo, hi },
                                SourceKind::Level => Source::Level,
                                SourceKind::Aggregate => Source::Aggregate { of, rate },
                            };
                        },
                        cx,
                    );
                    cx.notify();
                },
                cx,
            ),
        ))
        .when(!spectral, |d| aggregate_rows(d, host, id, cx))
        .when(spectral && kind != SourceKind::Level, |d| {
            d.child(setting_row(
                "Low Bound",
                None,
                panel::value_slider_edit(
                    &scrubs.lo,
                    host.value_edit(),
                    hz_to_frac(freq_lo),
                    fmt_hz(freq_lo),
                    format!("{freq_lo:.0}"),
                    hz_to_frac,
                    move |this: &mut P, fraction, cx| {
                        edit_signal(
                            this.hub(),
                            id,
                            |signal| {
                                if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                    &mut signal.source
                                {
                                    let ceil = (*hi / MIN_RATIO).max(SLIDER_MIN_HZ);
                                    *lo = frac_to_hz(fraction).clamp(SLIDER_MIN_HZ, ceil);
                                }
                            },
                            cx,
                        );
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "High Bound",
                None,
                panel::value_slider_edit(
                    &scrubs.hi,
                    host.value_edit(),
                    hz_to_frac(freq_hi),
                    fmt_hz(freq_hi),
                    format!("{freq_hi:.0}"),
                    hz_to_frac,
                    move |this: &mut P, fraction, cx| {
                        edit_signal(
                            this.hub(),
                            id,
                            |signal| {
                                if let Source::Band { lo, hi } | Source::Onset { lo, hi } =
                                    &mut signal.source
                                {
                                    let floor = (*lo * MIN_RATIO).min(SLIDER_MAX_HZ);
                                    *hi = frac_to_hz(fraction).clamp(floor, SLIDER_MAX_HZ);
                                }
                            },
                            cx,
                        );
                        cx.notify();
                    },
                    cx,
                ),
            ))
        })
        .when(spectral, |d| {
            d.child(setting_row(
                "Response",
                Some(if kind == SourceKind::Onset {
                    "How long each pulse rings before it dies away"
                } else {
                    "0 snaps to the music, 100 drifts after it"
                }),
                panel::value_slider_edit(
                    &scrubs.smooth,
                    host.value_edit(),
                    smooth,
                    format!("{}%", (smooth * 100.0).round() as i32),
                    format!("{}", (smooth * 100.0).round() as i32),
                    |v| v / 100.0,
                    move |this: &mut P, fraction, cx| {
                        edit_signal(
                            this.hub(),
                            id,
                            |signal| {
                                signal.smooth = fraction.clamp(0.0, 1.0);
                            },
                            cx,
                        );
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "Threshold",
                Some(
                    "Under this the signal reads as nothing, and above it the output \
                     climbs from zero again, so the quiet parts leave the knob alone; \
                     the mark on the meter above is where it sits",
                ),
                panel::value_slider_edit(
                    &scrubs.threshold,
                    host.value_edit(),
                    threshold,
                    format!("{}%", (threshold * 100.0).round() as i32),
                    format!("{}", (threshold * 100.0).round() as i32),
                    |v| v / 100.0,
                    move |this: &mut P, fraction, cx| {
                        edit_signal(
                            this.hub(),
                            id,
                            |signal| {
                                signal.threshold = fraction.clamp(0.0, 1.0);
                            },
                            cx,
                        );
                        cx.notify();
                    },
                    cx,
                ),
            ))
        })
}

/// A total's own rows: which signal it adds up, how fast it climbs, and
/// whether a new song sends it back to zero. The gate and the response
/// stay off the list on purpose, since both belong to the signal it
/// follows and setting them twice would be two answers to one question.
fn aggregate_rows<P: SignalHost>(col: Div, host: &P, id: u64, cx: &mut Context<P>) -> Div {
    let pool = host.hub().pool();
    let Some(signal) = pool.iter().find(|s| s.id == id) else {
        return col;
    };
    let Some((of, rate)) = signal.aggregate() else {
        return col;
    };
    let Some(scrubs) = host.signal_ui().signal_scrubs.get(&id) else {
        return col;
    };
    let reset = signal.reset_on_track;

    // A dropdown rather than the route editor's chips: the pool grows
    // without limit and a wrapping row of every other signal takes over the
    // block it sits in. Aggregates are offered too - one total over another
    // is a second integral, which is strange but not wrong, and it reads
    // last frame's value so a ring just sits still.
    let others: Vec<(u64, String)> = pool
        .iter()
        .filter(|s| s.id != id)
        .map(|s| (s.id, s.label()))
        .collect();
    let alone = others.is_empty();
    let known = others.iter().any(|(other, _)| *other == of);
    let label = others
        .iter()
        .find(|(other, _)| *other == of)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| {
            if alone {
                "Nothing to follow".to_string()
            } else {
                "Pick a signal".to_string()
            }
        });
    let button = Button::new(SharedString::from(format!("aggregate-of-{id}")))
        .label(label)
        .small()
        .outline()
        .dropdown_caret(true);
    // Nothing to pick, so the button says so and takes no press rather
    // than opening an empty menu.
    let picker = if alone {
        button.disabled(true).into_any_element()
    } else {
        let weak = cx.entity().downgrade();
        button
            .dropdown_menu(move |mut menu, _, _| {
                for (pick, label) in &others {
                    let (pick, host) = (*pick, weak.clone());
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(pick == of)
                            .on_click(move |_, _, cx| {
                                let Some(host) = host.upgrade() else {
                                    return;
                                };
                                host.update(cx, |this: &mut P, cx| {
                                    edit_signal(
                                        this.hub(),
                                        id,
                                        |signal| {
                                            if let Source::Aggregate { of, .. } = &mut signal.source
                                            {
                                                *of = pick;
                                            }
                                        },
                                        cx,
                                    );
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    };

    col.child(setting_row(
        "Adds Up",
        Some("Which signal this totals; it climbs while that one reads high and stalls while it's quiet"),
        picker,
    ))
    .when(alone, |d| {
        d.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(
                    "There's no other signal in the pool for this to add up, so it sits at \
                     zero. Add one and it shows up in the list.",
                ),
        )
    })
    .when(!alone && !known, |d| {
        d.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child("Nothing picked, so this total sits at zero. Pick a signal above."),
        )
    })
    .child(setting_row(
        "Rate",
        Some("Wraps per second at full input; it rolls over 1 back to 0 and keeps climbing, which a shader reads as a phase"),
        panel::value_slider_edit_over(
            &scrubs.rate,
            host.value_edit(),
            rate / AGGREGATE_RATE_MAX,
            format!("{rate:.2}/s"),
            format!("{rate:.2}"),
            1.0,
            |v| v / AGGREGATE_RATE_MAX,
            move |this: &mut P, fraction, cx| {
                edit_signal(
                    this.hub(),
                    id,
                    |signal| {
                        if let Source::Aggregate { rate, .. } = &mut signal.source {
                            *rate = (fraction * AGGREGATE_RATE_MAX).clamp(0.0, AGGREGATE_RATE_MAX);
                        }
                    },
                    cx,
                );
                cx.notify();
            },
            cx,
        ),
    ))
    .child(setting_row(
        "Reset on Track",
        Some("Drain back to zero when a new song starts, so a phase doesn't carry the last one's total into it"),
        toggle(
            reset,
            move |this: &mut P, on, cx| {
                edit_signal(this.hub(), id, |signal| signal.reset_on_track = on, cx);
                cx.notify();
            },
            cx,
        ),
    ))
    .child(setting_row(
        "Flush",
        Some("Send it back to zero now; it drains over a moment rather than snapping, so nothing riding it jumps"),
        settings_ui::small_button(
            "Flush",
            icons::REFRESH_CW,
            false,
            cx.listener(move |this: &mut P, _, _, cx| {
                this.hub().flush(id);
                cx.notify();
            }),
        ),
    ))
}

/// Where a signal's gate sits on its meter, or None with the gate off, so
/// a signal nobody has thresholded draws a clean bar.
fn gate_mark(signal: &Signal) -> Option<f32> {
    let threshold = signal.threshold();
    (threshold > 0.0).then_some(threshold)
}

/// One route's tuning rows for the inline editor: which shared signal
/// it rides (with the pool as a picker), that signal's tuning in
/// place, and the span it sweeps. A route whose signal is gone says so
/// and waits for a repoint instead of pretending.
fn route_tuning<P: RouteHost>(host: &P, index: usize, cx: &mut Context<P>) -> Div {
    let route = &host.routes()[index];
    let scrubs = &host.signal_ui().route_scrubs[index];
    let pool = host.hub().pool();
    let known = pool.iter().any(|s| s.id == route.signal);
    let from = route.from.clamp(0.0, SPAN_OVER);
    let to = route.to.clamp(0.0, SPAN_OVER);

    let mut chips = div().flex().flex_row().flex_wrap().gap(px(1.));
    for signal in &pool {
        let id = signal.id;
        chips = chips.child(scope_chip(
            signal.label(),
            known && route.signal == id,
            move |this: &mut P, cx| {
                if let Some(route) = this.routes_mut().get_mut(index) {
                    route.signal = id;
                }
                cx.notify();
            },
            cx,
        ));
    }
    chips = chips.child(scope_chip(
        "New Signal".to_string(),
        false,
        move |this: &mut P, cx| {
            let (id, _) = this.hub().add(
                Source::Band {
                    lo: 30.0,
                    hi: 120.0,
                },
                0.3,
            );
            persist_pool_soon(this.hub(), cx);
            if let Some(route) = this.routes_mut().get_mut(index) {
                route.signal = id;
            }
            cx.notify();
        },
        cx,
    ));

    let mut col = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(panel::setting_block(
            "Signal",
            Some("Which shared signal this route rides; tuning it here tunes every route on it"),
            None,
            chips,
        ));
    if known {
        col = col
            .child(meter(
                host.hub().clone(),
                route.signal,
                palette::accent(),
                pool.iter()
                    .find(|s| s.id == route.signal)
                    .and_then(gate_mark),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child("Shared by every route on this signal"),
            )
            .child(signal_tuning(host, route.signal, cx));
    } else {
        col = col.child(div().text_xs().text_color(palette::text_muted()).child(
            "This route's signal is gone; the knob holds its slider value \
                    until another is picked above.",
        ));
    }
    // The span belongs to this route alone, where everything above it
    // is the shared signal: the same signal can pull one knob all the
    // way and nudge another, so the two halves are labelled apart.
    col.child(
        div()
            .pt(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_faint())
            .child("Range for this parameter only"),
    )
    .child(setting_row(
        "Quiet",
        Some("What the knob reaches at silence, as a share of its own setting"),
        panel::value_slider_edit_over(
            &scrubs.from,
            host.value_edit(),
            from,
            format!("{}%", (from * 100.0).round() as i32),
            format!("{}", (from * 100.0).round() as i32),
            SPAN_OVER,
            |v| v / 100.0,
            move |this: &mut P, fraction, cx| {
                if let Some(route) = this.routes_mut().get_mut(index) {
                    route.from = fraction.clamp(0.0, SPAN_OVER);
                }
                cx.notify();
            },
            cx,
        ),
    ))
    .child(setting_row(
        "Loud",
        Some("What it reaches at full signal; 100% is the slider's own value, below Quiet modulates down"),
        panel::value_slider_edit_over(
            &scrubs.to,
            host.value_edit(),
            to,
            format!("{}%", (to * 100.0).round() as i32),
            format!("{}", (to * 100.0).round() as i32),
            SPAN_OVER,
            |v| v / 100.0,
            move |this: &mut P, fraction, cx| {
                if let Some(route) = this.routes_mut().get_mut(index) {
                    route.to = fraction.clamp(0.0, SPAN_OVER);
                }
                cx.notify();
            },
            cx,
        ),
    ))
}

/// A settings row whose knob a route can drive: the row itself with a
/// bind toggle at its edge, and the route's tuning expanded beneath
/// while open. The slider keeps working while bound, since the route's
/// span is a share of it: the slider sets what full signal reaches and
/// the span decides how far the music pulls it back. Clicking the
/// toggle on an unbound row creates the route on the spot, and a
/// right-click anywhere on the row's control does the same, so binding
/// never needs the little icon found first. Removing the route lives
/// on the trash inside the expanded editor.
pub fn bindable_row<P: RouteHost>(
    host: &P,
    label: &'static str,
    description: Option<&'static str>,
    target: String,
    control: Div,
    cx: &mut Context<P>,
) -> Div {
    let bound = host.routes().iter().rposition(|r| r.target == target);
    let open = host.signal_ui().open_bind.as_deref() == Some(target.as_str());
    let weak = cx.entity().downgrade();
    let menu_target = target.clone();
    let control = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        // Right-click routes: pick a pool signal to ride, or add one
        // deliberately. The menu shows even over an empty pool, so the
        // way in is never invisible.
        .context_menu(move |mut menu, _, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            let pool = this.read(cx).hub().pool();
            for signal in &pool {
                let id = signal.id;
                let panel = weak.clone();
                let target = menu_target.clone();
                menu = menu.item(
                    PopupMenuItem::new(signal.label()).on_click(move |_, _, cx| {
                        if let Some(this) = panel.upgrade() {
                            this.update(cx, |this, cx| attach_signal(this, target.clone(), id, cx));
                        }
                    }),
                );
            }
            if !pool.is_empty() {
                menu = menu.separator();
            }
            let panel = weak.clone();
            let target = menu_target.clone();
            menu.item(
                PopupMenuItem::new("Add Signal")
                    .icon(Icon::default().path(icons::PLUS))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = panel.upgrade() {
                            this.update(cx, |this, cx| attach_new_signal(this, target.clone(), cx));
                        }
                    }),
            )
        })
        // The slider keeps its full weight while bound: it is what the
        // route's span is a share of, so it still sets the ceiling.
        .child(control)
        // The bind mark only exists once a route does; an unbound row
        // keeps an empty slot the same size so the sliders stay in
        // column, and the context menu is the way in.
        .map(|d| {
            if bound.is_some() {
                d.child(settings_ui::icon_button(
                    icons::AUDIO_WAVEFORM,
                    false,
                    cx.listener({
                        let target = target.clone();
                        move |this: &mut P, _, _, cx| {
                            let open =
                                this.signal_ui().open_bind.as_deref() == Some(target.as_str());
                            this.signal_ui_mut().open_bind =
                                if open { None } else { Some(target.clone()) };
                            cx.notify();
                        }
                    }),
                ))
            } else {
                d.child(
                    div()
                        .flex_none()
                        .w(tokens::SPACE_XS * 2.0 + px(14.))
                        .h(px(14.)),
                )
            }
        });
    // The context menu keys its open state on the element id path, and
    // `context_menu` names every one of them the same thing. Several
    // bindable rows on a page would land on one shared state, rendering
    // one menu entity in several places and swallowing its clicks, so
    // each row's control sits under an id of its own.
    let control = div()
        .id(SharedString::from(format!("bind-row-{target}")))
        .child(control);
    let mut row = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(panel::setting_row(label, description, control));
    if open {
        if let Some(index) = bound {
            let header = settings_ui::block_header(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("Route"),
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .child(toggle(
                        host.routes()[index].enabled,
                        move |this: &mut P, on, cx| {
                            if let Some(route) = this.routes_mut().get_mut(index) {
                                route.enabled = on;
                            }
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(settings_ui::icon_button(
                        icons::TRASH,
                        false,
                        cx.listener(move |this: &mut P, _, _, cx| {
                            this.signal_ui_mut().open_bind = None;
                            remove_route(this, index, cx);
                        }),
                    )),
            );
            row = row.child(settings_ui::nested(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .child(header)
                    .child(route_tuning(host, index, cx)),
            ));
        }
    }
    row
}
