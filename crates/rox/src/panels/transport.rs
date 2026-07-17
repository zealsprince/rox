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
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::store::TrackMeta;
use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::player::{fmt_time, fmt_time_padded};

/// The playback panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TransportConfig {
    #[serde(default)]
    pub align: Align,
    /// The stop button that ejects the playing track.
    #[serde(default)]
    pub stop: bool,
    /// The random button that plays one track from anywhere in the library.
    #[serde(default)]
    pub random: bool,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

/// The playback controls: prev, the seek nudges around play/pause, next,
/// and the loop and shuffle modes, plus the optional stop and random
/// buttons. What is
/// playing lives in the track info panel. The pump's
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

    /// The panel's own dropdown entries: the optional button toggles, the
    /// same knobs the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Stop Button")
                .checked(self.config.stop)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.stop = !this.config.stop;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Random Button")
                .checked(self.config.random)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.random = !this.config.random;
                        cx.notify();
                    });
                }),
        )
    }

    /// Pick one track from anywhere in the library and play it as a fresh
    /// one-track queue.
    fn play_random(&mut self, cx: &mut Context<Self>) {
        let paths = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            if projection.is_empty() {
                return;
            }
            let id = projection.db_id[random_index(projection.len())];
            library.paths_for(&[id]).ok()
        };
        let Some(paths) = paths else { return };
        self.state
            .player
            .update(cx, |player, cx| player.play(paths, cx));
    }
}

/// A random index below `len`, off the std hasher's per-process random
/// keys; picking a track does not need a rand dependency.
fn random_index(len: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let hash = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (hash % len as u64) as usize
}

impl PanelSettings for TransportPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Controls"]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                "stop",
                Some("the stop button that ejects the playing track"),
                panel::toggle(
                    self.config.stop,
                    |this: &mut Self, stop, cx| {
                        this.config.stop = stop;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "random",
                Some("the random button that plays one track from anywhere in the library"),
                panel::toggle(
                    self.config.random,
                    |this: &mut Self, random, cx| {
                        this.config.random = random;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
    }
}

impl Render for TransportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(cx).into_any_element())
    }
}

impl TransportPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let playing = player.is_playing();
        let active = player.is_active();
        // Loop state reads through the button itself: dim while off, the
        // accent while on, the one-track glyph for single-track loop.
        let (loop_icon, loop_color) = match player.loop_mode() {
            LoopMode::Off => (icons::REPEAT, palette::text_faint()),
            LoopMode::All => (icons::REPEAT, palette::accent()),
            LoopMode::One => (icons::REPEAT_1, palette::accent()),
        };
        // Shuffle reads the same way: dim while off, the accent while on.
        let shuffle_color = if player.shuffle() {
            palette::accent()
        } else {
            palette::text_faint()
        };

        // Play/pause is the primary action, so it gets the filled round
        // button while everything around it stays flat.
        let play_pause = div()
            .size(tokens::PLAY_SIZE)
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
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
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
            // Stop ejects the track: the session drops and every view over
            // it goes idle. Dim while nothing is loaded.
            .when(self.config.stop, |d| {
                d.child(panel::icon_control(
                    icons::STOP,
                    if active {
                        palette::text()
                    } else {
                        palette::text_faint()
                    },
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.stop(cx)),
                    cx,
                ))
            })
            .child(panel::icon_control(
                loop_icon,
                loop_color,
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                cx,
            ))
            .child(panel::icon_control(
                icons::SHUFFLE,
                shuffle_color,
                |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.toggle_shuffle()),
                cx,
            ))
            .when(self.config.random, |d| {
                d.child(panel::icon_control(
                    icons::DICE,
                    palette::text(),
                    |this: &mut Self, cx| this.play_random(cx),
                    cx,
                ))
            })
    }
}

/// The track info panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TrackInfoConfig {
    #[serde(default)]
    pub align: Align,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
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

impl PanelSettings for TrackInfoPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Layout"]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
    }
}

impl Render for TrackInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(cx).into_any_element())
    }
}

impl TrackInfoPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
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
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD);

        let Some(now) = now else {
            // Nothing to describe: a session still opening, or the reason
            // one failed to start. Plain idle stays blank.
            let line = if active {
                Some("opening...".into())
            } else {
                error
            };
            return root.when_some(line, |root, line| {
                root.child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_color(palette::text_muted())
                        .child(line),
                )
            });
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
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct VolumeConfig {
    #[serde(default)]
    pub align: Align,
    /// Let the slider fill whatever width the panel has instead of capping
    /// at its natural size.
    #[serde(default)]
    pub stretch: bool,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
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

impl PanelSettings for VolumePanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Layout"]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
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

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
    }
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(cx).into_any_element())
    }
}

impl VolumePanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
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
            .min_w(tokens::SLIDER_MIN_W)
            .when(!self.config.stretch, |d| d.max_w(tokens::SLIDER_MAX_W))
            .h(tokens::CONTROL_H)
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
                    // Muted keeps the knob where it is and dims the fill. The
                    // slider spans 0 to 100%; a louder hand-edited settings
                    // value shows as full.
                    move |bounds, _, window, _| {
                        panel::paint_slider(volume, muted, bounds, window);
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
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            // Scrolling anywhere on the strip nudges the volume; like the
            // slider it spans 0 to 100% and unmutes on touch.
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let lines = match event.delta {
                    gpui::ScrollDelta::Lines(lines) => lines.y,
                    gpui::ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
                };
                // A wheel notch arrives as 3 lines, so one notch steps 5%.
                this.state.player.update(cx, |player, cx| {
                    let volume = (player.volume() + lines / 3.0 * 0.05).clamp(0.0, 1.0);
                    player.set_volume(volume, cx);
                });
            }))
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
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for last.fm. Only draws while scrobbling is
    /// connected and on.
    #[serde(default)]
    pub scrobble_marker: bool,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

fn default_true() -> bool {
    true
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            timings: true,
            show_total: false,
            scrobble_marker: false,
            theme: PanelTheme::default(),
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

    /// The panel's own dropdown entries: the quick timings and marker
    /// toggles, the same knobs the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Show Timings")
                .checked(self.config.timings)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.timings = !this.config.timings;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Scrobble Marker")
                .checked(self.config.scrobble_marker)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.scrobble_marker = !this.config.scrobble_marker;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for SeekStripPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Clocks"]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
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
            .child(panel::setting_row(
                "scrobble marker",
                Some("a thin line where the track counts as scrobbled to last.fm"),
                panel::toggle(
                    self.config.scrobble_marker,
                    |this: &mut Self, on, cx| {
                        this.config.scrobble_marker = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
    }
}

/// The track line centered in whatever height the panel gets: unplayed side
/// dim, played side solid, the waveform's playhead on top. `marker` draws
/// the scrobble threshold as a thin full-height line under the playhead.
fn paint_strip(progress: f32, marker: Option<f32>, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let head_x = progress.clamp(0.0, 1.0) * w;
    let line_y = (h - tokens::SEEK_STRIP_H) / 2.0;
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(w), px(tokens::SEEK_STRIP_H)),
        ),
        palette::alpha(palette::accent(), 0x33),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(head_x), px(tokens::SEEK_STRIP_H)),
        ),
        palette::accent(),
    ));
    if let Some(marker) = marker {
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(marker.clamp(0.0, 1.0) * w),
                    bounds.origin.y,
                ),
                size(px(1.0), px(h)),
            ),
            palette::alpha(palette::highlight(), 0x80),
        ));
    }
    window.paint_quad(fill(
        Bounds::new(
            point(
                bounds.origin.x + px(head_x - tokens::PLAYHEAD_W / 2.0),
                bounds.origin.y,
            ),
            size(px(tokens::PLAYHEAD_W), px(h)),
        ),
        palette::alpha(palette::highlight(), 0xd9),
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
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(window, cx).into_any_element())
    }
}

impl SeekStripPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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
            // Idle: the strip stays blank until a session brings a track.
            return root;
        };

        let progress = now
            .duration_secs
            .filter(|d| *d > 0.0)
            .map(|d| (now.position_secs / d) as f32)
            .unwrap_or(0.0);
        // The marker only shows where a scrobble could actually land: the
        // toggle on and the scrobbler armed.
        let marker = (self.config.scrobble_marker)
            .then(|| self.state.scrobbler.read(cx).marker())
            .flatten();
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
                        paint_strip(progress, marker, bounds, window);
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
        root.gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .child(clock(fmt_time_padded(now.position_secs, digits)))
            .child(track)
            .child(clock(ending).cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.config.show_total = !this.config.show_total;
                    cx.notify();
                }),
            ))
    }
}

/// The Panel and focus plumbing is identical across the transport panels;
/// only the name and the minimum width differ. Every transport panel has a
/// per-view config struct (a `config` field, a `config_menu` method, and a
/// PanelSettings impl): the layout dump carries the config, Duplicate
/// copies it, and the dropdown gets the panel's own entries plus Panel
/// Settings in a block above the shared items. The minimum width is what
/// the resizable layout refuses to squeeze the panel below, so controls
/// never slide off screen; a panel whose controls depend on its config
/// passes a closure over `&self` instead of a literal.
macro_rules! transport_panel {
    ($panel:ty, $name:literal, min_w = $min_w:literal) => {
        transport_panel!($panel, $name, min_w = |_: &$panel| px($min_w));
    };
    ($panel:ty, $name:literal, min_w = $min_w:expr) => {
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
                gpui::size(($min_w)(self), rox_dock::resizable::PANEL_MIN_SIZE)
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
                // settings window, apart from the core panel items.
                let menu = self.config_menu(menu, cx);
                let menu = menu.separator();
                let menu = panel_settings::settings_item(menu, &cx.entity());
                // Duplicate hand-rolled rather than through
                // `panel::duplicate_item` because the copy takes the config
                // along, like the library's.
                let weak = cx.entity().downgrade();
                let menu = menu.item(
                    PopupMenuItem::new("Duplicate")
                        .icon(Icon::default().path(icons::COPY))
                        .on_click(move |_, window, cx| {
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
                        }),
                );
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
// playback row's seven buttons plus 32px (icon, padding, gap) for each
// optional button turned on, the volume strip's icon, 80px slider floor,
// and readout, the seek strip's clocks around a usable track, and enough
// of the track info line to read a title.
transport_panel!(
    TransportPanel,
    "playback",
    min_w = |this: &TransportPanel| {
        let extras = this.config.stop as u8 + this.config.random as u8;
        px(242. + f32::from(extras) * 32.)
    }
);
transport_panel!(VolumePanel, "volume", min_w = 200.);
transport_panel!(SeekStripPanel, "seek", min_w = 160.);
transport_panel!(TrackInfoPanel, "track info", min_w = 120.);
