//! The transport panels - playback controls, a volume strip, and a
//! click-to-seek strip - the app's whole playback UI, living in the bottom
//! dock by default. Each is a view over the shared player entity, exactly
//! like the audio views: duplicates are fresh views, pop-outs rehost the
//! entity.

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, rgba, size, App, Bounds, Context, EventEmitter,
    FocusHandle, Focusable, MouseButton, Pixels, Subscription, WeakEntity, Window,
};
use gpui_component::button::Button;
use rox_dock::{Panel, PanelEvent, TabPanel};

use rox_playback::engine::LoopMode;

use crate::panel::{self, AppState, StatePanel};

/// Thickness of the seek strip's track line.
const STRIP_H: f32 = 6.0;

/// The playback controls: prev, play/pause, next, the seek nudges, and the
/// loop mode, with the status line (queue position, track name, clock)
/// under them. The pump's tick notifies the player while a session runs,
/// so the observe below keeps the play state fresh even in a popped-out
/// window.
pub struct TransportPanel {
    state: AppState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl TransportPanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        TransportPanel {
            state,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }
}

impl Render for TransportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        let playing = player.is_playing();
        let loop_label = match player.loop_mode() {
            LoopMode::Off => "loop: off",
            LoopMode::All => "loop: all",
            LoopMode::One => "loop: one",
        };
        // The status line, or the reason there is none: a session-start
        // error, else the idle message.
        let status = player
            .status_line()
            .or_else(|| player.error())
            .unwrap_or_else(|| "nothing playing".into());

        let controls = div()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .child(panel::control(
                "prev",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.prev()),
                cx,
            ))
            .child(panel::control(
                if playing { "pause" } else { "play" },
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.toggle_pause()),
                cx,
            ))
            .child(panel::control(
                "next",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.next()),
                cx,
            ))
            .child(panel::control(
                "-10s",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                cx,
            ))
            .child(panel::control(
                "+10s",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                cx,
            ))
            .child(panel::control(
                loop_label,
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                cx,
            ));

        div()
            .size_full()
            .bg(rgb(0x121212))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_3()
            .child(controls)
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_color(rgb(0x808080))
                    .child(status),
            )
    }
}

/// The volume strip: the level meter with the volume steppers around the
/// readout, the bar's right side as its own panel.
pub struct VolumePanel {
    state: AppState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl VolumePanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        VolumePanel {
            state,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        let volume = (player.volume() * 100.0).round() as u32;
        let meter = player.meter().min(1.0);

        div()
            .size_full()
            .bg(rgb(0x121212))
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .child(panel::meter(meter))
            .child(panel::control(
                "vol -",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.nudge_volume(-0.1)),
                cx,
            ))
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .text_center()
                    .child(format!("{volume}%")),
            )
            .child(panel::control(
                "vol +",
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.nudge_volume(0.1)),
                cx,
            ))
    }
}

/// The seek strip: the waveform minus the peaks - a track line with the
/// played side in the accent and a playhead, click to seek. Position and
/// seek come off the player the same way the waveform's do.
pub struct SeekStripPanel {
    state: AppState,
    /// The strip's bounds as of the last paint, for click-to-seek mapping.
    strip: Arc<Mutex<Option<Bounds<Pixels>>>>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl SeekStripPanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SeekStripPanel {
            state,
            strip: Arc::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    fn seek_to_fraction(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = *self.strip.lock().unwrap() else {
            return;
        };
        let player = self.state.player.read(cx);
        let Some(now) = player.now_playing() else {
            return;
        };
        let Some(duration) = now.duration_secs else {
            return;
        };
        let w = f32::from(bounds.size.width);
        if w <= 0.0 {
            return;
        }
        let fraction = (f32::from(x - bounds.origin.x) / w).clamp(0.0, 1.0);
        player.seek_to(fraction as f64 * duration);
    }
}

/// The track line centered in whatever height the panel gets: unplayed side
/// dim, played side solid, the waveform's playhead on top.
fn paint_strip(progress: f32, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let head_x = progress.clamp(0.0, 1.0) * w;
    let line_y = (h - STRIP_H) / 2.0;
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(w), px(STRIP_H)),
        ),
        rgba(0x3dff9c33),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(head_x), px(STRIP_H)),
        ),
        rgba(0x3dff9cff),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x + px(head_x - 1.0), bounds.origin.y),
            size(px(2.0), px(h)),
        ),
        rgba(0xe0e0e0d9),
    ));
}

impl Render for SeekStripPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.state.player.read(cx).now_playing();

        // The position clock only moves while a session runs; poll by frame
        // like the waveform does. No session: fully parked.
        if now.is_some() {
            window.request_animation_frame();
        }

        let body = match &now {
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x808080))
                .child("nothing playing")
                .into_any_element(),
            Some(now) => {
                let progress = now
                    .duration_secs
                    .filter(|d| *d > 0.0)
                    .map(|d| (now.position_secs / d) as f32)
                    .unwrap_or(0.0);
                let strip = self.strip.clone();
                canvas(
                    move |bounds, _, _| {
                        // Remember where the strip landed so a click maps
                        // back to a position in the track.
                        *strip.lock().unwrap() = Some(bounds);
                    },
                    move |bounds, _, window, _| {
                        paint_strip(progress, bounds, window);
                    },
                )
                .size_full()
                .into_any_element()
            }
        };

        div()
            .size_full()
            .bg(rgb(0x121212))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.seek_to_fraction(event.position.x, cx);
                }),
            )
            .child(body)
    }
}

/// The Panel, StatePanel, and focus plumbing is identical across the three
/// transport panels; only the name differs.
macro_rules! transport_panel {
    ($panel:ty, $name:literal) => {
        impl StatePanel for $panel {
            fn state(&self) -> AppState {
                self.state.clone()
            }

            fn tab_panel(&self) -> Option<WeakEntity<TabPanel>> {
                self.tab_panel.clone()
            }

            fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self {
                Self::new(state, cx)
            }
        }

        impl EventEmitter<PanelEvent> for $panel {}

        impl Focusable for $panel {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Panel for $panel {
            fn panel_name(&self) -> &'static str {
                $name
            }

            fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                panel::tab_title($name, &cx.entity(), self.tab_panel.clone())
            }

            fn inner_padding(&self, _cx: &App) -> bool {
                false
            }

            fn on_added_to(
                &mut self,
                tab_panel: WeakEntity<TabPanel>,
                _window: &mut Window,
                cx: &mut Context<Self>,
            ) {
                self.tab_panel = Some(tab_panel.clone());
                self.state
                    .tab_hosts
                    .update(cx, |hosts, _| hosts.report(tab_panel));
            }

            fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
                self.tab_panel = None;
            }

            fn toolbar_buttons(
                &mut self,
                _window: &mut Window,
                cx: &mut Context<Self>,
            ) -> Option<Vec<Button>> {
                Some(vec![
                    panel::duplicate_button(&cx.entity()),
                    panel::popout_button(&cx.entity(), $name, self.tab_panel.clone()),
                ])
            }
        }
    };
}

transport_panel!(TransportPanel, "playback");
transport_panel!(VolumePanel, "volume");
transport_panel!(SeekStripPanel, "seek");
