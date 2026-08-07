//! The signals window: the shared pool of audio signals the panels bind
//! their knobs to, in one window of its own.
//!
//! It sits at the top level because the pool does. Every panel's routes ride
//! the same signals, and an edit here lands on all of them, so tending it
//! from inside one particles panel's settings made an app-wide thing look
//! like that panel's own. The routes stay where they belong: under the knobs
//! they drive, in the panel's settings, through [`signal_ui::bindable_row`].
//!
//! Live meters need the hub ticked, and until now the only things ticking it
//! were a particles panel painting and the screen shader. This window ticks
//! it too while it's open, so the readouts move against the music with
//! nothing else on screen.
//!
//! It carries a spectrum and a transport for the same reason the equalizer
//! does: a band is picked by eye against what's playing, and going back to
//! the workspace window for every pause breaks the loop you tune in.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, size, svg, App, Bounds, Context, Div, Entity, Global, MouseButton,
    ScrollHandle, Subscription, Window, WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::Root;

use rox_viz::signal::SignalHub;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, ValueEdit};
use crate::panels::spectrum::{self, Labels, SpectrumConfig, SpectrumPanel};
use crate::settings::{Settings, SignalsWindowState};
use crate::signal_ui::{self, SignalHost, SignalUi};

/// Wide enough for the spectrum to be worth reading a band off, since the
/// tuning rows sit under it and a bound is picked against what's on screen.
const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(520.),
    height: px(420.),
};

/// How tall the spectrum stands. Context for the sliders rather than the
/// subject, so it takes a strip off the top instead of a share of the
/// window that would grow with it.
const SPECTRUM_H: f32 = 132.;

/// The open signals window, if any: opening again focuses it rather than
/// stacking a second one, the stats, console and EQ move.
struct OpenSignals(WindowHandle<Root>);

impl Global for OpenSignals {}

/// Open the signals window, or bring the open one to the front.
///
/// Deferred like the EQ and the console: the menu action that opens it runs
/// inside the workspace's own update, and reading the front workspace for
/// the hub mid-update would panic.
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
    // The hub comes from whichever workspace is in front when this opens,
    // the same place the tint does. With no workspace up there's no hub to
    // borrow, so it builds one over the saved pool: the signals can still be
    // edited and persisted, they just have no audio to read.
    let state = crate::workspace::front_workspace(cx).map(|(_, state)| state);
    let saved = Settings::load().windows.signals;
    let (width, height) = saved
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        // The spectrum across the top, then a column of signal blocks, each
        // a meter over four tuning rows.
        .unwrap_or((720., 700.));
    // Open on a first run, folded away for anyone who has folded it once.
    let about = saved.map(|s| s.about).unwrap_or(true);
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle =
        panel::open_child_window(cx, "rox - Signals", bounds, Some(MIN), move |window, cx| {
            cx.new(|cx| SignalsWindow::new(state, about, window, cx))
        });
    cx.set_global(OpenSignals(handle));
}

struct SignalsWindow {
    /// The workspace that was in front when this opened, for the feed the
    /// hub ticks off and the art tint. None when there was no workspace up.
    state: Option<AppState>,
    /// The hub being edited: the front workspace's, or a standalone one over
    /// the saved pool when this opened with no workspace.
    hub: Arc<SignalHub>,
    /// The pool editor's widget state, kept in step with the pool by
    /// [`signal_ui::sync`] on every render.
    signal_ui: SignalUi,
    /// The one typed-readout slot, so only one tuning row is ever being
    /// typed into.
    value_edit: ValueEdit,
    /// The spectrum across the top, the real panel rather than a copy of
    /// its drawing: a band picked here is picked against the same analysis
    /// a spectrum panel would show. None when this opened with no
    /// workspace, which leaves it out rather than drawing a dead one.
    spectrum: Option<Entity<SpectrumPanel>>,
    /// The config that spectrum draws with, kept so the band marks laid
    /// over it map through the very same range the bars do.
    spectrum_config: SpectrumConfig,
    /// Whether the explainer at the top of the page is unfolded. Persisted,
    /// since someone who has read it once shouldn't have to fold it away on
    /// every open.
    about: bool,
    scroll: ScrollHandle,
    /// Wakes the window when playback moves, which is what starts the meters
    /// again after a pause: the frame loop below only sustains itself while
    /// something is playing.
    _player_changed: Option<Subscription>,
}

impl SignalsWindow {
    fn new(
        state: Option<AppState>,
        about: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The OS close button never runs remove_window, so the frame
        // persists through the should-close hook, the way the other child
        // windows do it. The pool itself writes as it is edited, and so
        // does the fold, which is why this edits the entry in place rather
        // than replacing it.
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
            .unwrap_or_else(|| Arc::new(SignalHub::new(Settings::load().look.bundle.signals)));
        // The frequency scale is on where a docked spectrum ships without
        // it: every bound on the page below is in Hz, and a strip with no
        // numbers is a picture rather than a reference. Freeze is on for
        // the same reason: a band gets picked against the moment that
        // showed it, and pausing there is how that moment gets held still
        // long enough to drag a bound onto it.
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

    /// Fold the explainer away, or bring it back, and remember which.
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

    /// The explainer under its own fold: the header is the whole strip, so
    /// the copy that teaches the page can be put away once it has.
    fn about_section(&self, cx: &mut Context<Self>) -> Div {
        let open = self.about;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
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
                    .child("About Signals"),
            )
            .when(open, |d| d.child(blurb()))
    }

    /// The four playback verbs under the spectrum, centered: a signal is
    /// tuned against what's playing, so starting and skipping belongs in
    /// this window rather than back in the workspace one.
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

    /// Advance the hub off the player's feed and say whether the meters
    /// should keep asking for frames. The hub throttles itself, so ticking
    /// from here costs nothing extra when a particles panel is already
    /// doing it. While audio moves the player observe re-renders on every
    /// pump tick, the only rate new values arrive at, so frame polling is
    /// just for the drain after playback stops: a signal decaying to
    /// nothing is exactly the part worth watching, and it outlives
    /// [`SignalHub::live`]. Once every signal settles the window parks,
    /// and a resume wakes it through the pump's play-state notify.
    fn step(&self, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let player = state.player.read(cx);
        self.hub.tick(&player.feed(), player.playing_entry());
        if player.is_playing() {
            return false;
        }
        self.hub
            .pool()
            .iter()
            .any(|signal| self.hub.raw_value(signal.id).unwrap_or(0.0) > 0.001)
    }
}

/// The pool editor reads this window through the trait. It owns no routes,
/// so [`SignalHost::routes`] keeps its default: the routes riding these
/// signals belong to the panels, which edit them under their own knobs.
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
        // With no workspace player to theme to, tint to this window's own id,
        // which the palette map doesn't know, so it reads the base palette.
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
        // The whole tree builds inside the closure: an element made outside
        // it reads the palette before the tint is in place and paints
        // untinted.
        panel::window_body(player, || {
            let page = signal_ui::signals_page(self, cx);
            let about = self.about_section(cx);
            let transport = self.transport(cx);
            // The bands of whatever is unfolded below, drawn over the same
            // strip they were picked against. The dragged one brightens, so
            // a bound being moved is the one the eye follows.
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
                // The spectrum leads: every bound on the page below is a
                // frequency, and this is where one gets picked.
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
                // Straight under the spectrum: the two belong together as
                // what's playing, and the pool below is the work.
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
                                .child(page)
                                // Nothing to read the music off, so the
                                // meters would sit at nothing without
                                // saying why.
                                .when(orphaned, |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .text_color(palette::text_muted())
                                            .child(
                                                "No library window is open, so these show no \
                                                 audio. Edits still save.",
                                            ),
                                    )
                                }),
                        )
                        // Fades out when idle, same as the panels.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                .into_any_element()
        })
    }
}

/// What a signal is and how one gets used, since neither is guessable from
/// a list of bands: the page under this is all bounds and percentages, and
/// the binding it serves happens in another window entirely.
fn blurb() -> Div {
    let line = |text: &'static str| {
        div()
            .flex_none()
            .text_xs()
            .text_color(palette::text_muted())
            .child(text)
    };
    // The glyph shown rather than named: the reader has to recognize the
    // mark in a menu, and the way to teach that is to show it. It leads the
    // line instead of sitting mid-sentence, because a flex row wraps by
    // child: a sentence split around the icon breaks onto its own line and
    // then runs off the edge, having no width of its own to wrap inside.
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
        .child(div().flex_1().min_w_0().child(
            "Panels marked with this in the menus can have most of their parameters \
             bound: right-click a parameter in the panel's settings and pick a signal, \
             or add one from there.",
        ));
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(tokens::SPACE_XS)
        .child(line(
            "A signal turns what's playing into one number between 0 and 1: the \
             energy in a frequency band, the level of the whole mix, or a pulse on \
             every hit inside a band. Response sets how fast it follows, Threshold \
             silences it under a level you pick.",
        ))
        .child(line(
            "A Total is the fourth kind: it adds another signal up over time and \
             wraps at 1, so it climbs while the music is loud and stalls while it \
             isn't. That's the one to reach for when a shader wants a phase that \
             moves with the song rather than with the clock.",
        ))
        .child(marked)
        .child(line(
            "What's tuned here is shared, so a change lands on every parameter \
             routed to that signal, in every panel and window.",
        ))
}
