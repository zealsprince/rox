//! The track info readout panel: the playing track's tags as ordered rows
//! of pieces. The stock arrangement is the classic one-liner (numbered
//! title and duration, the byline behind, the output chip at the trailing
//! edge); the arrange editor prunes, reorders, and breaks it into rows with
//! their own text sizes. The marquee crawl and the row cycle handle tight
//! panels.

use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Div, EntityId, EventEmitter, FocusHandle, Focusable, MouseButton,
    Pixels, Rgba, ScrollHandle, SharedString, Stateful, Subscription, WeakEntity, Window, canvas,
    div, point, prelude::*, px, rems, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::{Origin, TrackKey};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

use rox_services::thumbs::Thumb;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::group_head;
use crate::panel::{
    self, Align, AppState, PanelChrome, PanelSettings, ScrubState, align_row, justify,
};
use crate::panel_settings;
use crate::player::{fmt_time, observe_view};
use crate::settings::ui as settings_ui;

use super::transport_panel;

/// The text pieces compose into crawlable runs; the chip, art, spacer,
/// and divider hold their own shape.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InfoPiece {
    /// Zero-padded the way the classic line writes it.
    TrackNo,
    /// Or the file name for a track the library doesn't know.
    Title,
    /// In parens: the classic line's "(2:17)".
    Duration,
    Artist,
    Album,
    Year,
    Genre,
    /// Codec, stream shape, and bitrate, from [`group_head::quality`].
    Quality,
    /// Off the explicit queue; empty during plain context playback.
    Next,
    /// As "N queued".
    Queued,
    /// Claims its width first and never crawls.
    Output,
    Favourite,
    Rating,
    Art,
    /// A flexible gap; a row holds as many as the layout needs.
    Spacer,
    /// A spacer with a hairline across its gap.
    Divider,
    Break,
}

/// Stock order: where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<InfoPiece>] = &[
    panel::ArrangeSpec {
        key: "info-item-track-no",
        icon: Some(icons::LIST_MUSIC),
        value: InfoPiece::TrackNo,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-title",
        icon: Some(icons::MUSIC),
        value: InfoPiece::Title,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-duration",
        icon: Some(icons::CLOCK),
        value: InfoPiece::Duration,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-artist",
        icon: Some(icons::MIC),
        value: InfoPiece::Artist,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-album",
        icon: Some(icons::DISC),
        value: InfoPiece::Album,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-year",
        icon: Some(icons::CALENDAR),
        value: InfoPiece::Year,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-genre",
        icon: Some(icons::TAG),
        value: InfoPiece::Genre,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-quality",
        icon: Some(icons::AUDIO_WAVEFORM),
        value: InfoPiece::Quality,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-next",
        icon: Some(icons::SKIP_FORWARD),
        value: InfoPiece::Next,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-queued",
        icon: Some(icons::LAYERS),
        value: InfoPiece::Queued,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-output",
        icon: Some(icons::VOLUME_2),
        value: InfoPiece::Output,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-favourite",
        icon: Some(icons::HEART),
        value: InfoPiece::Favourite,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "info-item-rating",
        icon: Some(icons::STAR),
        value: InfoPiece::Rating,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-art",
        icon: Some(icons::IMAGE),
        value: InfoPiece::Art,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: InfoPiece::Spacer,
        repeats: true,
    },
    panel::ArrangeSpec {
        key: "head-piece-divider",
        icon: Some(icons::MINUS),
        value: InfoPiece::Divider,
        repeats: true,
    },
];

/// The spacers go where the retired align knob put the text, so a layout
/// from before the list keeps its look; `chip` off is the retired
/// toggle's read.
fn stock_items(align: Align, chip: bool) -> Vec<InfoPiece> {
    let mut items = vec![
        InfoPiece::TrackNo,
        InfoPiece::Title,
        InfoPiece::Duration,
        InfoPiece::Artist,
        InfoPiece::Album,
    ];
    if chip {
        if align != Align::Left {
            items.insert(0, InfoPiece::Spacer);
        }
        if align != Align::Right {
            items.push(InfoPiece::Spacer);
        }
        items.push(InfoPiece::Output);
    }
    items
}

/// Reads through [`TrackInfoConfigDump`] so layouts from before the
/// ordered list still load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "TrackInfoConfigDump")]
pub struct TrackInfoConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
    #[serde(default)]
    pub marquee: MarqueeMode,
    /// Pixels per second, for scroll and loop.
    #[serde(default = "default_marquee_speed")]
    pub marquee_speed: f32,
    /// Seconds the scroll rests at each end.
    #[serde(default = "default_marquee_delay")]
    pub marquee_delay: f32,
    /// Cycle the rows through a single line with a fade between turns. A
    /// single-row arrangement reads as itself; the shown row still crawls if
    /// it overflows.
    #[serde(default)]
    pub swap: bool,
    /// Seconds each row stays fully shown before the fade.
    #[serde(default = "default_swap_secs")]
    pub swap_secs: f32,
    /// Off holds the muted color whatever the state, for a transport line
    /// that needs one flat tone; the hover note still explains.
    #[serde(default = "default_output_tint")]
    pub output_tint: bool,
    /// Display order; one not listed is hidden.
    pub items: Vec<InfoPiece>,
    /// Per-row multiplier over the panel's base, indexed like the editor's
    /// rows; a row past the end reads 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scales: Vec<f32>,
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
            output_tint: default_output_tint(),
            items: stock_items(Align::default(), true),
            scales: Vec::new(),
        }
    }
}

/// Newer layouts write the ordered list. Older ones had a `show_output`
/// toggle for the chip.
#[derive(Deserialize)]
struct TrackInfoConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    align: Align,
    #[serde(default)]
    marquee: MarqueeMode,
    #[serde(default = "default_marquee_speed")]
    marquee_speed: f32,
    #[serde(default = "default_marquee_delay")]
    marquee_delay: f32,
    #[serde(default)]
    swap: bool,
    #[serde(default = "default_swap_secs")]
    swap_secs: f32,
    #[serde(default = "default_show_output")]
    show_output: bool,
    #[serde(default = "default_output_tint")]
    output_tint: bool,
    #[serde(default)]
    items: Option<Vec<InfoPiece>>,
    #[serde(default)]
    scales: Vec<f32>,
}

impl From<TrackInfoConfigDump> for TrackInfoConfig {
    fn from(dump: TrackInfoConfigDump) -> Self {
        let items = match dump.items {
            // Deduped per row: the catalog has no break (it's the editor's row
            // boundary), and each row may hold its own copy of a piece.
            Some(items) => items
                .split(|i| matches!(i, InfoPiece::Break))
                .map(|row| panel::dedup(ITEMS, row.to_vec()))
                .collect::<Vec<_>>()
                .join(&InfoPiece::Break),
            // The retired panel swapped heading against byline, so the fold splits
            // the classic line into those two rows, the chip on both so it never
            // blinks out.
            None if dump.swap => {
                let mut rows = vec![
                    vec![InfoPiece::TrackNo, InfoPiece::Title, InfoPiece::Duration],
                    vec![InfoPiece::Artist, InfoPiece::Album],
                ];
                if dump.show_output {
                    for row in &mut rows {
                        row.push(InfoPiece::Spacer);
                        row.push(InfoPiece::Output);
                    }
                }
                rows.join(&InfoPiece::Break)
            }
            None => stock_items(dump.align, dump.show_output),
        };
        TrackInfoConfig {
            chrome: dump.chrome,
            align: dump.align,
            marquee: dump.marquee,
            marquee_speed: dump.marquee_speed,
            marquee_delay: dump.marquee_delay,
            swap: dump.swap,
            swap_secs: dump.swap_secs,
            output_tint: dump.output_tint,
            items,
            scales: dump.scales,
        }
    }
}

/// Only built for the two states that earn a color, so a plain chip has
/// no tooltip.
struct OutputTooltip(SharedString);

impl Render for OutputTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(tokens::SPACE_SM)
            .max_w(px(320.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_xs()
            .text_color(palette::text())
            .child(self.0.clone())
    }
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarqueeMode {
    #[default]
    Off,
    /// Crawl to the end, rest, crawl back, rest.
    Scroll,
    /// Crawl one way without end.
    Loop,
}

const MARQUEE_SPEED_MIN: f32 = 10.0;
const MARQUEE_SPEED_MAX: f32 = 120.0;

fn default_marquee_speed() -> f32 {
    30.0
}

const SWAP_SECS_MIN: f32 = 1.0;
const SWAP_SECS_MAX: f32 = 15.0;

fn default_swap_secs() -> f32 {
    4.0
}

const MARQUEE_DELAY_MIN: f32 = 0.0;
const MARQUEE_DELAY_MAX: f32 = 10.0;

/// Legacy-only; new layouts list the chip as a piece.
fn default_show_output() -> bool {
    true
}

fn default_output_tint() -> bool {
    true
}

fn default_marquee_delay() -> f32 {
    2.0
}
/// The breather between a tail and the next head in loop mode.
const MARQUEE_GAP: f32 = 48.0;
const SWAP_FADE_SECS: f32 = 0.4;

const ROW_SCALE_MIN: f32 = 0.5;
const ROW_SCALE_MAX: f32 = 3.0;

/// The scroll handle owns the clipping and reports the overflow off the
/// last layout; the rest drives the offset one leg at a time.
struct MarqueeScroll {
    handle: ScrollHandle,
    /// Pixels left of home.
    offset: f32,
    /// The scroll crawl's direction: 1 heading out, -1 heading home.
    dir: f32,
    /// Time left resting at an end before the next leg starts.
    hold: f32,
    /// Mirrored off the config each frame so the crawl can refill `hold`
    /// itself.
    delay: f32,
    last_tick: Instant,
    /// Whether one copy alone overflows, so the line renders doubled and
    /// wraps.
    looping: bool,
    /// Set by the body. Under the cycle the crawl parks at the end instead
    /// of bouncing back.
    cycling: bool,
    /// Scroll mode's handshake: the trip out is done and the cycle may fade
    /// the row.
    crawl_done: bool,
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
            looping: false,
            cycling: false,
            crawl_done: false,
        }
    }

    fn reset(&mut self) {
        self.offset = 0.0;
        self.dir = 1.0;
        self.hold = self.delay;
        self.last_tick = Instant::now();
        self.looping = false;
        self.crawl_done = false;
    }

    /// Without `park` it turns around at each end. With it (the row cycle
    /// driving) it stays put once out and rested and raises `crawl_done`. The
    /// step clamps so a stalled frame never teleports the line.
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

    /// Wraps once a full copy and its gap have gone by, so the doubled line
    /// reads as one loop.
    fn advance_loop(&mut self, period: f32, speed: f32) {
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.1);
        self.last_tick = Instant::now();
        self.offset += speed * dt;
        if self.offset >= period {
            self.offset -= period;
        }
    }
}

struct RowCycle {
    ix: usize,
    at: Instant,
    fade_at: Option<Instant>,
}

impl RowCycle {
    fn new() -> Self {
        RowCycle {
            ix: 0,
            at: Instant::now(),
            fade_at: None,
        }
    }

    fn reset(&mut self) {
        *self = RowCycle::new();
    }
}

/// Each None when its field is empty, so the piece drops out of the line.
struct PieceTexts {
    trackno: Option<String>,
    title: Option<String>,
    duration: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    year: Option<String>,
    genre: Option<String>,
    quality: Option<String>,
    next: Option<String>,
    queued: Option<String>,
}

enum RowBit {
    /// Segments of text and whether each is muted.
    Run(Vec<(String, bool)>),
    Fixed(InfoPiece),
    /// A station's or a server's glyph, outside the crawl so it holds its
    /// place while the line scrolls.
    Glyph(&'static str),
}

/// Same-color neighbors read as one phrase: bright ones join with a space,
/// "05. Title (2:17)", muted ones with the byline's " - ". An empty field
/// drops out.
///
/// `glyph` names the piece whose row wears the source mark, None for a
/// local track. The mark goes after that row's last run: a crawl scrolls a
/// run as one box, so a mark mid-line would split it into two runs
/// crawling on their own clocks.
fn row_bits(
    pieces: &[InfoPiece],
    texts: &PieceTexts,
    glyph: Option<(InfoPiece, &'static str)>,
) -> Vec<RowBit> {
    let mut bits = Vec::new();
    let mut run: Vec<(String, bool)> = Vec::new();
    // Only the row with the marked piece gets one.
    let mut marked = false;
    for piece in pieces {
        let text = match piece {
            InfoPiece::TrackNo => texts.trackno.clone().map(|t| (t, false)),
            InfoPiece::Title => texts.title.clone().map(|t| (t, false)),
            InfoPiece::Duration => texts.duration.clone().map(|t| (t, false)),
            InfoPiece::Artist => texts.artist.clone().map(|t| (t, true)),
            InfoPiece::Album => texts.album.clone().map(|t| (t, true)),
            InfoPiece::Year => texts.year.clone().map(|t| (t, true)),
            InfoPiece::Genre => texts.genre.clone().map(|t| (t, true)),
            InfoPiece::Quality => texts.quality.clone().map(|t| (t, true)),
            InfoPiece::Next => texts.next.clone().map(|t| (t, true)),
            InfoPiece::Queued => texts.queued.clone().map(|t| (t, true)),
            InfoPiece::Output
            | InfoPiece::Favourite
            | InfoPiece::Rating
            | InfoPiece::Art
            | InfoPiece::Spacer
            | InfoPiece::Divider => {
                if !run.is_empty() {
                    bits.push(RowBit::Run(std::mem::take(&mut run)));
                }
                bits.push(RowBit::Fixed(*piece));
                continue;
            }
            // Rows come pre-split; a break never gets here.
            InfoPiece::Break => continue,
        };
        let Some((text, muted)) = text else { continue };

        if glyph.is_some_and(|(lead, _)| lead == *piece) {
            marked = true;
        }

        match run.last_mut() {
            Some((run_text, run_muted)) if *run_muted == muted => {
                run_text.push_str(if muted { " - " } else { " " });
                run_text.push_str(&text);
            }
            _ => run.push((text, muted)),
        }
    }
    if !run.is_empty() {
        bits.push(RowBit::Run(run));
    }

    // Behind the last run, so anything after the text (the chip, a spacer)
    // keeps its place at the far edge.
    if let Some((_, path)) = glyph
        && marked
        && let Some(at) = bits.iter().rposition(|bit| matches!(bit, RowBit::Run(_)))
    {
        bits.insert(at + 1, RowBit::Glyph(path));
    }

    bits
}

/// Empty rows kept, so the per-row scales stay indexed like the editor's.
fn editor_rows(items: &[InfoPiece]) -> Vec<Vec<InfoPiece>> {
    items
        .split(|i| matches!(i, InfoPiece::Break))
        .map(|row| row.to_vec())
        .collect()
}

/// The session errors and the idle message stand in while nothing shows.
pub struct TrackInfoPanel {
    state: AppState,
    config: TrackInfoConfig,
    /// None for a file the library doesn't know. Cached because the pump
    /// notifies every frame and the lookup is a query. The station-title
    /// revision rides along, so a stream's turnover re-resolves like a track
    /// change.
    meta: Option<(TrackKey, u64, Option<rox_library::store::TrackMeta>)>,
    /// Keyed on the queue revision, so the snapshot pass and the lookup only
    /// rerun when the queue moves.
    queue_info: Option<(u64, usize, Option<String>)>,
    /// Cached like the tags; cleared when the catalog or the playlists move.
    favourite: Option<(TrackKey, Option<i64>, bool)>,
    /// One per text run, in row order; rebuilt when the arrangement changes
    /// shape.
    marquees: Vec<MarqueeScroll>,
    cycle: RowCycle,
    /// A track change starts the crawls over.
    marquee_key: Option<TrackKey>,
    speed_scrub: ScrubState,
    delay_scrub: ScrubState,
    swap_scrub: ScrubState,
    /// Grown to the row count as the page builds.
    scale_scrubs: Vec<ScrubState>,
    value_edit: panel::ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

impl TrackInfoPanel {
    pub fn new(state: AppState, config: TrackInfoConfig, cx: &mut Context<Self>) -> Self {
        // The line changes with the track, not as it plays, so the gated observe
        // skips per-tick repaints.
        let _player_changed = observe_view(&state.player, cx);
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                // A favourites toggle anywhere moves the heart; the tags stand.
                if matches!(event, LibraryEvent::PlaylistsChanged) {
                    this.favourite = None;
                    cx.notify();
                    return;
                }
                // A star click moves the rating, which the tags cache holds.
                if matches!(event, LibraryEvent::Rated) {
                    this.meta = None;
                    cx.notify();
                    return;
                }
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.meta = None;
                this.queue_info = None;
                this.favourite = None;
                cx.notify();
            },
        );
        TrackInfoPanel {
            state,
            config,
            meta: None,
            queue_info: None,
            favourite: None,
            marquees: Vec::new(),
            cycle: RowCycle::new(),
            marquee_key: None,
            speed_scrub: ScrubState::default(),
            delay_scrub: ScrubState::default(),
            swap_scrub: ScrubState::default(),
            scale_scrubs: Vec::new(),
            value_edit: panel::ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _library_changed,
        }
    }

    /// The one knob worth flipping without opening settings: a context menu
    /// with every knob is a worse settings page.
    ///
    /// Flat checked items rather than a submenu: a nested `.checked()` shows a
    /// stale tick until it's reopened.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Copy reads the track when clicked, not when the menu opens, so a
        // track change under an open menu copies what's playing.
        let state = self.state.clone();
        let menu = panel::copy_submenu(
            menu,
            window,
            cx,
            Rc::new(move |cx: &App| panel::CopyText::playing(&state, cx).into_iter().collect()),
        );
        let mut menu = menu
            .separator()
            .label(rox_i18n::t!("track-info-menu-overflow"));
        for (name, mode) in [
            (
                rox_i18n::t!("track-info-overflow-truncate"),
                MarqueeMode::Off,
            ),
            (
                rox_i18n::t!("track-info-overflow-scroll"),
                MarqueeMode::Scroll,
            ),
            (rox_i18n::t!("track-info-overflow-loop"), MarqueeMode::Loop),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.marquee == mode)
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.marquee = mode;
                            // The crawls' offsets mean nothing to the arriving mode.
                            this.reset_marquees();
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    fn reset_marquees(&mut self) {
        for marquee in &mut self.marquees {
            marquee.reset();
        }
        self.cycle.reset();
    }

    /// Fade in, hold, fade out, then the next row comes in with its crawls
    /// home. Under scroll mode the crawls run the clock: every overflowing run
    /// crawls out and rests before the fade-out. Returns the shown row's index
    /// and fade. The cycle never settles, so it keeps its own frames running.
    fn advance_cycle(
        &mut self,
        mode: MarqueeMode,
        dwell: f32,
        row_runs: &[std::ops::Range<usize>],
        window: &mut Window,
    ) -> (usize, f32) {
        window.request_animation_frame();
        let smooth = |u: f32| u * u * (3.0 - 2.0 * u);
        // An arrangement edit can shrink the row list under a standing cycle.
        if self.cycle.ix >= row_runs.len() {
            self.cycle.reset();
        }
        let t = self.cycle.at.elapsed().as_secs_f32();
        let done = if mode == MarqueeMode::Scroll {
            let dwelled = t >= SWAP_FADE_SECS + dwell;
            self.marquees[row_runs[self.cycle.ix].clone()]
                .iter()
                .all(|marquee| {
                    if f32::from(marquee.handle.max_offset().width) <= 0.0 {
                        dwelled
                    } else {
                        marquee.crawl_done
                    }
                })
        } else {
            t >= SWAP_FADE_SECS + dwell
        };
        if done && self.cycle.fade_at.is_none() {
            self.cycle.fade_at = Some(Instant::now());
        }
        if let Some(fade_at) = self.cycle.fade_at {
            let out = fade_at.elapsed().as_secs_f32();
            if out >= SWAP_FADE_SECS {
                // Faded away: the next row comes in, its crawls at home.
                self.cycle.ix = (self.cycle.ix + 1) % row_runs.len();
                self.cycle.at = Instant::now();
                self.cycle.fade_at = None;
                for marquee in &mut self.marquees[row_runs[self.cycle.ix].clone()] {
                    marquee.reset();
                }
                return (self.cycle.ix, 0.0);
            }
            return (self.cycle.ix, smooth(1.0 - out / SWAP_FADE_SECS));
        }
        (self.cycle.ix, smooth((t / SWAP_FADE_SECS).min(1.0)))
    }

    fn set_marquee_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.config.marquee_speed = speed;
        cx.notify();
    }

    fn set_marquee_delay(&mut self, delay: f32, cx: &mut Context<Self>) {
        self.config.marquee_delay = delay;
        cx.notify();
    }

    fn set_swap_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        self.config.swap_secs = secs;
        cx.notify();
    }

    /// Keyed on the whole track, so two cue tracks of one image don't both
    /// draw whichever the library sorts first.
    fn meta_for(&mut self, key: &TrackKey, cx: &App) -> Option<&rox_library::store::TrackMeta> {
        // A station keeps one key for hours, so the title revision is part of
        // staleness; the lookup runs once per song.
        let live_rev = self.state.player.read(cx).title_rev().unwrap_or(0);
        let stale = match self.meta.as_ref() {
            Some((cached, rev, _)) => cached != key || *rev != live_rev,
            None => true,
        };

        if stale {
            let row = self.state.library.read(cx).meta_for_key(key);
            let meta = self.state.player.read(cx).live_over(row);
            self.meta = Some((key.clone(), live_rev, meta));
        }

        self.meta.as_ref().and_then(|(.., meta)| meta.as_ref())
    }

    fn queue_info(&mut self, cx: &App) -> (usize, Option<String>) {
        let player = self.state.player.read(cx);
        let rev = player.queue_rev().unwrap_or(0);
        if self.queue_info.as_ref().map(|(r, ..)| *r) != Some(rev) {
            let queued = player.queued();
            let next = queued.first().map(|entry| {
                let key = player.key_for(entry);
                let meta = self.state.library.read(cx).meta_for_key(&key);
                match meta {
                    Some(meta) if !meta.title.is_empty() && !meta.artist.is_empty() => {
                        format!("{} - {}", meta.title, meta.artist)
                    }
                    Some(meta) if !meta.title.is_empty() => meta.title,
                    // A file the library doesn't know still names itself.
                    _ => key
                        .path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| key.path.display().to_string()),
                }
            });
            self.queue_info = Some((rev, queued.len(), next));
        }
        let (_, count, next) = self.queue_info.as_ref().unwrap();
        (*count, next.clone())
    }

    fn favourite_for(&mut self, key: &TrackKey, cx: &App) -> (Option<i64>, bool) {
        if self.favourite.as_ref().map(|(k, ..)| k) != Some(key) {
            let library = self.state.library.read(cx);
            let id = library.id_for_key(key);
            let on = id.is_some_and(|id| library.is_favourite(id));
            self.favourite = Some((key.clone(), id, on));
        }
        self.favourite
            .as_ref()
            .map_or((None, false), |(_, id, on)| (*id, *on))
    }

    /// A click runs the favourite panel's toggle. Scaled with its row, so a
    /// title-row heart holds the line.
    fn favourite_heart(
        &self,
        id: Option<i64>,
        on: bool,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tip = match (id.is_some(), on) {
            (false, _) => rox_i18n::t!("transport-favourite-nothing"),
            (true, true) => rox_i18n::t!("transport-favourite-remove"),
            (true, false) => rox_i18n::t!("transport-favourite-add"),
        };
        panel::Tip::keyed("favourite", tip)
            .apply(
                div()
                    .flex_none()
                    .size(palette::scaled_px(24.) * scale)
                    .rounded(tokens::RADIUS)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(if on {
                                icons::HEART_FILLED
                            } else {
                                icons::HEART
                            })
                            .size(palette::scaled_px(15.) * scale)
                            .text_color(if on {
                                palette::accent()
                            } else {
                                palette::text_faint()
                            }),
                    )
                    // Stays up dimmed so the piece holds its place in the row.
                    .when(id.is_none(), |d| d.opacity(0.4))
                    .when_some(id, |d, id| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(palette::bg_control_hover()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Self, _, _, cx| {
                                    this.state.library.update(cx, |library, cx| {
                                        library.set_favourites(&[id], !on, cx)
                                    });
                                }),
                            )
                    }),
            )
            .into_any_element()
    }

    /// The stars keep their stock size whatever the row's scale, like every
    /// other rating surface.
    fn rating_stars(&self, id: Option<i64>, value: u8, _cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.clone();
        // Keyed by the shown track so the hover preview matches every other
        // surface rating it.
        let key = id.unwrap_or(-1) as u64;
        let control = crate::rating_ui::control(key, value, move |rating, _, cx| {
            let Some(id) = id else { return };
            state
                .library
                .update(cx, |library, cx| library.rate(id, rating, cx));
        });
        div()
            .flex_none()
            .flex()
            .items_center()
            // Stays up dimmed so the piece holds its place in the row.
            .when(id.is_none(), |d| d.opacity(0.4))
            .child(control)
            .into_any_element()
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
        let rows = editor_rows(&self.config.items);
        self.scale_scrubs
            .resize_with(rows.len(), ScrubState::default);
        let editor = panel::arrange_rows_editor(
            "track-info-items",
            ITEMS,
            &rows,
            None,
            |this: &mut Self, rows, cx| {
                this.config.items = rows.join(&InfoPiece::Break);
                cx.notify();
            },
            cx,
        );
        // A single-row line just calls it the text size.
        let sizes: Vec<AnyElement> = (0..rows.len())
            .map(|ix| {
                let label = if rows.len() == 1 {
                    rox_i18n::t!("track-info-text-size").to_string()
                } else {
                    rox_i18n::t!("track-info-row-size", number = (ix + 1) as u64).to_string()
                };
                let scale = self.config.scales.get(ix).copied().unwrap_or(1.0).clamp(
                    ROW_SCALE_MIN,
                    settings_ui::ceiling(ROW_SCALE_MIN, ROW_SCALE_MAX),
                );
                panel::setting_row_dyn(
                    label,
                    None,
                    settings_ui::scalar(
                        &self.scale_scrubs[ix],
                        &self.value_edit,
                        scale,
                        settings_ui::span(ROW_SCALE_MIN, ROW_SCALE_MAX, "x").decimals(2),
                        move |this: &mut Self, scale, cx| {
                            if this.config.scales.len() <= ix {
                                this.config.scales.resize(ix + 1, 1.0);
                            }
                            this.config.scales[ix] = scale;
                            cx.notify();
                        },
                        cx,
                    ),
                )
                .into_any_element()
            })
            .collect();
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
            .child(panel::setting_block(
                rox_i18n::t!("transport-pieces"),
                Some(rox_i18n::t!("transport-pieces.description")),
                None,
                editor,
            ))
            .children(sizes)
            .into_any_element()
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let speed = self.config.marquee_speed.clamp(
            MARQUEE_SPEED_MIN,
            settings_ui::ceiling(MARQUEE_SPEED_MIN, MARQUEE_SPEED_MAX),
        );
        let delay = self.config.marquee_delay.clamp(
            MARQUEE_DELAY_MIN,
            settings_ui::ceiling(MARQUEE_DELAY_MIN, MARQUEE_DELAY_MAX),
        );
        let dwell = self.config.swap_secs.clamp(
            SWAP_SECS_MIN,
            settings_ui::ceiling(SWAP_SECS_MIN, SWAP_SECS_MAX),
        );
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .when(self.config.items.contains(&InfoPiece::Output), |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("track-info-color-output-chip"),
                        Some(rox_i18n::t!("track-info-color-output-chip.description")),
                        panel::toggle(
                            self.config.output_tint,
                            |this: &mut Self, on, cx| {
                                this.config.output_tint = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                })
                .child(panel::setting_row(
                    rox_i18n::t!("track-info-marquee"),
                    Some(rox_i18n::t!("track-info-marquee.description")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("panel-size-off"), MarqueeMode::Off),
                            (
                                rox_i18n::t!("track-info-overflow-scroll"),
                                MarqueeMode::Scroll,
                            ),
                            (rox_i18n::t!("track-info-overflow-loop"), MarqueeMode::Loop),
                        ],
                        self.config.marquee,
                        |this: &mut Self, mode, cx| {
                            this.config.marquee = mode;
                            this.reset_marquees();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.marquee != MarqueeMode::Off, |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("track-info-speed"),
                        Some(rox_i18n::t!("track-info-speed.description")),
                        settings_ui::scalar(
                            &self.speed_scrub,
                            &self.value_edit,
                            speed,
                            settings_ui::span(MARQUEE_SPEED_MIN, MARQUEE_SPEED_MAX, " px/s"),
                            Self::set_marquee_speed,
                            cx,
                        ),
                    ))
                })
                .when(self.config.marquee == MarqueeMode::Scroll, |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("track-info-delay"),
                        Some(rox_i18n::t!("track-info-delay.description")),
                        settings_ui::scalar(
                            &self.delay_scrub,
                            &self.value_edit,
                            delay,
                            settings_ui::span(MARQUEE_DELAY_MIN, MARQUEE_DELAY_MAX, " s")
                                .decimals(1),
                            Self::set_marquee_delay,
                            cx,
                        ),
                    ))
                })
                .child(panel::setting_row(
                    rox_i18n::t!("track-info-cycle-rows"),
                    Some(rox_i18n::t!("track-info-cycle-rows.description")),
                    panel::toggle(
                        self.config.swap,
                        |this: &mut Self, swap, cx| {
                            this.config.swap = swap;
                            this.reset_marquees();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.swap, |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("track-info-cycle-every"),
                        Some(rox_i18n::t!("track-info-cycle-every.description")),
                        settings_ui::scalar(
                            &self.swap_scrub,
                            &self.value_edit,
                            dwell,
                            settings_ui::span(SWAP_SECS_MIN, SWAP_SECS_MAX, " s"),
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
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl TrackInfoPanel {
    /// Muted while nothing converts, the banner's tone colors when something
    /// does, or a muted alert face with the tint off. None before a stream
    /// negotiates. `ix` keeps two chips apart for gpui.
    ///
    /// `live` only changes the tooltip's wording: "this file" reads wrong
    /// while someone listens to radio.
    fn output_chip(&self, ix: usize, live: bool, cx: &App) -> Option<Stateful<Div>> {
        let status = self.state.player.read(cx).output_status()?;
        let negotiated = &status.negotiated;
        let exclusive = negotiated.mode == rox_playback::output::Mode::Exclusive;
        let resampling = status
            .source_rate
            .is_some_and(|source| source != negotiated.sample_rate);
        // Shared output is normal and colors nothing: a chip that's always lit
        // stops being a signal. The two states worth flagging are a refused mode
        // and an unneeded conversion.
        let (color, why): (Rgba, Option<SharedString>) = if let Some(reason) = &negotiated.fallback
        {
            (
                palette::tone_bad(),
                Some(rox_i18n::t!(
                    "track-info-output-fallback",
                    reason = reason.clone()
                )),
            )
        } else if resampling {
            let source = group_head::khz(status.source_rate.unwrap_or_default());
            let device = group_head::khz(negotiated.sample_rate);
            // Exclusive resamples too when the card won't take the file's rate, and
            // that's worth saying: the claim went through and it still isn't the
            // file's own samples. A station gets the same two sentences about a
            // stream.
            (
                palette::tone_warn(),
                Some(match (exclusive, live) {
                    (true, false) => rox_i18n::t!(
                        "track-info-output-resample-exclusive",
                        source = source,
                        device = device
                    ),
                    (false, false) => rox_i18n::t!(
                        "track-info-output-resample-mixer",
                        source = source,
                        device = device
                    ),
                    (true, true) => rox_i18n::t!(
                        "track-info-output-resample-exclusive-stream",
                        source = source,
                        device = device
                    ),
                    (false, true) => rox_i18n::t!(
                        "track-info-output-resample-mixer-stream",
                        source = source,
                        device = device
                    ),
                }),
            )
        } else {
            (palette::text_muted(), None)
        };
        // The face stands in for the tint and only shows in the two flagged
        // states once the chip is flat.
        let face = why.is_some() && !self.config.output_tint;
        let color = if self.config.output_tint {
            color
        } else {
            palette::text_muted()
        };
        // "Shared" is every desktop's default and goes unsaid; "Exclusive" is
        // the state someone went looking for. The rate goes through the library
        // column's speller.
        let label = format!(
            "{}{} kHz {}",
            if exclusive { "Exclusive " } else { "" },
            group_head::khz(negotiated.sample_rate),
            negotiated.format
        );
        Some(
            div()
                .id(("output-chip", ix))
                // The chip claims its width first and the line crawls in what's left,
                // so it never moves with the marquee.
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .px(tokens::SPACE_SM)
                .rounded(tokens::RADIUS)
                .bg(palette::bg_control())
                .text_xs()
                .text_color(color)
                .child(label)
                .when(face, |d| {
                    d.child(svg().path(icons::ALERT).size_3().text_color(color))
                })
                .when_some(why, |d, why| {
                    d.tooltip(move |_, cx| cx.new(|_| OutputTooltip(why.clone())).into())
                }),
        )
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let items = self.config.items.clone();
        let align = self.config.align;
        let mode = self.config.marquee;
        let swap = self.config.swap;
        let scales = self.config.scales.clone();
        let speed = self.config.marquee_speed.clamp(
            MARQUEE_SPEED_MIN,
            settings_ui::ceiling(MARQUEE_SPEED_MIN, MARQUEE_SPEED_MAX),
        );
        let delay = self.config.marquee_delay.clamp(
            MARQUEE_DELAY_MIN,
            settings_ui::ceiling(MARQUEE_DELAY_MIN, MARQUEE_DELAY_MAX),
        );
        let dwell = self.config.swap_secs.clamp(
            SWAP_SECS_MIN,
            settings_ui::ceiling(SWAP_SECS_MIN, SWAP_SECS_MAX),
        );

        let (now, active, ended, error) = {
            let player = self.state.player.read(cx);
            (
                player.now_playing(),
                player.is_active(),
                player.queue_ended(),
                player.error(),
            )
        };

        let shell = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .justify_center();

        let Some(now) = now else {
            // A session still opening, or the reason there's nothing to hear. Plain
            // idle stays blank. A reason outranks the wait: a session whose every
            // entry was refused would otherwise sit on "opening..." with the only
            // account in the log.
            let line: Option<SharedString> = match error {
                Some(error) => Some(error),
                None if active => Some(rox_i18n::t!("track-info-opening")),
                None => None,
            };
            let chip = items
                .contains(&InfoPiece::Output)
                .then(|| self.output_chip(0, false, cx))
                .flatten();
            return shell.child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .px(tokens::SPACE_MD)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .map(|d| justify(d, align))
                            .when_some(line, |d, line| {
                                d.child(
                                    div()
                                        .max_w_full()
                                        .truncate()
                                        .text_color(palette::text_muted())
                                        .child(line),
                                )
                            }),
                    )
                    .when_some(chip, |d, chip| d.child(chip)),
            );
        };

        // A fresh track starts every cycle over.
        if self.marquee_key.as_ref() != Some(&now.key) {
            self.marquee_key = Some(now.key.clone());
            self.reset_marquees();
        }

        // The texts own their strings so the crawl states below can borrow
        // freely.
        let meta = self.meta_for(&now.key, cx);
        let rating_value = meta.map(|m| m.rating).unwrap_or(0);
        let title = meta.map(|m| m.title.clone()).unwrap_or_default();
        let title = if title.is_empty() {
            now.path()
                .and_then(|path| path.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                // No file behind it: the key's own reference is the only name until the
                // source hands tags over.
                .unwrap_or_else(|| now.key.path.display().to_string())
        } else {
            title
        };
        let mut texts = PieceTexts {
            trackno: meta
                .map(|m| m.track_no)
                .filter(|no| *no > 0)
                .map(|no| format!("{no:02}.")),
            title: Some(title),
            duration: now.duration_secs.map(|d| format!("({})", fmt_time(d))),
            artist: meta.map(|m| m.artist.clone()).filter(|s| !s.is_empty()),
            album: meta.map(|m| m.album.clone()).filter(|s| !s.is_empty()),
            year: meta
                .map(|m| m.year)
                .filter(|y| *y != 0)
                .map(|y| y.to_string()),
            genre: meta.map(|m| m.genre.clone()).filter(|s| !s.is_empty()),
            quality: meta
                .map(|m| {
                    group_head::quality(
                        Some(m.codec.as_str()).filter(|c| !c.is_empty()),
                        m.bitrate_kbps,
                        m.bitrate_kbps,
                        m.bit_depth,
                        m.sample_rate_hz,
                    )
                })
                .filter(|s| !s.is_empty()),
            next: None,
            queued: None,
        };
        if items
            .iter()
            .any(|i| matches!(i, InfoPiece::Next | InfoPiece::Queued))
        {
            let (count, next) = self.queue_info(cx);
            texts.next = next.map(|line| rox_i18n::t!("track-info-next", line = line).to_string());
            texts.queued = (count > 0)
                .then(|| rox_i18n::t!("track-info-queued-count", count = count as u64).to_string());
        }
        // Resolves only when a row includes the piece. A station has no file to
        // take art from, so it uses the backdrop's source: the song's cover if
        // the lookup found one, the station logo otherwise.
        let thumb: Option<Thumb> = items.contains(&InfoPiece::Art).then(|| match now.path() {
            Some(path) => {
                let path = path.to_path_buf();
                self.state
                    .thumbs
                    .update(cx, |thumbs, cx| thumbs.get(&path, cx))
            }

            None => match self.state.now_art.read(cx).live_art() {
                Some(image) => Thumb::Ready(image),
                None => Thumb::Missing,
            },
        });
        // Built ahead of the row loop so the loop can hold the crawl states
        // mutably.
        let chips: Vec<Option<Stateful<Div>>> = (0..items
            .iter()
            .filter(|i| matches!(i, InfoPiece::Output))
            .count())
            .map(|ix| self.output_chip(ix, now.live, cx))
            .collect();

        // Marks the piece that names the source: the album, which holds a
        // station's name once the overlay has run, or the title before the first
        // announcement.
        let glyph = match now.origin {
            Origin::Local => None,
            Origin::Radio => Some(icons::RADIO),
            Origin::Subsonic => Some(icons::DATABASE),
        }
        .map(|path| match texts.album.is_some() {
            true => (InfoPiece::Album, path),
            false => (InfoPiece::Title, path),
        });

        // Rows keep their editor indices so the scales line up past an empty
        // row.
        let mut plans: Vec<(usize, Vec<RowBit>)> = editor_rows(&items)
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.is_empty())
            .map(|(ix, row)| (ix, row_bits(row, &texts, glyph)))
            .collect();
        // The end-of-queue note trails the first row's last run, where
        // the single line has always worn it.
        if ended && let Some((_, bits)) = plans.first_mut() {
            let note = ("(queue finished)".to_string(), true);
            match bits.last_mut() {
                Some(RowBit::Run(run)) => run.push(note),
                _ => bits.push(RowBit::Run(vec![note])),
            }
        }
        // Each row's slice of the crawl states, so the cycle can reset one
        // row's runs.
        let mut row_runs: Vec<std::ops::Range<usize>> = Vec::with_capacity(plans.len());
        let mut runs = 0usize;
        for (_, bits) in &plans {
            let n = bits.iter().filter(|b| matches!(b, RowBit::Run(_))).count();
            row_runs.push(runs..runs + n);
            runs += n;
        }
        if self.marquees.len() != runs {
            self.marquees.resize_with(runs, MarqueeScroll::new);
        }

        // Live only with something to trade.
        let cycling = swap && plans.len() > 1;
        let (active, cycle_fade) = if cycling {
            self.advance_cycle(mode, dwell, &row_runs, window)
        } else {
            (0, 1.0)
        };

        // Built ahead like the chips, at their row's scale.
        let mut hearts: Vec<AnyElement> = Vec::new();
        if items.contains(&InfoPiece::Favourite) {
            let (fav_id, fav_on) = self.favourite_for(&now.key, cx);
            let heart_scales: Vec<f32> = plans
                .iter()
                .flat_map(|(scale_ix, bits)| {
                    let scale = scales.get(*scale_ix).copied().unwrap_or(1.0).clamp(
                        ROW_SCALE_MIN,
                        settings_ui::ceiling(ROW_SCALE_MIN, ROW_SCALE_MAX),
                    );
                    bits.iter()
                        .filter(|bit| matches!(bit, RowBit::Fixed(InfoPiece::Favourite)))
                        .map(move |_| scale)
                        .collect::<Vec<_>>()
                })
                .collect();
            hearts = heart_scales
                .into_iter()
                .map(|scale| self.favourite_heart(fav_id, fav_on, scale, cx))
                .collect();
        }
        let mut heart_iter = hearts.into_iter();
        // The id comes off the heart's resolve, the value off the tags cache.
        let mut stars: Vec<AnyElement> = Vec::new();
        if items.contains(&InfoPiece::Rating) {
            let (rating_id, _) = self.favourite_for(&now.key, cx);
            let count = plans
                .iter()
                .flat_map(|(_, bits)| bits.iter())
                .filter(|bit| matches!(bit, RowBit::Fixed(InfoPiece::Rating)))
                .count();
            stars = (0..count)
                .map(|_| self.rating_stars(rating_id, rating_value, cx))
                .collect();
        }
        let mut star_iter = stars.into_iter();

        let entity_id = cx.entity_id();
        let mut chip_iter = chips.into_iter();
        let mut run_ix = 0usize;
        let mut rows: Vec<Div> = Vec::new();
        for (row_ord, (scale_ix, bits)) in plans.into_iter().enumerate() {
            // A row waiting its turn renders nothing but still counts its bits past,
            // so the crawl states and prebuilt elements stay lined up.
            if cycling && row_ord != active {
                for bit in bits {
                    match bit {
                        RowBit::Run(_) => run_ix += 1,
                        RowBit::Fixed(InfoPiece::Output) => {
                            chip_iter.next();
                        }
                        RowBit::Fixed(InfoPiece::Favourite) => {
                            heart_iter.next();
                        }
                        RowBit::Fixed(InfoPiece::Rating) => {
                            star_iter.next();
                        }
                        RowBit::Fixed(_) | RowBit::Glyph(_) => {}
                    }
                }
                continue;
            }
            let scale = scales.get(scale_ix).copied().unwrap_or(1.0).clamp(
                ROW_SCALE_MIN,
                settings_ui::ceiling(ROW_SCALE_MIN, ROW_SCALE_MAX),
            );
            let mut row = div()
                .flex_none()
                .w_full()
                .flex()
                .items_center()
                .map(|d| justify(d, align))
                .gap(tokens::SPACE_SM)
                .px(tokens::SPACE_MD);
            // A stored 1 reads as follow-panel, so the stock line never forces a
            // size.
            if (scale - 1.0).abs() > 0.001 {
                row = row.text_size(rems(scale));
            }
            // The shown row takes the cycle's fade whole, so a chip or a heart
            // trades with its row.
            if cycle_fade < 1.0 {
                row = row.opacity(cycle_fade);
            }
            for bit in bits {
                match bit {
                    RowBit::Run(segments) => {
                        let marquee = &mut self.marquees[run_ix];
                        run_ix += 1;
                        // Mirror the configured rest before anything
                        // refills a hold this frame.
                        marquee.delay = delay;
                        // Under the cycle the crawl parks at the end and hands over instead of
                        // bouncing home.
                        marquee.cycling = cycling;
                        row = row.child(match mode {
                            MarqueeMode::Off => run_line(&segments).into_any_element(),
                            MarqueeMode::Scroll | MarqueeMode::Loop => marquee_line(
                                marquee, mode, speed, &segments, run_ix, entity_id, window,
                            )
                            .into_any_element(),
                        });
                    }
                    RowBit::Fixed(InfoPiece::Output) => {
                        if let Some(Some(chip)) = chip_iter.next() {
                            row = row.child(chip);
                        }
                    }
                    RowBit::Fixed(InfoPiece::Favourite) => {
                        if let Some(heart) = heart_iter.next() {
                            row = row.child(heart);
                        }
                    }
                    RowBit::Fixed(InfoPiece::Rating) => {
                        if let Some(control) = star_iter.next() {
                            row = row.child(control);
                        }
                    }
                    RowBit::Fixed(InfoPiece::Art) => {
                        if let Some(thumb) = thumb.clone() {
                            // Scaled with the row's text so the art matches the line.
                            let side = palette::scaled_px(20.) * scale;
                            // The music note is the shape of a file with no cover, so a station
                            // gets the radio mark.
                            let empty_glyph = match now.live {
                                true => icons::RADIO,
                                false => icons::MUSIC,
                            };
                            row = row.child(div().flex_none().w(side).h(side).child(match thumb {
                                Thumb::Ready(_) => group_head::art_content(
                                    thumb,
                                    f32::from(tokens::RADIUS),
                                    12.,
                                    false,
                                ),
                                _ => empty_cover(empty_glyph, 12. * scale),
                            }));
                        }
                    }
                    RowBit::Fixed(InfoPiece::Spacer) => {
                        row = row.child(div().flex_1());
                    }
                    RowBit::Fixed(InfoPiece::Divider) => {
                        row = row.child(div().flex_1().h(px(1.)).bg(palette::border()));
                    }
                    RowBit::Glyph(path) => {
                        // Muted and sized with the text, so it reads as byline rather than a
                        // control.
                        row = row.child(
                            svg()
                                .path(path)
                                .size(palette::scaled_px(13.) * scale)
                                .flex_none()
                                .text_color(palette::text_muted()),
                        );
                    }
                    RowBit::Fixed(_) => {}
                }
            }
            rows.push(row);
        }
        shell.children(rows)
    }
}

/// [`group_head::art_content`] draws a music note here, which is wrong
/// for a station: there was never a file to take a cover off.
fn empty_cover(glyph: &'static str, icon_px: f32) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(glyph)
                .size(px(icon_px))
                .text_color(palette::text_faint()),
        )
        .into_any_element()
}

/// Bright segments hold their width and the muted ones give way.
fn run_line(segments: &[(String, bool)]) -> Div {
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(tokens::SPACE_SM)
        .children(segments.iter().map(|(text, muted)| {
            if *muted {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(text.clone()))
            } else {
                div()
                    .flex_shrink_0()
                    .max_w_full()
                    .truncate()
                    .child(SharedString::from(text.clone()))
            }
        }))
}

fn run_row(segments: &[(String, bool)]) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_SM)
        .whitespace_nowrap()
        .children(segments.iter().map(|(text, muted)| {
            let piece = div().child(SharedString::from(text.clone()));
            if *muted {
                piece.text_color(palette::text_muted())
            } else {
                piece
            }
        }))
}

/// Scroll crawls out, rests, and crawls home; loop doubles the line and
/// wraps the offset. `run_ix` keeps the boxes' ids apart across rows.
#[allow(clippy::too_many_arguments)]
fn marquee_line(
    marquee: &mut MarqueeScroll,
    mode: MarqueeMode,
    speed: f32,
    segments: &[(String, bool)],
    run_ix: usize,
    entity_id: EntityId,
    window: &mut Window,
) -> Stateful<Div> {
    // Both start at zero, so a fresh panel stays still until the first
    // layout.
    let container = f32::from(marquee.handle.bounds().size.width);
    let overflow = f32::from(marquee.handle.max_offset().width);
    let moving = if mode == MarqueeMode::Loop {
        if marquee.looping {
            // The layout is doubled: peel the second copy and the gap back off.
            let line = (overflow + container - MARQUEE_GAP) / 2.0;
            if line <= container + 0.5 {
                marquee.reset();
                false
            } else {
                marquee.advance_loop(line + MARQUEE_GAP, speed);
                true
            }
        } else if overflow > 0.0 {
            marquee.looping = true;
            true
        } else {
            false
        }
    } else {
        marquee.looping = false;
        if overflow > 0.0 {
            marquee.advance(overflow, speed, marquee.cycling);
            true
        } else {
            if marquee.offset != 0.0 {
                marquee.reset();
            }
            false
        }
    };
    if moving {
        window.request_animation_frame();
    }
    marquee
        .handle
        .set_offset(point(px(-marquee.offset), px(0.)));

    // No frames run while the line fits, so a resize stealing the room would
    // go unseen; the probe wakes the panel when the overflow stops matching
    // the crawl.
    let handle = marquee.handle.clone();
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

    // Twice, a gap apart, so the wrap happens on an identical picture.
    let content = if marquee.looping {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(MARQUEE_GAP))
            .child(run_row(segments))
            .child(run_row(segments))
    } else {
        run_row(segments)
    };

    // min_w_0 lets the box shrink below its content, and flex sizes the
    // child row at max-content. Lose either and there's no overflow to
    // crawl.
    div()
        .id(("track-marquee", run_ix))
        .flex()
        .min_w_0()
        .max_w_full()
        .overflow_x_scroll()
        .track_scroll(&marquee.handle)
        .child(content)
        .child(probe)
}

// The width is enough of the track info line to read a title.
transport_panel!(
    TrackInfoPanel,
    "track info",
    rox_i18n::t!("panel-title-track-info"),
    min_w = 120.
);

#[cfg(test)]
mod tests {
    use super::{InfoPiece, PieceTexts, RowBit, TrackInfoConfig, editor_rows, icons, row_bits};
    use crate::panel::Align;

    fn texts() -> PieceTexts {
        PieceTexts {
            trackno: Some("05.".into()),
            title: Some("Level Up".into()),
            duration: Some("(2:17)".into()),
            artist: Some("USAO".into()),
            album: Some("REVOLUTION BEATZ".into()),
            year: None,
            genre: None,
            quality: None,
            next: None,
            queued: None,
        }
    }

    #[test]
    fn legacy_shapes_fold_into_the_piece_list() {
        let config: TrackInfoConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == TrackInfoConfig::default().items);
        assert!(config.items.contains(&InfoPiece::Output));

        let config: TrackInfoConfig = serde_json::from_str(r#"{"show_output": false}"#).unwrap();
        assert!(
            config.items
                == vec![
                    InfoPiece::TrackNo,
                    InfoPiece::Title,
                    InfoPiece::Duration,
                    InfoPiece::Artist,
                    InfoPiece::Album,
                ]
        );

        let config: TrackInfoConfig = serde_json::from_str(r#"{"align": "center"}"#).unwrap();
        assert!(config.align == Align::Center);
        assert!(config.items.first() == Some(&InfoPiece::Spacer));
    }

    #[test]
    fn legacy_swap_folds_into_two_rows() {
        let config: TrackInfoConfig =
            serde_json::from_str(r#"{"swap": true, "show_output": false}"#).unwrap();
        assert!(config.swap);
        assert!(
            config.items
                == vec![
                    InfoPiece::TrackNo,
                    InfoPiece::Title,
                    InfoPiece::Duration,
                    InfoPiece::Break,
                    InfoPiece::Artist,
                    InfoPiece::Album,
                ]
        );

        let config: TrackInfoConfig = serde_json::from_str(r#"{"swap": true}"#).unwrap();
        let rows = editor_rows(&config.items);
        assert!(rows.len() == 2);
        assert!(rows.iter().all(|row| row.contains(&InfoPiece::Output)));
        // A spacer pins the chip to the row's end.
        assert!(rows.iter().all(|row| {
            row.iter().position(|p| *p == InfoPiece::Spacer)
                < row.iter().position(|p| *p == InfoPiece::Output)
        }));
    }

    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: TrackInfoConfig =
            serde_json::from_str(r#"{"items": ["title", "artist", "title"]}"#).unwrap();
        assert!(config.items == vec![InfoPiece::Title, InfoPiece::Artist]);

        let config: TrackInfoConfig =
            serde_json::from_str(r#"{"items": ["title", "break", "title"]}"#).unwrap();
        assert!(config.items == vec![InfoPiece::Title, InfoPiece::Break, InfoPiece::Title]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: TrackInfoConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }

    #[test]
    fn the_stock_row_reads_as_the_classic_line() {
        let pieces = [
            InfoPiece::TrackNo,
            InfoPiece::Title,
            InfoPiece::Duration,
            InfoPiece::Artist,
            InfoPiece::Album,
        ];
        let bits = row_bits(&pieces, &texts(), None);
        assert!(bits.len() == 1);
        let RowBit::Run(run) = &bits[0] else {
            panic!("expected a run");
        };
        assert!(
            run == &vec![
                ("05. Level Up (2:17)".to_string(), false),
                ("USAO - REVOLUTION BEATZ".to_string(), true),
            ]
        );
    }

    #[test]
    fn fixed_pieces_cut_runs_and_empty_fields_drop() {
        let pieces = [
            InfoPiece::Title,
            InfoPiece::Spacer,
            InfoPiece::Year,
            InfoPiece::Artist,
        ];
        let bits = row_bits(&pieces, &texts(), None);
        assert!(bits.len() == 3);
        assert!(matches!(&bits[0], RowBit::Run(run) if run.len() == 1));
        assert!(matches!(&bits[1], RowBit::Fixed(InfoPiece::Spacer)));
        let RowBit::Run(run) = &bits[2] else {
            panic!("expected a run");
        };
        // The year is empty, so the muted phrase is the artist alone.
        assert!(run == &vec![("USAO".to_string(), true)]);
    }

    #[test]
    fn the_source_mark_trails_the_row_it_marks() {
        let pieces = [InfoPiece::Title, InfoPiece::Artist, InfoPiece::Album];

        let bits = row_bits(&pieces, &texts(), Some((InfoPiece::Album, icons::RADIO)));
        assert!(bits.len() == 2);
        let RowBit::Run(run) = &bits[0] else {
            panic!("expected a run");
        };
        assert!(
            run == &vec![
                ("Level Up".to_string(), false),
                ("USAO - REVOLUTION BEATZ".to_string(), true),
            ]
        );
        assert!(matches!(&bits[1], RowBit::Glyph(icons::RADIO)));

        // Behind the words, ahead of whatever sits at the far edge.
        let trailing = [InfoPiece::Title, InfoPiece::Album, InfoPiece::Output];
        let bits = row_bits(&trailing, &texts(), Some((InfoPiece::Album, icons::RADIO)));
        assert!(matches!(&bits[1], RowBit::Glyph(icons::RADIO)));
        assert!(matches!(&bits[2], RowBit::Fixed(InfoPiece::Output)));

        let elsewhere = [InfoPiece::Title, InfoPiece::Artist];
        let bits = row_bits(&elsewhere, &texts(), Some((InfoPiece::Album, icons::RADIO)));
        assert!(bits.len() == 1);

        let bits = row_bits(&pieces, &texts(), None);
        assert!(bits.len() == 1);
    }

    #[test]
    fn editor_rows_keep_empties_and_rejoin() {
        let items = vec![InfoPiece::Title, InfoPiece::Break];
        let rows = editor_rows(&items);
        assert!(rows == vec![vec![InfoPiece::Title], vec![]]);
        assert!(rows.join(&InfoPiece::Break) == items);
    }
}
