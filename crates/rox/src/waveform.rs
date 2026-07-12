//! The waveform panel: the whole track's amplitude shape as mirrored bars
//! around a center line, played bars in the accent, the rest as a dim ghost,
//! with a playhead tracking the position clock. Clicking the strip seeks.
//! Peaks come from a full decode of the current track on a background
//! thread when the track changes - a few thousand min/max pairs held in
//! memory, no cache on disk. Painting is a row of quads; while nothing
//! plays the panel sits completely still.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, rgba, size, App, Bounds, Context, EventEmitter,
    FocusHandle, Focusable, MouseButton, Pixels, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, TabPanel};

use rox_playback::engine;

use crate::panel::{self, AppState, StatePanel};

/// Resolution of the in-memory peaks. The paint resamples these down to
/// however many bars fit the width.
const PEAK_BINS: usize = 2048;

/// Display bar geometry, a few px wide with a hairline gap.
const BAR_WIDTH: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
const MIN_BAR: f32 = 2.0;

enum Peaks {
    /// No track has been seen yet.
    None,
    Decoding,
    Ready(Arc<Vec<(f32, f32)>>),
    Failed,
}

pub struct WaveformPanel {
    state: AppState,
    /// The track the peaks (or the running decode) belong to.
    track: Option<PathBuf>,
    peaks: Peaks,
    /// Discards stale decode results when the track changes mid-decode.
    generation: u64,
    /// The strip's bounds as of the last paint, for click-to-seek mapping.
    strip: Arc<Mutex<Option<Bounds<Pixels>>>>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel when a session starts, so an idle window notices the
    /// new track without the player bar's frame pump.
    _player_changed: Subscription,
}

impl WaveformPanel {
    pub fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        WaveformPanel {
            state,
            track: None,
            peaks: Peaks::None,
            generation: 0,
            strip: Arc::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The playing track changed: decode its peaks off the UI thread and
    /// swap them in when done.
    fn start_decode(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.track = Some(path.clone());
        self.peaks = Peaks::Decoding;
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine::decode_peaks(&path, PEAK_BINS) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.peaks = match result {
                    Ok(peaks) => Peaks::Ready(Arc::new(peaks)),
                    Err(e) => {
                        eprintln!("waveform decode failed: {e}");
                        Peaks::Failed
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    fn strip(&self, progress: f32, peaks: Arc<Vec<(f32, f32)>>) -> impl IntoElement {
        let strip = self.strip.clone();
        canvas(
            move |bounds, _, _| {
                // Remember where the strip landed so a click maps back to a
                // position in the track.
                *strip.lock().unwrap() = Some(bounds);
            },
            move |bounds, _, window, _| {
                paint_peaks(&peaks, progress, bounds, window);
            },
        )
        .size_full()
    }

    fn message(&self, text: &'static str) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x808080))
            .child(text)
    }
}

/// Mirrored bars around the center line, resampled to the width: played bars
/// solid accent, the rest a dim ghost, and a playhead on top.
fn paint_peaks(
    peaks: &[(f32, f32)],
    progress: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 || peaks.is_empty() {
        return;
    }

    let count = ((w / (BAR_WIDTH + BAR_GAP)) as usize).max(1);
    let step = w / count as f32;
    let center = h / 2.0;
    let max_bar = h * 0.46;
    let head_x = progress.clamp(0.0, 1.0) * w;

    let per = peaks.len() as f32 / count as f32;
    for i in 0..count {
        // Each display bar takes its bucket's extremes so transients survive
        // the downsample.
        let from = (i as f32 * per) as usize;
        let to = (((i + 1) as f32 * per) as usize).clamp(from + 1, peaks.len());
        let (lo, hi) = peaks[from..to]
            .iter()
            .fold((0.0f32, 0.0f32), |(lo, hi), &(bl, bh)| {
                (lo.min(bl), hi.max(bh))
            });

        let x = i as f32 * step;
        let top = center - (hi * max_bar).max(MIN_BAR / 2.0);
        let bottom = center - (lo * max_bar).min(-MIN_BAR / 2.0);
        let played = x + step * 0.5 <= head_x;
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(top)),
                size(px(BAR_WIDTH), px(bottom - top)),
            ),
            if played {
                rgba(0x3dff9cff)
            } else {
                rgba(0x3dff9c33)
            },
        ));
    }

    // The playhead line.
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x + px(head_x - 1.0), bounds.origin.y),
            size(px(2.0), px(h)),
        ),
        rgba(0xe0e0e0d9),
    ));
}

impl StatePanel for WaveformPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn tab_panel(&self) -> Option<WeakEntity<TabPanel>> {
        self.tab_panel.clone()
    }

    fn duplicate(state: AppState, cx: &mut Context<Self>) -> Self {
        WaveformPanel::new(state, cx)
    }
}

impl EventEmitter<PanelEvent> for WaveformPanel {}

impl Focusable for WaveformPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for WaveformPanel {
    fn panel_name(&self) -> &'static str {
        "waveform"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("waveform")
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel);
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
            panel::popout_button(&cx.entity(), "waveform", self.tab_panel.clone()),
        ])
    }
}

impl Render for WaveformPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.state.player.read(cx).now_playing();

        // Kick a decode when the playing track changes.
        if let Some(now) = &now {
            if self.track.as_deref() != Some(now.path.as_path()) {
                let path = now.path.clone();
                self.start_decode(path, cx);
            }
            // The position clock only moves while a session runs, and pause
            // and track skips do not notify; poll by frame like the player
            // bar does. No session: fully parked.
            window.request_animation_frame();
        }

        let body = match (&now, &self.peaks) {
            (None, _) => self.message("nothing playing").into_any_element(),
            (Some(_), Peaks::Decoding) => self.message("analyzing audio...").into_any_element(),
            (Some(_), Peaks::Failed) => self
                .message("waveform unavailable for this track")
                .into_any_element(),
            (Some(now), Peaks::Ready(peaks)) => {
                let progress = now
                    .duration_secs
                    .filter(|d| *d > 0.0)
                    .map(|d| (now.position_secs / d) as f32)
                    .unwrap_or(0.0);
                self.strip(progress, peaks.clone()).into_any_element()
            }
            (Some(_), Peaks::None) => div().into_any_element(),
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
