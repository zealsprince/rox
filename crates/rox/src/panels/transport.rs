//! The transport panels - playback controls, the track info readout, a
//! volume strip, and a click-to-seek strip - the app's whole playback UI,
//! living in the bottom dock by default. Each is a view over the shared
//! player entity, exactly like the audio views: duplicates are fresh views,
//! pop-outs rehost the entity.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, FontFeatures, MouseButton, Pixels, ScrollHandle,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::store::TrackMeta;
use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, align_row, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::player::{fmt_time, fmt_time_padded, observe_view};

/// The playback panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
    /// The previous-track button.
    #[serde(default = "default_true")]
    pub prev: bool,
    /// The play/pause button, the primary transport action.
    #[serde(default = "default_true")]
    pub play: bool,
    /// The next-track button.
    #[serde(default = "default_true")]
    pub next: bool,
    /// The seek nudge buttons that jump back and forward ten seconds. On by
    /// default; drop them on a compact bar that only needs prev/play/next.
    #[serde(default = "default_true")]
    pub seek: bool,
    /// The loop button that cycles off, all, one.
    #[serde(default = "default_true")]
    pub repeat: bool,
    /// The shuffle button.
    #[serde(default = "default_true")]
    pub shuffle: bool,
    /// The stop button that ejects the playing track.
    #[serde(default)]
    pub stop: bool,
    /// The random button that plays one track from anywhere in the library.
    #[serde(default)]
    pub random: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        // The seek/loop/shuffle buttons ship on, matching what a layout with
        // none of these fields set decodes to; only stop and random are
        // opt-in.
        TransportConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            prev: true,
            play: true,
            next: true,
            seek: true,
            repeat: true,
            shuffle: true,
            stop: false,
            random: false,
        }
    }
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
        // Play state, loop, and shuffle change on a user action, never on
        // the position tick, so ride the gated observe.
        let _player_changed = observe_view(&state.player, cx);
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

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Controls", icons::PLAY)]
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
                "Previous",
                Some("The previous-track button"),
                panel::toggle(
                    self.config.prev,
                    |this: &mut Self, prev, cx| {
                        this.config.prev = prev;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Play",
                Some("The play and pause button"),
                panel::toggle(
                    self.config.play,
                    |this: &mut Self, play, cx| {
                        this.config.play = play;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Next",
                Some("The next-track button"),
                panel::toggle(
                    self.config.next,
                    |this: &mut Self, next, cx| {
                        this.config.next = next;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Seek Buttons",
                Some("The back and forward ten-second nudges around play"),
                panel::toggle(
                    self.config.seek,
                    |this: &mut Self, seek, cx| {
                        this.config.seek = seek;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Loop",
                Some("The loop button that cycles off, all, one"),
                panel::toggle(
                    self.config.repeat,
                    |this: &mut Self, repeat, cx| {
                        this.config.repeat = repeat;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Shuffle",
                Some("The shuffle button"),
                panel::toggle(
                    self.config.shuffle,
                    |this: &mut Self, shuffle, cx| {
                        this.config.shuffle = shuffle;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Stop",
                Some("The stop button that ejects the playing track"),
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
                "Random",
                Some("The random button that plays one track from anywhere in the library"),
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
}

impl Render for TransportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
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
            .when(self.config.prev, |d| {
                d.child(panel::icon_control(
                    icons::SKIP_BACK,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.prev()),
                    cx,
                ))
            })
            .when(self.config.seek, |d| {
                d.child(panel::icon_control(
                    icons::REWIND,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                    cx,
                ))
            })
            .when(self.config.play, |d| d.child(play_pause))
            .when(self.config.seek, |d| {
                d.child(panel::icon_control(
                    icons::FAST_FORWARD,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                    cx,
                ))
            })
            .when(self.config.next, |d| {
                d.child(panel::icon_control(
                    icons::SKIP_FORWARD,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.next()),
                    cx,
                ))
            })
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
            .when(self.config.repeat, |d| {
                d.child(panel::icon_control(
                    loop_icon,
                    loop_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                    cx,
                ))
            })
            .when(self.config.shuffle, |d| {
                d.child(panel::icon_control(
                    icons::SHUFFLE,
                    shuffle_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.toggle_shuffle()),
                    cx,
                ))
            })
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
#[derive(Clone, Serialize, Deserialize)]
pub struct TrackInfoConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
    /// What a line too long for the panel does; see [`MarqueeMode`].
    #[serde(default)]
    pub marquee: MarqueeMode,
    /// The crawl's pace for the scroll and loop modes, pixels per second.
    #[serde(default = "default_marquee_speed")]
    pub marquee_speed: f32,
    /// How long the scroll rests at each end before moving again,
    /// seconds.
    #[serde(default = "default_marquee_delay")]
    pub marquee_delay: f32,
    /// Show one piece at a time - the heading, then the byline - fading
    /// between them instead of the whole line at once. Independent of
    /// the marquee: the shown piece still crawls if it overflows.
    #[serde(default)]
    pub swap: bool,
    /// How long each piece sits fully shown before the swap, seconds.
    #[serde(default = "default_swap_secs")]
    pub swap_secs: f32,
}

impl Default for TrackInfoConfig {
    fn default() -> Self {
        TrackInfoConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            marquee: MarqueeMode::default(),
            marquee_speed: default_marquee_speed(),
            marquee_delay: default_marquee_delay(),
            swap: false,
            swap_secs: default_swap_secs(),
        }
    }
}

/// What the track line does when it outgrows the panel.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarqueeMode {
    /// Cut the line off where the room runs out.
    #[default]
    Off,
    /// Crawl to the end, rest, crawl back, rest, repeat.
    Scroll,
    /// Crawl one way without end, the line chasing its own tail.
    Loop,
}

/// The crawl speed range the settings slider spans, pixels per second.
const MARQUEE_SPEED_MIN: f32 = 10.0;
const MARQUEE_SPEED_MAX: f32 = 120.0;

/// The default crawl pace, a comfortable read.
fn default_marquee_speed() -> f32 {
    30.0
}

/// The swap dwell range the settings slider spans, seconds.
const SWAP_SECS_MIN: f32 = 1.0;
const SWAP_SECS_MAX: f32 = 15.0;

/// The default dwell, long enough to read either piece.
fn default_swap_secs() -> f32 {
    4.0
}

/// The end-rest range the settings slider spans, seconds.
const MARQUEE_DELAY_MIN: f32 = 0.0;
const MARQUEE_DELAY_MAX: f32 = 10.0;

/// The default rest at each end of a scroll, a beat to read the edge.
fn default_marquee_delay() -> f32 {
    2.0
}
/// The gap between the line's two copies in loop mode, the breather
/// between a tail and the next head.
const MARQUEE_GAP: f32 = 48.0;
/// The swap fade's length, going out and coming in.
const SWAP_FADE_SECS: f32 = 0.4;

/// The track line's crawl state while the marquee setting is on. The
/// scroll handle owns the clipping and reports the overflow off the last
/// layout; the rest drives the offset through it, one leg at a time.
struct MarqueeScroll {
    handle: ScrollHandle,
    /// How far the line sits left of home, in pixels.
    offset: f32,
    /// The scroll crawl's direction: 1 heading out, -1 heading home.
    dir: f32,
    /// Time left resting at an end before the next leg starts.
    hold: f32,
    /// The configured rest at each end, mirrored off the panel config by
    /// the body each frame so the crawl state can refill `hold` itself.
    delay: f32,
    /// The last frame's clock, for the per-frame step.
    last_tick: Instant,
    /// The path the crawl belongs to; a track change starts over.
    path: Option<PathBuf>,
    /// Loop mode's verdict off the last layout: whether one copy alone
    /// overflows, so the line renders doubled and wraps.
    looping: bool,
    /// The piece swap mode shows, counting through heading and byline.
    swap_ix: usize,
    /// When the shown piece's cycle started: fade in, dwell, fade out.
    swap_at: Instant,
    /// Whether the swap is actually cycling this frame - on, with a
    /// byline to trade against. The body sets it; the crawl reads it to
    /// decide between bouncing back and parking at the end.
    swap_live: bool,
    /// The scroll-mode handshake: the crawl finished its trip out (or a
    /// fitting piece its dwell) and the swap may fade the piece away.
    crawl_done: bool,
    /// When the fade-out started; None while the piece is coming in or
    /// fully up.
    fade_at: Option<Instant>,
}

impl MarqueeScroll {
    fn new() -> Self {
        MarqueeScroll {
            handle: ScrollHandle::new(),
            offset: 0.0,
            dir: 1.0,
            hold: default_marquee_delay(),
            delay: default_marquee_delay(),
            last_tick: Instant::now(),
            path: None,
            looping: false,
            swap_ix: 0,
            swap_at: Instant::now(),
            swap_live: false,
            crawl_done: false,
            fade_at: None,
        }
    }

    /// Send the crawl home without touching the swap cycle, for a fresh
    /// piece coming in.
    fn rehome(&mut self) {
        self.offset = 0.0;
        self.dir = 1.0;
        self.hold = self.delay;
        self.last_tick = Instant::now();
        self.looping = false;
        self.crawl_done = false;
        self.fade_at = None;
    }

    /// Back home, resting, the swap cycle back on its heading.
    fn reset(&mut self) {
        self.rehome();
        self.swap_ix = 0;
        self.swap_at = Instant::now();
    }

    /// One frame of the scroll crawl: run the rest down, then step along
    /// the current leg. Without `park` it turns around with a fresh rest
    /// at each end; with it (the swap rides the crawl) it stays put once
    /// it has crawled out and rested, raising `crawl_done` for the swap
    /// to fade the piece away. The step clamps so a stalled frame never
    /// teleports the line.
    fn advance(&mut self, overflow: f32, speed: f32, park: bool) {
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        if self.hold > 0.0 {
            self.hold -= dt;
            return;
        }
        if park && self.offset >= overflow {
            self.crawl_done = true;
            return;
        }
        self.offset += self.dir * speed * dt;
        if self.offset >= overflow {
            self.offset = overflow;
            self.hold = self.delay;
            if !park {
                self.dir = -1.0;
            }
        } else if self.offset <= 0.0 {
            self.offset = 0.0;
            self.dir = 1.0;
            self.hold = self.delay;
        }
    }

    /// One frame of the endless crawl: step left at the pace, wrapping
    /// once a full copy and its gap have gone by, so the doubled line
    /// reads as one unbroken loop.
    fn advance_loop(&mut self, period: f32, speed: f32) {
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        self.offset += speed * dt;
        if self.offset >= period {
            self.offset -= period;
        }
    }
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
    /// The marquee's crawl, live only while the setting is on and the
    /// line overflows.
    marquee: MarqueeScroll,
    /// The settings page's speed slider strip.
    speed_scrub: ScrubState,
    /// The settings page's end-rest delay slider strip.
    delay_scrub: ScrubState,
    /// The settings page's swap dwell slider strip.
    swap_scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl TrackInfoPanel {
    pub fn new(state: AppState, config: TrackInfoConfig, cx: &mut Context<Self>) -> Self {
        // The track line changes when the track does, not as it plays
        // through, so the gated observe skips the per-tick repaints.
        let _player_changed = observe_view(&state.player, cx);
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.meta = None;
                cx.notify();
            },
        );
        TrackInfoPanel {
            state,
            config,
            meta: None,
            marquee: MarqueeScroll::new(),
            speed_scrub: ScrubState::default(),
            delay_scrub: ScrubState::default(),
            swap_scrub: ScrubState::default(),
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

    /// Store the speed slider's fraction as a pace inside the range.
    fn set_marquee_speed(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.marquee_speed =
            MARQUEE_SPEED_MIN + fraction * (MARQUEE_SPEED_MAX - MARQUEE_SPEED_MIN);
        cx.notify();
    }

    /// Store the delay slider's fraction as seconds inside the range.
    fn set_marquee_delay(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.marquee_delay =
            MARQUEE_DELAY_MIN + fraction * (MARQUEE_DELAY_MAX - MARQUEE_DELAY_MIN);
        cx.notify();
    }

    /// Store the dwell slider's fraction as seconds inside the range.
    fn set_swap_secs(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.swap_secs = SWAP_SECS_MIN + fraction * (SWAP_SECS_MAX - SWAP_SECS_MIN);
        cx.notify();
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

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let speed = self
            .config
            .marquee_speed
            .clamp(MARQUEE_SPEED_MIN, MARQUEE_SPEED_MAX);
        let speed_fraction = (speed - MARQUEE_SPEED_MIN) / (MARQUEE_SPEED_MAX - MARQUEE_SPEED_MIN);
        let delay = self
            .config
            .marquee_delay
            .clamp(MARQUEE_DELAY_MIN, MARQUEE_DELAY_MAX);
        let delay_fraction = (delay - MARQUEE_DELAY_MIN) / (MARQUEE_DELAY_MAX - MARQUEE_DELAY_MIN);
        let dwell = self.config.swap_secs.clamp(SWAP_SECS_MIN, SWAP_SECS_MAX);
        let dwell_fraction = (dwell - SWAP_SECS_MIN) / (SWAP_SECS_MAX - SWAP_SECS_MIN);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    "Marquee",
                    Some("What a line too long for the panel does: crawl and return, or loop without end"),
                    panel::choices(
                        &[
                            ("Off", MarqueeMode::Off),
                            ("Scroll", MarqueeMode::Scroll),
                            ("Loop", MarqueeMode::Loop),
                        ],
                        self.config.marquee,
                        |this: &mut Self, mode, cx| {
                            this.config.marquee = mode;
                            this.marquee.reset();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.marquee != MarqueeMode::Off, |d| {
                    d.child(panel::setting_row(
                        "Speed",
                        Some("How fast the line crawls"),
                        panel::value_slider(
                            &self.speed_scrub,
                            speed_fraction,
                            format!("{speed:.0} px/s"),
                            Self::set_marquee_speed,
                            cx,
                        ),
                    ))
                })
                .when(self.config.marquee == MarqueeMode::Scroll, |d| {
                    d.child(panel::setting_row(
                        "Delay",
                        Some("How long the line rests at each end before moving again"),
                        panel::value_slider(
                            &self.delay_scrub,
                            delay_fraction,
                            format!("{delay:.1} s"),
                            Self::set_marquee_delay,
                            cx,
                        ),
                    ))
                })
                .child(panel::setting_row(
                    "Swap",
                    Some("Show one piece at a time - the title, then the artist - fading between them"),
                    panel::toggle(
                        self.config.swap,
                        |this: &mut Self, swap, cx| {
                            this.config.swap = swap;
                            this.marquee.reset();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.swap, |d| {
                    d.child(panel::setting_row(
                        "Swap every",
                        Some("How long each piece sits before the fade"),
                        panel::value_slider(
                            &self.swap_scrub,
                            dwell_fraction,
                            format!("{dwell:.0} s"),
                            Self::set_swap_secs,
                            cx,
                        ),
                    ))
                })
                .into_any_element(),
        )
    }
}

impl Render for TrackInfoPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl TrackInfoPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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

        // Mirror the configured rest before anything refills a hold this
        // frame.
        self.marquee.delay = self
            .config
            .marquee_delay
            .clamp(MARQUEE_DELAY_MIN, MARQUEE_DELAY_MAX);

        // A fresh track starts every cycle over: crawl home, swap back
        // to the heading.
        if self.marquee.path.as_deref() != Some(now.path.as_path()) {
            self.marquee.path = Some(now.path.clone());
            self.marquee.reset();
        }

        // The swap cycle picks which piece shows and how faded it sits;
        // the marquee below then treats that piece as the whole line.
        self.marquee.swap_live = self.config.swap && !byline.is_empty();
        let (heading, byline, fade) = if self.marquee.swap_live {
            let (on_byline, fade) = self.swap_cycle(window);
            if on_byline {
                (String::new(), byline, fade)
            } else {
                (heading, String::new(), fade)
            }
        } else {
            (heading, byline, 1.0)
        };

        // One line: the heading, the byline dimmed beside it, both giving
        // way gracefully when the panel runs out of room - unless a
        // marquee mode crawls what overflows instead.
        match self.config.marquee {
            MarqueeMode::Off => {}
            MarqueeMode::Scroll | MarqueeMode::Loop => {
                return root.child(self.marquee_line(heading, byline, ended, fade, window, cx));
            }
        }
        root.when(!heading.is_empty(), |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .max_w_full()
                    .truncate()
                    .when(fade < 1.0, |d| d.opacity(fade))
                    .child(heading),
            )
        })
        .when(!byline.is_empty(), |d| {
            d.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text_muted())
                    .when(fade < 1.0, |d| d.opacity(fade))
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

    /// Advance the swap cycle and hand back which piece shows (false for
    /// the heading, true for the byline) and how faded it sits. On a
    /// timer - in over the fade, full through the dwell, out over the
    /// fade, then the other piece - except under scroll mode, where the
    /// crawl runs the clock instead: a piece leaves once it has crawled
    /// out and rested, and the next one comes in back at the start. The
    /// cycle never settles, so it keeps its own frames running.
    fn swap_cycle(&mut self, window: &mut Window) -> (bool, f32) {
        window.request_animation_frame();
        let dwell = self.config.swap_secs.clamp(SWAP_SECS_MIN, SWAP_SECS_MAX);
        // Smoothstepped so the fades ease instead of snapping.
        let smooth = |u: f32| u * u * (3.0 - 2.0 * u);
        if self.config.marquee == MarqueeMode::Scroll {
            let t = self.marquee.swap_at.elapsed().as_secs_f32();
            // A piece that fits never crawls, so the dwell stands in for
            // the trip out.
            let overflow = f32::from(self.marquee.handle.max_offset().width);
            if overflow <= 0.0 && t >= SWAP_FADE_SECS + dwell {
                self.marquee.crawl_done = true;
            }
            if self.marquee.crawl_done && self.marquee.fade_at.is_none() {
                self.marquee.fade_at = Some(Instant::now());
            }
            if let Some(fade_at) = self.marquee.fade_at {
                let out = fade_at.elapsed().as_secs_f32();
                if out >= SWAP_FADE_SECS {
                    // Faded away: the other piece comes in at the start.
                    self.marquee.swap_ix = (self.marquee.swap_ix + 1) % 2;
                    self.marquee.swap_at = Instant::now();
                    self.marquee.rehome();
                    return (self.marquee.swap_ix % 2 == 1, 0.0);
                }
                return (
                    self.marquee.swap_ix % 2 == 1,
                    smooth(1.0 - out / SWAP_FADE_SECS),
                );
            }
            // Coming in, then full until the crawl hands over.
            return (
                self.marquee.swap_ix % 2 == 1,
                smooth((t / SWAP_FADE_SECS).min(1.0)),
            );
        }
        let cycle = SWAP_FADE_SECS + dwell + SWAP_FADE_SECS;
        let mut t = self.marquee.swap_at.elapsed().as_secs_f32();
        if t >= cycle {
            // The other piece comes in, crawling from home if it must.
            self.marquee.swap_ix = (self.marquee.swap_ix + 1) % 2;
            self.marquee.swap_at = Instant::now();
            self.marquee.rehome();
            t = 0.0;
        }
        let u = if t < SWAP_FADE_SECS {
            t / SWAP_FADE_SECS
        } else if t < SWAP_FADE_SECS + dwell {
            1.0
        } else {
            (1.0 - (t - SWAP_FADE_SECS - dwell) / SWAP_FADE_SECS).max(0.0)
        };
        (self.marquee.swap_ix % 2 == 1, smooth(u))
    }

    /// The crawling take on the track line, for the scroll and loop
    /// modes. The scroll box does the clipping and hands back the
    /// overflow off the last layout: scroll crawls out, rests, and
    /// crawls home again, while loop doubles the line and wraps the
    /// offset for an unbroken ticker.
    fn marquee_line(
        &mut self,
        heading: String,
        byline: String,
        ended: bool,
        fade: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let speed = self
            .config
            .marquee_speed
            .clamp(MARQUEE_SPEED_MIN, MARQUEE_SPEED_MAX);
        // Both come off the last layout and start at zero, so a fresh
        // panel sits still until it knows better.
        let container = f32::from(self.marquee.handle.bounds().size.width);
        let overflow = f32::from(self.marquee.handle.max_offset().width);
        let moving = if self.config.marquee == MarqueeMode::Loop {
            if self.marquee.looping {
                // The layout is doubled: peel the second copy and the
                // gap back off for the single line's width.
                let line = (overflow + container - MARQUEE_GAP) / 2.0;
                if line <= container + 0.5 {
                    // Room came back; one copy fits again.
                    self.marquee.reset();
                    false
                } else {
                    self.marquee.advance_loop(line + MARQUEE_GAP, speed);
                    true
                }
            } else if overflow > 0.0 {
                // One copy overflows: double up and start the wrap.
                self.marquee.looping = true;
                true
            } else {
                false
            }
        } else {
            self.marquee.looping = false;
            if overflow > 0.0 {
                // With the swap riding along, the crawl parks at the end
                // and hands over; the swap brings the next piece in back
                // at the start.
                self.marquee
                    .advance(overflow, speed, self.marquee.swap_live);
                true
            } else {
                if self.marquee.offset != 0.0 {
                    self.marquee.reset();
                }
                false
            }
        };
        if moving {
            window.request_animation_frame();
        }
        self.marquee
            .handle
            .set_offset(point(px(-self.marquee.offset), px(0.)));

        // No frames run while the line fits, so a resize that steals the
        // room would go unseen; the probe repaints with the panel and
        // wakes it whenever the overflow no longer matches the crawl.
        let handle = self.marquee.handle.clone();
        let entity_id = cx.entity_id();
        let probe = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if (handle.max_offset().width > px(0.)) != moving {
                    window.on_next_frame(move |_, cx| cx.notify(entity_id));
                }
            },
        )
        .absolute()
        .inset_0();

        // Loop mode shows the line twice, a gap apart, so the wrap lands
        // on an identical picture.
        let content = if self.marquee.looping {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(MARQUEE_GAP))
                .child(track_line_row(heading.clone(), byline.clone(), ended))
                .child(track_line_row(heading, byline, ended))
        } else {
            track_line_row(heading, byline, ended)
        };

        // min_w_0 lets the box shrink below its content in the panel's
        // row - without it the automatic minimum holds the box at the
        // full line's width. flex makes the box size its child row at
        // max-content - as a default block, the row would stretch to the
        // box instead. Either way lost, there is no overflow to crawl.
        div()
            .id("track-marquee")
            .flex()
            .min_w_0()
            .max_w_full()
            .overflow_x_scroll()
            .track_scroll(&self.marquee.handle)
            .when(fade < 1.0, |d| d.opacity(fade))
            .child(content)
            .child(probe)
    }
}

/// One copy of the track line: the heading with the byline dimmed
/// beside it, refusing to wrap, for the marquee's scroll box. Either
/// piece may be absent - the swap setting shows one at a time.
fn track_line_row(heading: String, byline: String, ended: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_SM)
        .whitespace_nowrap()
        .when(!heading.is_empty(), |d| d.child(heading))
        .when(!byline.is_empty(), |d| {
            d.child(div().text_color(palette::text_muted()).child(byline))
        })
        .when(ended, |d| {
            d.child(
                div()
                    .text_color(palette::text_muted())
                    .child("(queue finished)"),
            )
        })
}

/// The volume panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct VolumeConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
    /// Let the slider fill whatever width the panel has instead of capping
    /// at its natural size.
    #[serde(default)]
    pub stretch: bool,
    /// Collapse the panel to just the speaker icon: scroll it to change the
    /// volume, and the readout rides along in a tooltip.
    #[serde(default)]
    pub icon_only: bool,
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
        // Volume and mute are not on the pump at all; the gated observe
        // still catches changes from a keyboard shortcut or elsewhere.
        let _player_changed = observe_view(&state.player, cx);
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
        let stretch = cx.entity().downgrade();
        let icon_only = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Icon only")
                .checked(self.config.icon_only)
                .on_click(move |_, _, cx| {
                    let Some(this) = icon_only.upgrade() else {
                        return;
                    };
                    this.update(cx, |this, cx| {
                        this.config.icon_only = !this.config.icon_only;
                        cx.notify();
                    });
                }),
        )
        .item(
            PopupMenuItem::new("Stretch")
                .disabled(self.config.icon_only)
                .checked(self.config.stretch)
                .on_click(move |_, _, cx| {
                    let Some(this) = stretch.upgrade() else {
                        return;
                    };
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

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
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
                "Icon only",
                Some("Collapse to just the speaker icon; scroll it to change the volume"),
                panel::toggle(
                    self.config.icon_only,
                    |this: &mut Self, icon_only, cx| {
                        this.config.icon_only = icon_only;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Stretch",
                Some("Let the slider fill the panel instead of capping its width"),
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

/// One wheel step over the volume panel, shared by the full strip and the
/// icon-only speaker. A notch arrives as 3 lines, so one notch steps 5%; the
/// range is 0 to 100% and touching it unmutes.
fn volume_scroll(
    this: &mut VolumePanel,
    event: &gpui::ScrollWheelEvent,
    _window: &mut Window,
    cx: &mut Context<VolumePanel>,
) {
    let lines = match event.delta {
        gpui::ScrollDelta::Lines(lines) => lines.y,
        gpui::ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
    };
    this.state.player.update(cx, |player, cx| {
        let volume = (player.volume() + lines / 3.0 * 0.05).clamp(0.0, 1.0);
        player.set_volume(volume, cx);
    });
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
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

        // Icon-only: the speaker carries the whole panel. Click toggles mute,
        // scrolling nudges the volume, and the percent rides along in a
        // tooltip so the readout still has a home.
        if self.config.icon_only {
            let icon = div()
                .id("volume-icon")
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.state
                            .player
                            .update(cx, |player, cx| player.toggle_mute(cx));
                    }),
                )
                .tooltip(move |window, cx| Tooltip::new(format!("{percent}%")).build(window, cx))
                .child(svg().path(speaker).size(px(16.)).text_color(speaker_color));

            return div()
                .size_full()
                .bg(palette::bg_root())
                .flex()
                .items_center()
                .map(|d| justify(d, self.config.align))
                .px(tokens::SPACE_MD)
                .on_scroll_wheel(cx.listener(volume_scroll))
                .child(icon);
        }

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
            .on_scroll_wheel(cx.listener(volume_scroll))
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
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
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
}

fn default_true() -> bool {
    true
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            chrome: PanelChrome::default(),
            timings: true,
            show_total: false,
            scrobble_marker: false,
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
        // The clock and the playhead move every tick, so this one wants the
        // raw per-pump notify, not the gated observe the other panels ride.
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

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Clocks", icons::CLOCK)]
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
                "Timings",
                Some("The elapsed and ending clocks around the strip"),
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
                "Ending",
                Some("Count down the time left or show the full length"),
                panel::choices(
                    &[("Remaining", false), ("Total", true)],
                    self.config.show_total,
                    |this: &mut Self, show_total, cx| {
                        this.config.show_total = show_total;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Scrobble Marker",
                Some("A thin line where the track counts as scrobbled to last.fm"),
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

/// Tabular digits for the clock, built once - [`clock`] runs twice per
/// pump tick while playing, so the feature list should not reallocate
/// every call.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// A clock beside the strip: muted, fixed in the row, digits tabular so a
/// tick never changes the text width.
fn clock(text: String) -> Div {
    let mut clock = div().flex_none().text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(TNUM.clone());
    clock.child(text)
}

impl Render for SeekStripPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl SeekStripPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let now = self.state.player.read(cx).now_playing();

        // No frame polling: the raw observe in `new` re-renders the strip
        // on every pump tick while audio moves, which is the rate the clock
        // and playhead actually change at. A per-frame request on top only
        // redraws identical pixels - and kept the whole window repainting
        // at refresh rate through a paused session. Scrub drags notify on
        // their own through the mouse handlers.

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
        // The seek preview shows once the duration resolves; before that a
        // fraction maps to nothing.
        let hover_duration = now.duration_secs.filter(|d| *d > 0.0);
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let track = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
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
            )
            .when_some(hover_duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            });

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
    ($panel:ty, $name:literal, $title:literal, min_w = $min_w:literal) => {
        transport_panel!($panel, $name, $title, min_w = |_: &$panel| px($min_w));
    };
    ($panel:ty, $name:literal, $title:literal, min_w = $min_w:expr) => {
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
                panel::title_text(self.config.chrome.title.as_deref(), $title)
            }

            fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
                self.config
                    .chrome
                    .title
                    .clone()
                    .map(gpui::SharedString::from)
            }

            fn locked(&self, _cx: &App) -> bool {
                self.config.chrome.locked
            }

            fn inner_padding(&self, _cx: &App) -> bool {
                false
            }

            fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
                crate::panel::chrome_min_size(
                    &self.config.chrome,
                    gpui::size(($min_w)(self), rox_dock::resizable::PANEL_MIN_SIZE),
                )
            }

            fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
                crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
                let menu = panel_settings::rename_item(
                    menu,
                    &cx.entity(),
                    self.tab_panel.clone(),
                    _window,
                    cx,
                );
                let menu = panel_settings::settings_item(menu, &cx.entity());
                let menu = panel::duplicate_item(
                    menu,
                    &cx.entity(),
                    self.tab_panel.clone(),
                    |this, _window, cx| {
                        let (state, config) = {
                            let panel = this.read(cx);
                            (panel.state.clone(), panel.config.clone())
                        };
                        <$panel>::new(state, config, cx)
                    },
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

// The widths below are each panel's controls at their tightest: the seek
// strip's clocks around a usable track, and enough of the track info line to
// read a title. The playback row and the volume strip are fully composable
// now, so they lean on the app's own panel floor instead of pinning a width.
transport_panel!(
    TransportPanel,
    "playback",
    "Playback",
    min_w = |_: &TransportPanel| rox_dock::resizable::PANEL_MIN_SIZE
);
transport_panel!(
    VolumePanel,
    "volume",
    "Volume",
    min_w = |_: &VolumePanel| rox_dock::resizable::PANEL_MIN_SIZE
);
transport_panel!(SeekStripPanel, "seek", "Seek", min_w = 160.);
transport_panel!(TrackInfoPanel, "track info", "Track Info", min_w = 120.);
