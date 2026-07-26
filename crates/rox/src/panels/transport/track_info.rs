//! The track info readout panel: one line with the playing track's tags,
//! with the optional marquee crawl and piece swap for tight panels.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, Pixels, ScrollHandle, Subscription, WeakEntity, Window,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::store::TrackMeta;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, align_row, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::player::{fmt_time, observe_view};

use super::transport_panel;

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

// The width is enough of the track info line to read a title.
transport_panel!(TrackInfoPanel, "track info", "Track Info", min_w = 120.);
