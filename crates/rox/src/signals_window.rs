//! The signals window: the shared pool of audio signals the panels bind their
//! knobs to. A top-level window because the pool is app-wide; the routes stay
//! in each panel's settings, through [`signal_ui::bindable_row`].
//!
//! It carries a spectrum and a transport so a band is picked by eye against
//! what's playing.

use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Div, Entity, Global, MouseButton, ScrollHandle, SharedString,
    Subscription, Window, WindowHandle, div, prelude::*, px, size, svg,
};
use gpui_component::Root;
use gpui_component::scroll::Scrollbar;

use rox_viz::signal::SignalHub;

use rox_core::settings::{Settings, SignalsWindowState};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::signal_ui::{self, SignalHost, SignalUi};
use rox_panel_kit::ValueEdit;
use rox_panels::spectrum::{self, Labels, SpectrumConfig, SpectrumPanel};

const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(520.),
    height: px(420.),
};

/// A fixed strip: context for the sliders, not the subject.
const SPECTRUM_H: f32 = 132.;

struct OpenSignals(WindowHandle<Root>);

impl Global for OpenSignals {}

/// Deferred: the menu action runs inside the workspace's own update, and
/// reading the front workspace mid-update panics.
pub fn open(cx: &mut App) {
    cx.defer(open_now);
}

fn open_now(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenSignals>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // With no workspace up, build a hub over the saved pool: the signals still
    // edit and persist, with no audio to read.
    let state = rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state);
    let saved = Settings::load().windows.signals;
    let (width, height) = saved
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((720., 700.));
    let about = saved.map(|s| s.about).unwrap_or(true);
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = panel::open_child_window(
        cx,
        rox_i18n::t!("signals-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| cx.new(|cx| SignalsWindow::new(state, about, window, cx)),
    );
    cx.set_global(OpenSignals(handle));
}

struct SignalsWindow {
    state: Option<AppState>,
    hub: Arc<SignalHub>,
    signal_ui: SignalUi,
    value_edit: ValueEdit,
    /// The real panel, so a band is picked against the same analysis a spectrum
    /// panel shows.
    spectrum: Option<Entity<SpectrumPanel>>,
    spectrum_config: SpectrumConfig,
    /// Persisted, so a reader who folded it once doesn't fold it every open.
    about: bool,
    scroll: ScrollHandle,
    /// Restarts the meters after a pause: the frame loop only sustains itself
    /// while playing.
    _player_changed: Option<Subscription>,
}

impl SignalsWindow {
    fn new(
        state: Option<AppState>,
        about: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The OS close button never runs remove_window. Edits the entry in
        // place, since the pool and the fold write as they change.
        window.on_window_should_close(cx, |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let saved = s
                    .windows
                    .signals
                    .get_or_insert_with(SignalsWindowState::default);
                saved.width = frame.size.width.into();
                saved.height = frame.size.height.into();
            });
            true
        });
        let _player_changed = state
            .as_ref()
            .map(|state| cx.observe(&state.player, |_, _, cx| cx.notify()));
        let hub = state
            .as_ref()
            .map(|state| state.signals.clone())
            .unwrap_or_else(|| Arc::new(SignalHub::unfed(Settings::load().look.bundle.signals)));
        // Frequency labels on, since every bound below is in Hz; freeze on, so
        // a band can be held still long enough to drag a bound onto it.
        let spectrum_config = SpectrumConfig {
            labels: Labels::Freq,
            freeze: true,
            ..SpectrumConfig::default()
        };
        let spectrum = state.as_ref().map(|state| {
            let config = spectrum_config.clone();
            cx.new(|cx| SpectrumPanel::new(state.clone(), config, cx))
        });
        SignalsWindow {
            state,
            hub,
            signal_ui: SignalUi::default(),
            value_edit: ValueEdit::default(),
            spectrum,
            spectrum_config,
            about,
            scroll: ScrollHandle::new(),
            _player_changed,
        }
    }

    fn toggle_about(&mut self, cx: &mut Context<Self>) {
        self.about = !self.about;
        let about = self.about;
        Settings::update(move |s| {
            s.windows
                .signals
                .get_or_insert_with(SignalsWindowState::default)
                .about = about;
        });
        cx.notify();
    }

    fn about_section(&self, cx: &mut Context<Self>) -> Div {
        let open = self.about;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_SM)
            .child(
                // Hand-built: [`rox_panel_kit::ui::section`] has no click hook,
                // and one on its result would fold on any body click.
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .text_xs()
                    .text_color(palette::text_muted())
                    .cursor_pointer()
                    .hover(|d| d.text_color(palette::text()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.toggle_about(cx)),
                    )
                    .child(
                        div()
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
                                    .flex_none(),
                            )
                            .child(rox_i18n::t!("signals-about-toggle")),
                    ),
            )
            .when(open, |d| d.child(blurb()))
    }

    fn transport(&self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.state.as_ref()?;
        let strip = panel::transport_strip(&state.player.clone(), &state.library.clone(), cx);
        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .justify_center()
                .child(strip),
        )
    }

    /// Reading the values advances the hub, which throttles itself. While audio
    /// plays the player observe re-renders per pump tick, so frames are only
    /// requested for the decay after playback stops, which outlives
    /// [`SignalHub::live`].
    fn step(&self, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let player = state.player.read(cx);
        if player.is_playing() {
            return false;
        }
        self.hub
            .pool()
            .iter()
            .any(|signal| self.hub.raw_value(signal.id).unwrap_or(0.0) > 0.001)
    }
}

/// Owns no routes, so [`SignalHost::routes`] keeps its default.
impl SignalHost for SignalsWindow {
    fn hub(&self) -> &Arc<SignalHub> {
        &self.hub
    }

    fn signal_ui(&self) -> &SignalUi {
        &self.signal_ui
    }

    fn signal_ui_mut(&mut self) -> &mut SignalUi {
        &mut self.signal_ui
    }

    fn value_edit(&self) -> &ValueEdit {
        &self.value_edit
    }
}

impl Render for SignalsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no workspace player, tint to this window's own id, which reads
        // the base palette.
        let player = self
            .state
            .as_ref()
            .map(|state| state.player.entity_id())
            .unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        if self.step(cx) {
            window.request_animation_frame();
        }
        signal_ui::sync(self);
        let orphaned = self.state.is_none();
        // Everything builds inside the closure, or it reads the palette before
        // the tint is in place.
        panel::window_body(player, || {
            let page = signal_ui::signals_page(self, cx);
            let about = self.about_section(cx);
            let transport = self.transport(cx);
            let config = &self.spectrum_config;
            let bands: Vec<Div> = signal_ui::open_bands(self)
                .into_iter()
                .map(|band| {
                    spectrum::band_overlay(
                        config,
                        band.lo,
                        band.hi,
                        Some(band.label),
                        band.dragging,
                    )
                })
                .collect();
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .children(self.spectrum.clone().map(|spectrum| {
                    div()
                        .flex_none()
                        .relative()
                        .h(px(SPECTRUM_H))
                        .m(tokens::SPACE_MD)
                        .mb_0()
                        .rounded(tokens::RADIUS)
                        .overflow_hidden()
                        .child(spectrum)
                        .children(bands)
                }))
                .when_some(transport, |d, transport| {
                    d.child(
                        div()
                            .flex_none()
                            .px(tokens::SPACE_MD)
                            .pt(tokens::SPACE_MD)
                            .child(transport),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(
                            div()
                                .id("signals")
                                .size_full()
                                .flex()
                                .flex_col()
                                .gap(tokens::SPACE_MD)
                                .p(tokens::SPACE_MD)
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .child(about)
                                .child(page),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                // Pinned under the pool, since a scrolled page is where the
                // dead meters are.
                .when(orphaned, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .px(tokens::SPACE_MD)
                            .py(tokens::SPACE_SM)
                            .border_t_1()
                            .border_color(palette::border())
                            .text_xs()
                            .text_color(palette::tone_warn())
                            .child(rox_i18n::t!("signals-no-library")),
                    )
                })
                .into_any_element()
        })
    }
}

fn blurb() -> Div {
    let line = |text: SharedString| {
        div()
            .flex_none()
            .text_xs()
            .text_color(palette::text_muted())
            .child(text)
    };
    // The glyph leads the line: a flex row wraps by child, so an icon
    // mid-sentence would break the text onto lines of its own.
    let marked = div()
        .flex()
        .flex_row()
        .items_start()
        .gap(tokens::SPACE_XS)
        .text_xs()
        .text_color(palette::text_muted())
        .child(
            svg()
                .path(icons::AUDIO_WAVEFORM)
                .size_3()
                .flex_none()
                .mt(px(2.))
                .text_color(palette::text()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(rox_i18n::t!("signals-blurb-marked")),
        );
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(tokens::SPACE_XS)
        .child(line(rox_i18n::t!("signals-blurb-what")))
        .child(line(rox_i18n::t!("signals-blurb-total")))
        .child(marked)
        .child(line(rox_i18n::t!("signals-blurb-shared")))
}
