//! The transport panels - playback controls, the track info readout, a
//! volume strip, and a click-to-seek strip - the app's whole playback UI,
//! living in the bottom dock by default. Each is a view over the shared
//! player entity, exactly like the audio views: duplicates are fresh views,
//! pop-outs rehost the entity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, FontFeatures, MouseButton, Pixels, Subscription,
    WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::store::TrackMeta;
use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::palette;
use crate::panel::{self, AppState, Customizable, ScrubState};
use crate::panels::library::LibraryEvent;
use crate::player::{fmt_time, fmt_time_padded};

/// Thickness of the seek strip's track line.
const STRIP_H: f32 = 6.0;

/// The playback panel's per-view config: what a saved layout restores,
/// and what the customize window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TransportConfig {
    #[serde(default)]
    pub align: Align,
}

/// The playback controls: prev, the seek nudges around play/pause, next,
/// and the loop mode. What is playing lives in the track info panel. The pump's
/// tick notifies the player while a session runs, so the observe below
/// keeps the play state fresh even in a popped-out window.
pub struct TransportPanel {
    state: AppState,
    config: TransportConfig,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl TransportPanel {
    pub fn new(state: AppState, config: TransportConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        TransportPanel {
            state,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// No quick dropdown entries; the alignment lives in the customize
    /// window.
    fn config_menu(&self, menu: PopupMenu, _cx: &mut Context<Self>) -> PopupMenu {
        menu
    }
}

impl Customizable for TransportPanel {
    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        align_row(
            self.config.align,
            |this: &mut Self, align, cx| {
                this.config.align = align;
                cx.notify();
            },
            cx,
        )
        .into_any_element()
    }
}

impl Render for TransportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        let playing = player.is_playing();
        // Loop state reads through the button itself: dim while off, the
        // accent while on, the one-track glyph for single-track loop.
        let (loop_icon, loop_color) = match player.loop_mode() {
            LoopMode::Off => (icons::REPEAT, palette::text_faint()),
            LoopMode::All => (icons::REPEAT, palette::accent()),
            LoopMode::One => (icons::REPEAT_1, palette::accent()),
        };

        // Play/pause is the primary action, so it gets the filled round
        // button while everything around it stays flat.
        let play_pause = div()
            .size(px(30.))
            .flex_none()
            .rounded_full()
            .bg(palette::accent())
            .hover(|d| d.bg(palette::accent_hover()))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut Self, _, _, cx| {
                    this.state.player.update(cx, |p, _| p.toggle_pause())
                }),
            )
            .child(
                svg()
                    .path(if playing { icons::PAUSE } else { icons::PLAY })
                    .size_4()
                    .text_color(palette::text_on_accent()),
            );

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap_1()
            .px_2()
            .child(panel::icon_control(
                icons::SKIP_BACK,
                palette::text(),
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.prev()),
                cx,
            ))
            .child(panel::icon_control(
                icons::REWIND,
                palette::text(),
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                cx,
            ))
            .child(play_pause)
            .child(panel::icon_control(
                icons::FAST_FORWARD,
                palette::text(),
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                cx,
            ))
            .child(panel::icon_control(
                icons::SKIP_FORWARD,
                palette::text(),
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.next()),
                cx,
            ))
            .child(panel::icon_control(
                loop_icon,
                loop_color,
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                cx,
            ))
    }
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
fn justify(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::Right => d.justify_end(),
    }
}

/// The alignment setting row the transport panels' customize windows
/// share.
fn align_row<P: 'static>(
    current: Align,
    on_pick: impl Fn(&mut P, Align, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::setting_row(
        "alignment",
        Some("where the controls sit when the panel has room to spare"),
        panel::icon_choices(
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

/// The track info panel's per-view config: what a saved layout restores,
/// and what the customize window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TrackInfoConfig {
    #[serde(default)]
    pub align: Align,
}

/// The track info readout the playback panel's status line grew into: one
/// line with the playing track's tags from the library - track number,
/// title, duration, then artist and album - with the session errors and
/// the idle message in its place while nothing shows.
pub struct TrackInfoPanel {
    state: AppState,
    config: TrackInfoConfig,
    /// The playing path's tags, or None for a file the library does not
    /// know. Cached because the pump notifies every frame and the lookup is
    /// a database query; cleared when the track or the catalog changes.
    meta: Option<(PathBuf, Option<TrackMeta>)>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl TrackInfoPanel {
    pub fn new(state: AppState, config: TrackInfoConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, _: &LibraryEvent, cx| {
                this.meta = None;
                cx.notify();
            },
        );
        TrackInfoPanel {
            state,
            config,
            meta: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _library_changed,
        }
    }

    /// No quick dropdown entries; the alignment lives in the customize
    /// window.
    fn config_menu(&self, menu: PopupMenu, _cx: &mut Context<Self>) -> PopupMenu {
        menu
    }

    /// The playing path's tags, from the cache or one lookup on a miss.
    fn meta_for(&mut self, path: &Path, cx: &App) -> Option<&TrackMeta> {
        if self.meta.as_ref().map(|(p, _)| p.as_path()) != Some(path) {
            let meta = self.state.library.read(cx).meta_for(path);
            self.meta = Some((path.to_path_buf(), meta));
        }
        self.meta.as_ref().and_then(|(_, meta)| meta.as_ref())
    }
}

impl Customizable for TrackInfoPanel {
    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        align_row(
            self.config.align,
            |this: &mut Self, align, cx| {
                this.config.align = align;
                cx.notify();
            },
            cx,
        )
        .into_any_element()
    }
}

impl Render for TrackInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        let now = player.now_playing();
        let active = player.is_active();
        let ended = player.queue_ended();
        let error = player.error();

        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap_2()
            .px_3();

        let Some(now) = now else {
            // Nothing to describe: a session still opening, the reason one
            // failed to start, or plain idle.
            let line = if active {
                "opening...".into()
            } else {
                error.unwrap_or_else(|| "nothing playing".into())
            };
            return root.child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_color(palette::text_muted())
                    .child(line),
            );
        };

        // An untagged file still shows something: its file name for the
        // title, no byline.
        let meta = self.meta_for(&now.path, cx);
        let title = meta.map(|m| m.title.clone()).unwrap_or_else(|| {
            now.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| now.path.display().to_string())
        });
        let mut heading = String::new();
        if let Some(no) = meta.map(|m| m.track_no).filter(|no| *no > 0) {
            heading.push_str(&format!("{no:02}. "));
        }
        heading.push_str(&title);
        if let Some(duration) = now.duration_secs {
            heading.push_str(&format!(" ({})", fmt_time(duration)));
        }
        let byline = meta
            .map(|m| [m.artist.as_str(), m.album.as_str()])
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" - ");

        // One line: the heading, the byline dimmed beside it, both giving
        // way gracefully when the panel runs out of room.
        root.child(div().flex_shrink_0().max_w_full().truncate().child(heading))
            .when(!byline.is_empty(), |d| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_muted())
                        .child(byline),
                )
            })
            .when(ended, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child("(queue finished)"),
                )
            })
    }
}

/// The volume panel's per-view config: what a saved layout restores, and
/// what the customize window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct VolumeConfig {
    #[serde(default)]
    pub align: Align,
    /// Let the slider fill whatever width the panel has instead of capping
    /// at its natural size.
    #[serde(default)]
    pub stretch: bool,
}

/// The volume strip: the speaker button that toggles mute, and the volume
/// slider with the readout.
pub struct VolumePanel {
    state: AppState,
    config: VolumeConfig,
    /// The slider's painted bounds and drag state.
    scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl VolumePanel {
    pub fn new(state: AppState, config: VolumeConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        VolumePanel {
            state,
            config,
            scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The panel's own dropdown entries: the quick stretch toggle, the
    /// same knob the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Stretch")
                .checked(self.config.stretch)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.stretch = !this.config.stretch;
                        cx.notify();
                    });
                }),
        )
    }
}

impl Customizable for VolumePanel {
    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                "stretch",
                Some("let the slider fill the panel instead of capping its width"),
                panel::toggle(
                    self.config.stretch,
                    |this: &mut Self, stretch, cx| {
                        this.config.stretch = stretch;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

/// The volume slider: a rounded track, the setting as the filled side, a
/// round knob at the position. Muted keeps the knob where it is and dims
/// the fill. The slider spans 0 to 100%; a louder hand-edited settings
/// value shows as full.
fn paint_slider(volume: f32, muted: bool, bounds: Bounds<Pixels>, window: &mut Window) {
    const TRACK_H: f32 = 4.0;
    const KNOB: f32 = 12.0;

    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= KNOB || h <= 0.0 {
        return;
    }

    // The knob's travel is inset by its radius so it never clips the ends.
    let knob_x = KNOB / 2.0 + volume.clamp(0.0, 1.0) * (w - KNOB);
    let track_y = bounds.origin.y + px((h - TRACK_H) / 2.0);
    window.paint_quad(
        fill(
            Bounds::new(point(bounds.origin.x, track_y), size(px(w), px(TRACK_H))),
            palette::bg_control(),
        )
        .corner_radii(px(TRACK_H / 2.0)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, track_y),
                size(px(knob_x), px(TRACK_H)),
            ),
            if muted {
                palette::alpha(palette::accent(), 0x33)
            } else {
                palette::accent()
            },
        )
        .corner_radii(px(TRACK_H / 2.0)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(knob_x - KNOB / 2.0),
                    bounds.origin.y + px((h - KNOB) / 2.0),
                ),
                size(px(KNOB), px(KNOB)),
            ),
            if muted {
                palette::text_dim()
            } else {
                palette::text_bright()
            },
        )
        .corner_radii(px(KNOB / 2.0)),
    );
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.read(cx);
        let volume = player.volume();
        let muted = player.muted();
        let percent = (volume * 100.0).round() as u32;

        // The speaker doubles as the mute toggle and the state readout:
        // crossed out while muted, fewer waves at low volume.
        let (speaker, speaker_color) = if muted {
            (icons::VOLUME_X, palette::text_faint())
        } else if volume <= 0.5 {
            (icons::VOLUME_1, palette::text())
        } else {
            (icons::VOLUME_2, palette::text())
        };

        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let slider = div()
            .flex_1()
            .min_w(px(80.))
            .when(!self.config.stretch, |d| d.max_w(px(200.)))
            .h(px(22.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        this.state
                            .player
                            .update(cx, |player, cx| player.set_volume(fraction, cx));
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    move |bounds, _, window, _| {
                        paint_slider(volume, muted, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let player = player.clone();
                            move |fraction, cx| {
                                player.update(cx, |player, cx| player.set_volume(fraction, cx))
                            }
                        });
                    },
                )
                .size_full(),
            );

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap_2()
            .px_3()
            .child(panel::icon_control(
                speaker,
                speaker_color,
                |this: &mut Self, cx| {
                    this.state
                        .player
                        .update(cx, |player, cx| player.toggle_mute(cx))
                },
                cx,
            ))
            .child(slider)
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .text_center()
                    .text_color(palette::text_muted())
                    .child(format!("{percent}%")),
            )
    }
}

/// The seek panel's per-view config: what a saved layout restores, and
/// what the panel's dropdown menu edits. New display knobs land here, same
/// as the library's.
#[derive(Clone, Serialize, Deserialize)]
pub struct SeekConfig {
    /// The elapsed and remaining clocks around the strip.
    #[serde(default = "default_true")]
    pub timings: bool,
    /// The ending clock shows the full duration instead of the time left;
    /// clicking the clock flips it.
    #[serde(default)]
    pub show_total: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            timings: true,
            show_total: false,
        }
    }
}

/// The seek strip: the waveform minus the peaks - a track line with the
/// played side in the accent and a playhead, click or drag to seek, the
/// elapsed and remaining clocks at its ends. Position and seek come off
/// the player the same way the waveform's do.
pub struct SeekStripPanel {
    state: AppState,
    config: SeekConfig,
    /// The strip's painted bounds and drag state, for scrub mapping.
    scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl SeekStripPanel {
    pub fn new(state: AppState, config: SeekConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SeekStripPanel {
            state,
            config,
            scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The panel's own dropdown entries: the quick timings toggle, the
    /// same knob the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Show Timings")
                .checked(self.config.timings)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.timings = !this.config.timings;
                        cx.notify();
                    });
                }),
        )
    }
}

impl Customizable for SeekStripPanel {
    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(panel::setting_row(
                "timings",
                Some("the elapsed and ending clocks around the strip"),
                panel::toggle(
                    self.config.timings,
                    |this: &mut Self, timings, cx| {
                        this.config.timings = timings;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "ending",
                Some("count down the time left or show the full length"),
                panel::choices(
                    &[("remaining", false), ("total", true)],
                    self.config.show_total,
                    |this: &mut Self, show_total, cx| {
                        this.config.show_total = show_total;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
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
        palette::alpha(palette::accent(), 0x33),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(head_x), px(STRIP_H)),
        ),
        palette::accent(),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x + px(head_x - 1.0), bounds.origin.y),
            size(px(2.0), px(h)),
        ),
        palette::alpha(palette::text_bright(), 0xd9),
    ));
}

/// A clock beside the strip: muted, fixed in the row, digits tabular so a
/// tick never changes the text width.
fn clock(text: String) -> Div {
    let mut clock = div().flex_none().text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
    clock.child(text)
}

impl Render for SeekStripPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.state.player.read(cx).now_playing();

        // The position clock only moves while a session runs; poll by frame
        // like the waveform does. No session: fully parked.
        if now.is_some() {
            window.request_animation_frame();
        }

        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center();

        let Some(now) = now else {
            return root
                .justify_center()
                .text_color(palette::text_muted())
                .child("nothing playing");
        };

        let progress = now
            .duration_secs
            .filter(|d| *d > 0.0)
            .map(|d| (now.position_secs / d) as f32)
            .unwrap_or(0.0);
        // The seek click lives on the track alone so the clocks beside it
        // stay inert.
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let track = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        panel::seek_fraction(&this.state.player, fraction, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    move |bounds, _, window, _| {
                        paint_strip(progress, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let player = player.clone();
                            move |fraction, cx| panel::seek_fraction(&player, fraction, cx)
                        });
                    },
                )
                .size_full(),
            );

        if !self.config.timings {
            return root.child(track);
        }

        // The clocks the reference bar shows: elapsed on the left, the
        // ending clock on the right - time left, or the full duration when
        // toggled - and "-:--" until the duration resolves. Minutes pad to
        // the duration's digits so neither clock changes width mid-track
        // and wiggles the strip.
        let digits = now
            .duration_secs
            .map(|d| (d as u64 / 60).to_string().len())
            .unwrap_or(1);
        let ending = match now.duration_secs {
            Some(d) if self.config.show_total => fmt_time_padded(d, digits),
            Some(d) => format!(
                "-{}",
                fmt_time_padded((d - now.position_secs).max(0.0), digits)
            ),
            None => "-:--".into(),
        };
        root.gap_2()
            .px_2()
            .child(clock(fmt_time_padded(now.position_secs, digits)))
            .child(track)
            .child(
                clock(ending).cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.config.show_total = !this.config.show_total;
                        cx.notify();
                    }),
                ),
            )
    }
}

/// The Panel and focus plumbing is identical across the transport panels;
/// only the name and the minimum width differ. Every transport panel has a
/// per-view config struct (a `config` field, a `config_menu` method, and a
/// Customizable impl): the layout dump carries the config, Duplicate
/// copies it, and the dropdown gets the panel's own entries plus Customize
/// in a block above the shared items. The minimum width is what the
/// resizable layout refuses to squeeze the panel below, so controls never
/// slide off screen.
macro_rules! transport_panel {
    ($panel:ty, $name:literal, min_w = $min_w:literal) => {
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

            fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                gpui::SharedString::from($name)
            }

            fn inner_padding(&self, _cx: &App) -> bool {
                false
            }

            fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
                gpui::size(px($min_w), rox_dock::resizable::PANEL_MIN_SIZE)
            }

            /// The layout dump carries the panel's config; the builder
            /// registered in `workspace::register_panels` reads it back.
            fn dump(&self, _cx: &App) -> rox_dock::PanelState {
                let mut state = rox_dock::PanelState::new(self);
                state.info = rox_dock::PanelInfo::panel(
                    serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
                );
                state
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

            fn dropdown_menu(
                &mut self,
                menu: PopupMenu,
                _window: &mut Window,
                cx: &mut Context<Self>,
            ) -> PopupMenu {
                // The config block: the panel's quick entries and the
                // customize window, apart from the core panel items.
                let menu = self.config_menu(menu, cx);
                let menu = panel::customize_item(menu, &cx.entity());
                let menu = menu.separator();
                // Duplicate hand-rolled rather than through
                // `panel::duplicate_item` because the copy takes the config
                // along, like the library's.
                let weak = cx.entity().downgrade();
                let menu = menu.item(PopupMenuItem::new("Duplicate").on_click(
                    move |_, window, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        let (state, config, tabs) = {
                            let panel = this.read(cx);
                            (
                                panel.state.clone(),
                                panel.config.clone(),
                                panel.tab_panel.clone(),
                            )
                        };
                        let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                            return;
                        };
                        let dup = cx.new(|cx| <$panel>::new(state, config, cx));
                        tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                    },
                ));
                panel::popout_item(
                    menu,
                    &cx.entity(),
                    self.tab_panel.clone(),
                    self.state.clone(),
                )
            }
        }
    };
}

// The widths below are each panel's controls at their tightest: the
// playback row's six buttons, the volume strip's icon, 80px slider floor,
// and readout, the seek strip's clocks around a usable track, and enough
// of the track info line to read a title.
transport_panel!(TransportPanel, "playback", min_w = 210.);
transport_panel!(VolumePanel, "volume", min_w = 200.);
transport_panel!(SeekStripPanel, "seek", min_w = 160.);
transport_panel!(TrackInfoPanel, "track info", min_w = 120.);
