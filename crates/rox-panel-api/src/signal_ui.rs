//! The shared signal-pool and route-binding UI. The signals window tends
//! the pool and implements [`SignalHost`] alone; a panel with bindable
//! knobs implements [`RouteHost`] on top for its own route list. The pool
//! is app-wide in [`SignalHub`] and edits write through to settings.
//!
//! Shader slots don't route through [`bindable_row`]: a slot has no knob to
//! hang a route under. Their editor is [`routes`], with [`slots`] under it.

pub mod routes;
pub mod slots;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, BorderStyle, Bounds, Context, Div, Entity, Focusable as _, MouseButton,
    MouseDownEvent, Pixels, Rgba, SharedString, Subscription, Window, canvas, div, point,
    prelude::*, px, size, svg,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenuItem};
use gpui_component::{Disableable as _, Icon, Sizable as _};

use rox_viz::signal::{AGGREGATE_RATE_MAX, Route, Signal, SignalHub, Source};

use crate::panel::{self, ScrubState, ValueEdit, setting_row, toggle};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};

/// The tuning sliders' band, and the smallest ratio kept between a
/// signal's bounds so the mapping never inverts.
const SLIDER_MIN_HZ: f32 = 20.0;
const SLIDER_MAX_HZ: f32 = 20_000.0;
const MIN_RATIO: f32 = 1.2;

/// How far past its own setting a route may push a knob before the knob's
/// range clamps it.
const SPAN_OVER: f32 = 4.0;

/// The knobs a host exposes to routes. What a target id means stays with
/// the host.
pub trait RouteTargets {
    /// Every bindable target as `(id, label)`, in display order.
    fn targets(&self) -> Vec<(String, String)>;

    /// Unknown ids must do nothing, so a stale config goes quiet rather
    /// than misfiring.
    fn apply(&mut self, id: &str, value: f32);
}

/// Resolve routes against the hub's live signals into the host's targets.
/// Later routes to the same target win; routes whose signal is gone
/// contribute nothing.
pub fn apply_routes(routes: &[Route], hub: &SignalHub, targets: &mut impl RouteTargets) {
    for route in routes {
        if !route.enabled {
            continue;
        }
        let Some(signal) = hub.value(route.signal) else {
            continue;
        };
        // A share of the knob's own setting. Overshoot past 100% is allowed;
        // the knob's accessor clamps it.
        let factor = (route.from + (route.to - route.from) * signal).max(0.0);
        targets.apply(&route.target, factor);
    }
}

/// What the shared widgets need from a host. The value edit is the host's
/// one panel-wide readout edit, so a route slider and the host's own
/// sliders never type at once.
pub trait SignalHost: 'static + Sized {
    fn hub(&self) -> &Arc<SignalHub>;
    fn signal_ui(&self) -> &SignalUi;
    fn signal_ui_mut(&mut self) -> &mut SignalUi;
    fn value_edit(&self) -> &ValueEdit;
    /// Defaults to none, the signals window's case.
    fn routes(&self) -> &[Route] {
        &[]
    }
}

pub trait RouteHost: SignalHost {
    fn routes_mut(&mut self) -> &mut Vec<Route>;
}

/// Index-aligned with the host's route list.
#[derive(Default)]
struct RouteScrubs {
    from: ScrubState,
    to: ScrubState,
}

#[derive(Default)]
struct SignalScrubs {
    lo: ScrubState,
    hi: ScrubState,
    smooth: ScrubState,
    threshold: ScrubState,
    rate: ScrubState,
}

/// The widget state a host embeds, kept in step by [`sync`].
#[derive(Default)]
pub struct SignalUi {
    route_scrubs: Vec<RouteScrubs>,
    signal_scrubs: HashMap<u64, SignalScrubs>,
    /// The bounds cell backs the click-outside cancel: nothing else in the
    /// settings window takes focus, so blur alone never fires.
    rename: Option<(u64, Entity<InputState>, Subscription)>,
    rename_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    open_bind: Option<String>,
    /// The signal blocks showing their tuning.
    open: HashSet<u64>,
}

/// Run at the top of every settings render, whatever the page: a route
/// created from any page must find its scrubs on the next render.
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

/// One open signal's band, for a spectrum to mark.
pub struct BandMark {
    pub label: String,
    pub lo: f32,
    pub hi: f32,
    pub dragging: bool,
}

/// The bands of the open signal blocks, in pool order, for a spectrum to
/// overlay. Only open ones, so the spectrum stays readable with a big pool.
pub fn open_bands<P: SignalHost>(host: &P) -> Vec<BandMark> {
    let ui = host.signal_ui();
    host.hub()
        .pool()
        .iter()
        .filter(|signal| ui.open.contains(&signal.id))
        .filter_map(|signal| {
            let (lo, hi) = match signal.source {
                Source::Band { lo, hi } | Source::Onset { lo, hi } | Source::Trigger { lo, hi } => {
                    (lo, hi)
                }
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

/// Log-spaced, so an octave takes the same travel anywhere.
fn frac_to_hz(fraction: f32) -> f32 {
    SLIDER_MIN_HZ * (SLIDER_MAX_HZ / SLIDER_MIN_HZ).powf(fraction.clamp(0.0, 1.0))
}

fn hz_to_frac(hz: f32) -> f32 {
    (hz / SLIDER_MIN_HZ).ln() / (SLIDER_MAX_HZ / SLIDER_MIN_HZ).ln()
}

fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        rox_i18n::format::format_unit(hz as f64 / 1000.0, 1, "kHz")
    } else {
        rox_i18n::format::format_unit(hz as f64, 0, "Hz")
    }
}

/// The source picker's face for [`Source`], minus the band bounds.
#[derive(Clone, Copy, PartialEq)]
enum SourceKind {
    Band,
    Level,
    Onset,
    Trigger,
    Aggregate,
}

/// Built per call so the labels follow the live locale.
fn source_choices() -> Vec<(SharedString, SourceKind)> {
    vec![
        (rox_i18n::t!("signal-kind-band"), SourceKind::Band),
        (rox_i18n::t!("signal-kind-level"), SourceKind::Level),
        (rox_i18n::t!("signal-kind-onset"), SourceKind::Onset),
        (rox_i18n::t!("signal-kind-trigger"), SourceKind::Trigger),
        (rox_i18n::t!("signal-kind-total"), SourceKind::Aggregate),
    ]
}

/// A trigger with no threshold never fires, so switching to one seeds this.
const TRIGGER_SEED: f32 = 0.5;

const AGGREGATE_RATE: f32 = 1.0;

/// Write the shared pool to settings once the edit burst settles. The hub
/// takes edits live; only the file write waits, since a settings write
/// reserializes every shard and doing it per scrub tick stutters the app.
/// The generation is global because the pool is.
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

fn edit_signal(hub: &Arc<SignalHub>, id: u64, edit: impl FnOnce(&mut Signal), cx: &mut gpui::App) {
    hub.edit(|pool| {
        if let Some(signal) = pool.iter_mut().find(|s| s.id == id) {
            edit(signal);
        }
    });
    persist_pool_soon(hub, cx);
}

/// A thin live meter, read off the hub at paint time. The host owns the
/// frame cadence and re-renders on the pump's notify. Never request frames
/// here: it would spin every host window at monitor refresh for values
/// that only change at the pump's clock.
///
/// The bar is the value before the gate, dimmed while the gate eats it,
/// since placing the threshold is what this meter is for.
pub fn meter(hub: Arc<SignalHub>, id: u64, fill: Rgba, marker: Option<f32>) -> Div {
    div().h(px(6.)).w_full().child(
        canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let value = hub.raw_value(id).unwrap_or(0.0).clamp(0.0, 1.0);
                // How far the gate is open: gated value over raw.
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

/// The segmented control's look, built by hand because the options follow
/// the live pool. Open to any view, not just a [`SignalHost`].
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

/// Repoints an existing route rather than stacking a second.
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

/// Seeded with the given name, not the derived label, so clearing the
/// field returns the name to following the source.
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
            .placeholder(rox_i18n::t!("signal-name-placeholder"))
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

/// Routes bound to it stay and go quiet, so repointing restores them.
fn remove_signal<P: SignalHost>(host: &mut P, id: u64, cx: &mut Context<P>) {
    host.hub().edit(|pool| pool.retain(|s| s.id != id));
    persist_pool_soon(host.hub(), cx);
    cx.notify();
}

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
                .child(rox_i18n::t!("signals-empty")),
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

fn signal_block<P: SignalHost>(host: &P, id: u64, cx: &mut Context<P>) -> Div {
    let pool = host.hub().pool();
    let Some(signal) = pool.iter().find(|s| s.id == id) else {
        return div();
    };
    let riders = host.routes().iter().filter(|r| r.signal == id).count();
    let open = host.signal_ui().open.contains(&id);
    let renaming = matches!(&host.signal_ui().rename, Some((rid, _, _)) if *rid == id);
    // A one-frame window handler cancels the rename on any press outside
    // the field.
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
    // Only the chevron and name take the fold's click, and not during a
    // rename, or clicks on their way to the pencil or field fold the block.
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
            // Hidden at zero: the signals window owns no routes, and a count
            // there would read as the pool going nowhere.
            .when(riders > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(rox_i18n::t!(
                            "signal-routes-in-panel",
                            count = riders as u64
                        )),
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
        .child(meter(
            host.hub().clone(),
            id,
            palette::accent(),
            gate_mark(signal),
        ))
        .when(open, |d| d.child(signal_tuning(host, id, cx)))
}

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
        Source::Trigger { lo, hi } => (SourceKind::Trigger, lo, hi),
        Source::Level => (SourceKind::Level, 30.0, 120.0),
        Source::Aggregate { .. } => (SourceKind::Aggregate, 30.0, 120.0),
    };
    let smooth = signal.smooth.clamp(0.0, 1.0);
    let threshold = signal.threshold();
    // A total's band, response and gate belong to the signal it follows.
    let spectral = kind != SourceKind::Aggregate;
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("signal-source"),
            Some(rox_i18n::t!("signal-source.description")),
            panel::choices_shared(
                &source_choices(),
                kind,
                move |this: &mut P, kind, cx| {
                    // Switching kinds keeps the band. A fresh Total follows
                    // another pool signal, or it would sit at zero.
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
                                Source::Band { lo, hi }
                                | Source::Onset { lo, hi }
                                | Source::Trigger { lo, hi } => (lo, hi),
                                Source::Level | Source::Aggregate { .. } => (30.0, 120.0),
                            };
                            let (of, rate) = match signal.source {
                                Source::Aggregate { of, rate } => (of, rate),
                                _ => (first_other, AGGREGATE_RATE),
                            };
                            signal.source = match kind {
                                SourceKind::Band => Source::Band { lo, hi },
                                SourceKind::Onset => Source::Onset { lo, hi },
                                SourceKind::Trigger => Source::Trigger { lo, hi },
                                SourceKind::Level => Source::Level,
                                SourceKind::Aggregate => Source::Aggregate { of, rate },
                            };
                            if kind == SourceKind::Trigger && signal.threshold <= 0.0 {
                                signal.threshold = TRIGGER_SEED;
                            }
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
                rox_i18n::t!("signal-low-bound"),
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
                                if let Source::Band { lo, hi }
                                | Source::Onset { lo, hi }
                                | Source::Trigger { lo, hi } = &mut signal.source
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
                rox_i18n::t!("signal-high-bound"),
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
                                if let Source::Band { lo, hi }
                                | Source::Onset { lo, hi }
                                | Source::Trigger { lo, hi } = &mut signal.source
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
                rox_i18n::t!("signal-response"),
                Some(if matches!(kind, SourceKind::Onset | SourceKind::Trigger) {
                    rox_i18n::t!("signal-response-pulse")
                } else {
                    rox_i18n::t!("signal-response-drift")
                }),
                panel::value_slider_edit(
                    &scrubs.smooth,
                    host.value_edit(),
                    smooth,
                    rox_i18n::format::format_percent((smooth * 100.0).round() as f64),
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
                rox_i18n::t!("signal-threshold"),
                Some(if kind == SourceKind::Trigger {
                    rox_i18n::t!("signal-threshold-trigger")
                } else {
                    rox_i18n::t!("signal-threshold-gate")
                }),
                panel::value_slider_edit(
                    &scrubs.threshold,
                    host.value_edit(),
                    threshold,
                    rox_i18n::format::format_percent((threshold * 100.0).round() as f64),
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

/// A total's own rows. No gate or response: those belong to the signal
/// it follows.
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

    // A dropdown, since chips for an unbounded pool take over the block.
    // Aggregates are offered too: a total over a total reads last frame's
    // value, so a ring just sits still.
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
                rox_i18n::t!("signal-aggregate-nothing").to_string()
            } else {
                rox_i18n::t!("signal-aggregate-pick").to_string()
            }
        });
    let button = Button::new(SharedString::from(format!("aggregate-of-{id}")))
        .label(label)
        .small()
        .outline()
        .dropdown_caret(true);
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
        rox_i18n::t!("signal-adds-up"),
        Some(rox_i18n::t!("signal-adds-up.description")),
        picker,
    ))
    .when(alone, |d| {
        d.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("signal-aggregate-alone")),
        )
    })
    .when(!alone && !known, |d| {
        d.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("signal-aggregate-unpicked")),
        )
    })
    .child(setting_row(
        rox_i18n::t!("signal-rate"),
        Some(rox_i18n::t!("signal-rate.description")),
        panel::value_slider_edit_over(
            &scrubs.rate,
            host.value_edit(),
            rate / AGGREGATE_RATE_MAX,
            format!("{}/s", rox_i18n::format::format_float(rate as f64, 2)),
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
        rox_i18n::t!("signal-reset-on-track"),
        Some(rox_i18n::t!("signal-reset-on-track.description")),
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
        rox_i18n::t!("signal-flush"),
        Some(rox_i18n::t!("signal-flush.description")),
        settings_ui::small_button(
            rox_i18n::t!("signal-flush"),
            icons::REFRESH_CW,
            false,
            cx.listener(move |this: &mut P, _, _, cx| {
                this.hub().flush(id);
                cx.notify();
            }),
        ),
    ))
}

fn gate_mark(signal: &Signal) -> Option<f32> {
    let threshold = signal.threshold();
    (threshold > 0.0).then_some(threshold)
}

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
        rox_i18n::t!("route-new-signal").to_string(),
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
            rox_i18n::t!("route-signal"),
            Some(rox_i18n::t!("route-signal.description")),
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
                    .child(rox_i18n::t!("route-shared-note")),
            )
            .child(signal_tuning(host, route.signal, cx));
    } else {
        col = col.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("route-signal-gone")),
        );
    }
    // The span is per route; everything above is the shared signal.
    col.child(
        div()
            .pt(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_faint())
            .child(rox_i18n::t!("route-range-note")),
    )
    .child(setting_row(
        rox_i18n::t!("route-quiet"),
        Some(rox_i18n::t!("route-quiet.description")),
        panel::value_slider_edit_over(
            &scrubs.from,
            host.value_edit(),
            from,
            rox_i18n::format::format_percent((from * 100.0).round() as f64),
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
        rox_i18n::t!("route-loud"),
        Some(rox_i18n::t!("route-loud.description")),
        panel::value_slider_edit_over(
            &scrubs.to,
            host.value_edit(),
            to,
            rox_i18n::format::format_percent((to * 100.0).round() as f64),
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

/// A settings row whose knob a route can drive. Right-click binds; the
/// slider keeps working while bound, since the route's span is a share of
/// its setting.
pub fn bindable_row<P: RouteHost>(
    host: &P,
    label: impl Into<SharedString>,
    description: Option<SharedString>,
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
        // Shows even over an empty pool, so the way in is never invisible.
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
                PopupMenuItem::new(rox_i18n::t!("signal-add"))
                    .icon(Icon::default().path(icons::PLUS))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = panel.upgrade() {
                            this.update(cx, |this, cx| attach_new_signal(this, target.clone(), cx));
                        }
                    }),
            )
        })
        .child(control)
        // An unbound row keeps an empty slot the same size so the sliders
        // stay in column.
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
    // `context_menu` keys its state on the element id path and names every
    // one the same, so without a unique id per row, rows share one menu
    // entity and swallow its clicks.
    let control = div()
        .id(SharedString::from(format!("bind-row-{target}")))
        .child(control);
    let mut row = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(panel::setting_row(label, description, control));
    if open && let Some(index) = bound {
        let header = settings_ui::block_header(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("route-header")),
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
    row
}
