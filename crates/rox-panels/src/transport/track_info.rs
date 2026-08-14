//! The track info readout panel: the playing track's tags as ordered rows
//! of pieces. The stock arrangement is the classic one-liner, the numbered
//! title and duration with the byline behind it and the output chip at the
//! trailing edge; the arrange editor prunes, reorders, and breaks the list
//! into further rows, each with its own text size, so the same panel spans
//! a transport strip to a now-playing card. The marquee crawl and the
//! row cycle ride the rows for tight panels: the cycle shows the
//! arrangement's rows one at a time in a single line, trading on a timer.

use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, rems, svg, AnyElement, App, Context, Div, EntityId,
    EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Rgba, ScrollHandle, SharedString,
    Stateful, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use serde::{Deserialize, Serialize};

use rox_services::thumbs::Thumb;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::group_head;
use crate::panel::{
    self, align_row, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::player::{fmt_time, observe_view};
use crate::settings::ui as settings_ui;

use super::transport_panel;

/// One piece of the track line, the arrange editor's unit. The config's
/// list carries the shown ones in display order. The text pieces compose
/// into crawlable runs; the chip, art, spacer, and divider hold their own
/// shape between them.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InfoPiece {
    /// The track number, zero-padded the way the classic line writes it.
    TrackNo,
    /// The title, or the file name for a track the library does not know.
    Title,
    /// The duration in parens, the classic line's "(2:17)".
    Duration,
    Artist,
    Album,
    Year,
    Genre,
    /// The codec, stream shape, and bitrate readout, from
    /// [`group_head::quality`].
    Quality,
    /// What plays next off the explicit queue; empty during plain context
    /// playback, like the queue widgets.
    Next,
    /// How deep the explicit queue stands, as "N queued".
    Queued,
    /// The negotiated-output chip, the retired toggle as a piece: it
    /// claims its width first and never rides a crawl.
    Output,
    /// The heart over the playing track, the favourite panel's toggle as
    /// a piece, for the card that wants it riding a corner.
    Favourite,
    /// The stars over the playing track, the same write the rating panel
    /// and the library's rating column make.
    Rating,
    /// An inline cover square, one line tall, the header rows' small
    /// sibling.
    Art,
    /// A flexible gap that pushes the pieces around it apart; a row holds
    /// as many as the layout wants.
    Spacer,
    /// A spacer that draws a hairline in the border color across its gap.
    Divider,
    /// The line break: everything after it drops to the next row.
    Break,
}

/// The line's full catalog in stock order: what the arrange editor offers,
/// and where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<InfoPiece>] = &[
    panel::ArrangeSpec {
        label: "Track No",
        icon: Some(icons::LIST_MUSIC),
        value: InfoPiece::TrackNo,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Title",
        icon: Some(icons::MUSIC),
        value: InfoPiece::Title,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Duration",
        icon: Some(icons::CLOCK),
        value: InfoPiece::Duration,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Artist",
        icon: Some(icons::MIC),
        value: InfoPiece::Artist,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Album",
        icon: Some(icons::DISC),
        value: InfoPiece::Album,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Year",
        icon: Some(icons::CALENDAR),
        value: InfoPiece::Year,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Genre",
        icon: Some(icons::TAG),
        value: InfoPiece::Genre,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Quality",
        icon: Some(icons::AUDIO_WAVEFORM),
        value: InfoPiece::Quality,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Next",
        icon: Some(icons::SKIP_FORWARD),
        value: InfoPiece::Next,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Queued",
        icon: Some(icons::LAYERS),
        value: InfoPiece::Queued,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Output",
        icon: Some(icons::VOLUME_2),
        value: InfoPiece::Output,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Favourite",
        icon: Some(icons::HEART),
        value: InfoPiece::Favourite,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Rating",
        icon: Some(icons::STAR),
        value: InfoPiece::Rating,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Art",
        icon: Some(icons::IMAGE),
        value: InfoPiece::Art,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: InfoPiece::Spacer,
        repeats: true,
    },
    panel::ArrangeSpec {
        label: "Divider",
        icon: Some(icons::MINUS),
        value: InfoPiece::Divider,
        repeats: true,
    },
];

/// The classic line, spelled from the retired fixed shape: the numbered
/// title and duration, the byline behind it, and the chip at the trailing
/// edge. The spacers land where the retired align knob put the text, so a
/// layout saved before the pieces became a list keeps its look; `chip`
/// off leaves the text alone, the retired toggle's read.
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

/// The track info panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Deserialization routes through
/// [`TrackInfoConfigDump`] so layouts from before the line became an
/// ordered list still read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "TrackInfoConfigDump")]
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
    /// Cycle the arrangement's rows through a single line, one at a time
    /// with a fade between turns, so a tight strip carries a whole card's
    /// worth of rows. A single-row arrangement has nothing to trade and
    /// reads as itself. Independent of the marquee: the shown row still
    /// crawls if it overflows.
    #[serde(default)]
    pub swap: bool,
    /// How long each row sits fully shown before the fade, seconds.
    #[serde(default = "default_swap_secs")]
    pub swap_secs: f32,
    /// Let the chip take the banner's tone colors when the output isn't
    /// clean, or hold the muted text color whatever the state. Off suits a
    /// transport line that wants one flat tone; the hover note still says
    /// what's going on.
    #[serde(default = "default_output_tint")]
    pub output_tint: bool,
    /// The shown pieces in display order; one not listed is hidden.
    pub items: Vec<InfoPiece>,
    /// Each row's text size as a multiplier over the panel's base, indexed
    /// like the editor's rows; a row past the list's end reads 1. What
    /// lets a card's title line tower over its byline.
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

/// The dump shape [`TrackInfoConfig`] deserializes through: the ordered
/// list newer layouts write, or the retired `show_output` toggle that was
/// the chip's whole story.
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
            // Deduped row by row, the breaks put back after: the catalog
            // doesn't carry the break (it draws as the editor's row
            // boundary, not a chip), and each row may hold its own copy
            // of a piece.
            Some(items) => items
                .split(|i| matches!(i, InfoPiece::Break))
                .map(|row| panel::dedup(ITEMS, row.to_vec()))
                .collect::<Vec<_>>()
                .join(&InfoPiece::Break),
            // The retired fixed panel swapped its heading against its
            // byline; the cycle trades rows, so the fold splits the
            // classic line into those two rows, the chip riding both so
            // it never blinks out with a side.
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

/// The chip's hover note: why it's colored, in words rather than a legend
/// nobody would find. Only ever built for the two states that earn a color,
/// so a plain chip has no tooltip at all.
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

/// What a text run does when it outgrows the panel.
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

/// The chip is on unless a layout turns it off. Legacy-only; new layouts
/// carry the chip as a piece.
fn default_show_output() -> bool {
    true
}

/// The chip colors itself unless a layout asks it not to.
fn default_output_tint() -> bool {
    true
}

/// The default rest at each end of a scroll, a beat to read the edge.
fn default_marquee_delay() -> f32 {
    2.0
}
/// The gap between the line's two copies in loop mode, the breather
/// between a tail and the next head.
const MARQUEE_GAP: f32 = 48.0;
/// The swap fade's length, going out and coming in.
const SWAP_FADE_SECS: f32 = 0.4;

/// The per-row text scale range the settings sliders span, over the
/// panel's base size.
const ROW_SCALE_MIN: f32 = 0.5;
const ROW_SCALE_MAX: f32 = 3.0;

/// A text run's crawl state while the marquee setting is on, one per run
/// on the panel. The scroll handle owns the clipping and reports the
/// overflow off the last layout; the rest drives the offset through it,
/// one leg at a time.
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
    /// Loop mode's verdict off the last layout: whether one copy alone
    /// overflows, so the line renders doubled and wraps.
    looping: bool,
    /// Whether the row cycle rides this run's crawl this frame. The body
    /// sets it; the crawl reads it to decide between bouncing back and
    /// parking at the end.
    cycling: bool,
    /// The scroll-mode handshake: the crawl finished its trip out and the
    /// cycle may fade the row away.
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

    /// Back home, resting: for a fresh row coming in, and for a track or
    /// mode change starting the crawl over.
    fn reset(&mut self) {
        self.offset = 0.0;
        self.dir = 1.0;
        self.hold = self.delay;
        self.last_tick = Instant::now();
        self.looping = false;
        self.crawl_done = false;
    }

    /// One frame of the scroll crawl: run the rest down, then step along
    /// the current leg. Without `park` it turns around with a fresh rest
    /// at each end; with it (the row cycle rides the crawl) it stays put
    /// once it has crawled out and rested, raising `crawl_done` for the
    /// cycle to fade the row away. The step clamps so a stalled frame
    /// never teleports the line.
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

/// The row cycle's state while the cycle setting shows one row at a
/// time: which of the shown rows is up, when its turn started, and the
/// fade-out clock once the row has said its piece.
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

/// The text pieces resolved for the playing track, each None when its
/// field is empty so the piece drops out of the line the way the header
/// pieces do.
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

/// One row's render plan: the crawlable text runs between the fixed
/// pieces, in piece order.
enum RowBit {
    /// A contiguous stretch of text pieces composed into colored
    /// segments: the text and whether it sits muted.
    Run(Vec<(String, bool)>),
    /// A piece that holds its own shape outside the crawl.
    Fixed(InfoPiece),
}

/// Compose one row's pieces into its runs and fixed pieces. Same-color
/// neighbors read as one phrase: bright pieces join with a space, the
/// classic "05. Title (2:17)", muted ones with the byline's " - ". A
/// piece whose field is empty just drops out of the line.
fn row_bits(pieces: &[InfoPiece], texts: &PieceTexts) -> Vec<RowBit> {
    let mut bits = Vec::new();
    let mut run: Vec<(String, bool)> = Vec::new();
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
            // Rows come pre-split; a break never reaches here.
            InfoPiece::Break => continue,
        };
        let Some((text, muted)) = text else { continue };
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
    bits
}

/// The config's list cut at the break into one piece list per row, kept
/// as the editor shows them, empty rows included, so the per-row scales
/// stay indexed the same on both sides.
fn editor_rows(items: &[InfoPiece]) -> Vec<Vec<InfoPiece>> {
    items
        .split(|i| matches!(i, InfoPiece::Break))
        .map(|row| row.to_vec())
        .collect()
}

/// The track info readout the playback panel's status line grew into: the
/// playing track's tags from the library composed per the config's rows,
/// with the session errors and the idle message in their place while
/// nothing shows.
pub struct TrackInfoPanel {
    state: AppState,
    config: TrackInfoConfig,
    /// The playing path's tags, or None for a file the library does not
    /// know. Cached because the pump notifies every frame and the lookup is
    /// a database query; cleared when the track or the catalog changes.
    meta: Option<(TrackKey, Option<rox_library::store::TrackMeta>)>,
    /// The explicit queue's readouts keyed on its revision: the depth and
    /// what plays next. The snapshot walk and the library lookup only
    /// rerun when the queue actually moves.
    queue_info: Option<(u64, usize, Option<String>)>,
    /// The playing track's id and favourite state for the heart piece,
    /// cached like the tags; cleared when the catalog or the playlists
    /// move.
    favourite: Option<(TrackKey, Option<i64>, bool)>,
    /// The crawl states, one per text run on the panel, in row order;
    /// rebuilt when the arrangement changes shape.
    marquees: Vec<MarqueeScroll>,
    /// Which row is up while the cycle setting trades them, and where its
    /// turn stands.
    cycle: RowCycle,
    /// The track the crawls belong to; a track change starts them over.
    marquee_key: Option<TrackKey>,
    /// The settings page's speed slider strip.
    speed_scrub: ScrubState,
    /// The settings page's end-rest delay slider strip.
    delay_scrub: ScrubState,
    /// The settings page's swap dwell slider strip.
    swap_scrub: ScrubState,
    /// The settings page's per-row size slider strips, grown to the row
    /// count as the page builds.
    scale_scrubs: Vec<ScrubState>,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
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
                // A favourites toggle here or on any other surface moves
                // the heart; the tags stand.
                if matches!(event, LibraryEvent::PlaylistsChanged) {
                    this.favourite = None;
                    cx.notify();
                    return;
                }
                // A landed star moves the rating, which rides the tags
                // cache; re-resolve it, nothing else changed.
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
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _library_changed,
        }
    }

    /// What the line does when it doesn't fit, the one knob worth
    /// flipping without opening settings. Everything else stays on the
    /// settings page: the pieces are the arrange editor's, and a context
    /// menu that carries every knob is just a worse settings page.
    ///
    /// Flat checked items rather than a submenu on purpose: a plain
    /// `.checked()` only refreshes at the top level, and a nested flyout
    /// would show a stale tick until it was reopened.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu.separator().label("Overflow");
        for (name, mode) in [
            ("Truncate", MarqueeMode::Off),
            ("Scroll", MarqueeMode::Scroll),
            ("Loop", MarqueeMode::Loop),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.marquee == mode)
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.marquee = mode;
                            // A mode change leaves the crawls mid-trip, and
                            // the offsets they're holding mean nothing to
                            // the mode arriving.
                            this.reset_marquees();
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    /// Every run's crawl back home and the row cycle to its first row,
    /// for a track or mode change.
    fn reset_marquees(&mut self) {
        for marquee in &mut self.marquees {
            marquee.reset();
        }
        self.cycle.reset();
    }

    /// One frame of the row cycle: fade in, hold while the shown row says
    /// its piece, fade out, then the next row comes in with its crawls
    /// back home. The hold is the dwell timer, except under scroll mode,
    /// where the row's crawls run the clock instead: every overflowing
    /// run has to crawl out and rest (a fitting run counts done once the
    /// dwell passes) before the fade-out starts. Hands back the shown
    /// row's index into the render plans and its fade. The cycle never
    /// settles, so it keeps its own frames running.
    fn advance_cycle(
        &mut self,
        mode: MarqueeMode,
        dwell: f32,
        row_runs: &[std::ops::Range<usize>],
        window: &mut Window,
    ) -> (usize, f32) {
        window.request_animation_frame();
        let smooth = |u: f32| u * u * (3.0 - 2.0 * u);
        // An arrangement edit can shrink the row list under a standing
        // cycle; landing back on the first row beats indexing past the end.
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

    /// Store the crawl pace, pixels per second.
    fn set_marquee_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.config.marquee_speed = speed;
        cx.notify();
    }

    /// Store the end rest, seconds.
    fn set_marquee_delay(&mut self, delay: f32, cx: &mut Context<Self>) {
        self.config.marquee_delay = delay;
        cx.notify();
    }

    /// Store the swap dwell, seconds.
    fn set_swap_secs(&mut self, secs: f32, cx: &mut Context<Self>) {
        self.config.swap_secs = secs;
        cx.notify();
    }

    /// The playing track's tags, from the cache or one lookup on a miss.
    /// Keyed on the whole track, so two cue tracks of one image don't both
    /// draw whichever of them the library sorts first.
    fn meta_for(&mut self, key: &TrackKey, cx: &App) -> Option<&rox_library::store::TrackMeta> {
        if self.meta.as_ref().map(|(k, _)| k) != Some(key) {
            let meta = self.state.library.read(cx).meta_for_key(key);
            self.meta = Some((key.clone(), meta));
        }
        self.meta.as_ref().and_then(|(_, meta)| meta.as_ref())
    }

    /// The explicit queue's depth and next line, from the cache or one
    /// snapshot walk when the revision moved.
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

    /// The playing track's id and favourite state, from the cache or one
    /// lookup on a track change, the favourite panel's read.
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

    /// One heart piece: filled and accented while the playing track sits
    /// in the favourites, dimmed while nothing resolves, a click running
    /// the same toggle the favourite panel and the library's heart column
    /// run. Scaled with its row, so a title-row heart holds the line.
    fn favourite_heart(
        &self,
        id: Option<i64>,
        on: bool,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tip = match (id.is_some(), on) {
            (false, _) => "Nothing to favourite",
            (true, true) => "Remove from favourites",
            (true, false) => "Add to favourites",
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
                    // Nothing to favourite: the heart stays up, dimmed and
                    // dead, so the piece holds its place in the row.
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

    /// One stars piece: the rating panel's control in the row, the same
    /// write the library's rating column makes, so a star set here shows
    /// everywhere else. The stars keep their stock size whatever the
    /// row's text scale, like every other rating surface.
    fn rating_stars(&self, id: Option<i64>, value: u8, _cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.clone();
        // Keyed by the shown track so the hover preview matches every
        // other surface rating the same track.
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
            // Nothing to rate: the stars stay up, dimmed, so the piece
            // holds its place in the row.
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
        // One size slider per row, under the row's editor-side number; a
        // single-row line just calls it the text size.
        let sizes: Vec<AnyElement> = (0..rows.len())
            .map(|ix| {
                let label = if rows.len() == 1 {
                    "Text Size".to_string()
                } else {
                    format!("Row {} Size", ix + 1)
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
                "Pieces",
                Some(
                    "Drag along a row to reorder and between rows to move; \
                     a chip's x and plus hide and show",
                ),
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
                        "Color Output Chip",
                        Some(
                            "Let the chip turn warning colors when the output falls back or \
                             resamples. Off keeps it the same muted tone always, and the hover \
                             note still explains the state",
                        ),
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
                            this.reset_marquees();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(self.config.marquee != MarqueeMode::Off, |d| {
                    d.child(panel::setting_row(
                        "Speed",
                        Some("How fast the line crawls"),
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
                        "Delay",
                        Some("How long the line rests at each end before moving again"),
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
                    "Cycle Rows",
                    Some(
                        "Show the arrangement's rows one at a time in a single line, \
                         fading between them; one row alone reads as itself",
                    ),
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
                        "Cycle every",
                        Some("How long each row sits before the fade"),
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
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl TrackInfoPanel {
    /// The output chip: what the device settled on, short enough to live at
    /// the end of a transport line. Muted while nothing is being converted,
    /// carrying the banner's tone colors when something is, or a muted alert
    /// face in their place when the tint is off, so a glance says whether
    /// what's playing is what the file holds. None when no stream has
    /// negotiated yet. `ix` keeps two chips across rows apart for gpui.
    fn output_chip(&self, ix: usize, cx: &App) -> Option<Stateful<Div>> {
        let status = self.state.player.read(cx).output_status()?;
        let negotiated = &status.negotiated;
        let exclusive = negotiated.mode == rox_playback::output::Mode::Exclusive;
        let resampling = status
            .source_rate
            .is_some_and(|source| source != negotiated.sample_rate);
        // Shared output is the normal state, so it says nothing and colors
        // nothing: a chip that's always lit stops being a signal. The two
        // things worth interrupting for are a mode that was asked for and
        // refused, and a conversion happening that didn't have to.
        let (color, why): (Rgba, Option<SharedString>) = if let Some(reason) = &negotiated.fallback
        {
            (
                palette::tone_bad(),
                Some(
                    format!(
                        "Exclusive output was asked for and the device wouldn't give it up, so \
                         the shared mixer is standing in. The device said: {reason}"
                    )
                    .into(),
                ),
            )
        } else if resampling {
            let source = group_head::khz(status.source_rate.unwrap_or_default());
            let device = group_head::khz(negotiated.sample_rate);
            // Exclusive resamples too when the card won't take the file's
            // rate, and that's the case worth saying out loud: the toggle is
            // on, the claim went through, and it still isn't the file's own
            // samples.
            (
                palette::tone_warn(),
                Some(
                    if exclusive {
                        format!(
                            "This file is {source} kHz and the card took {device} kHz, so every \
                             sample is being converted on the way out. The device wouldn't run at \
                             the file's own rate."
                        )
                    } else {
                        format!(
                            "This file is {source} kHz and the mixer is running at {device} kHz, \
                             so every sample is being converted on the way out. Exclusive mode \
                             would hand the card the file's own rate instead."
                        )
                    }
                    .into(),
                ),
            )
        } else {
            (palette::text_muted(), None)
        };
        // The face stands in for the tint rather than doubling it: with the
        // color on it would say the same thing twice, so it only turns up in
        // the two flagged states once the chip has gone flat.
        let face = why.is_some() && !self.config.output_tint;
        let color = if self.config.output_tint {
            color
        } else {
            palette::text_muted()
        };
        // "Shared" is every desktop's default and carries no information;
        // "Exclusive" is worth the two words because it's the state someone
        // went looking for.
        // The rate goes through the same speller the library column and the
        // metadata field use, so one card reads the same everywhere.
        let label = format!(
            "{}{} kHz {}",
            if exclusive { "Exclusive " } else { "" },
            group_head::khz(negotiated.sample_rate),
            negotiated.format
        );
        Some(
            div()
                .id(("output-chip", ix))
                // flex_none is the whole point: the chip claims its width
                // first and the line crawls in whatever is left, so it never
                // rides along with the marquee.
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
            // Nothing to describe: a session still opening, or the reason
            // one failed to start. Plain idle stays blank, the chip still
            // reporting if the arrangement carries one.
            let line: Option<SharedString> = if active {
                Some("opening...".into())
            } else {
                error
            };
            let chip = items
                .contains(&InfoPiece::Output)
                .then(|| self.output_chip(0, cx))
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

        // A fresh track starts every cycle over: crawls home, swaps back
        // to their headings.
        if self.marquee_key.as_ref() != Some(&now.key) {
            self.marquee_key = Some(now.key.clone());
            self.reset_marquees();
        }

        // An untagged file still shows something: its file name for the
        // title, no byline. The lookup borrow ends here; the texts own
        // their strings so the crawl states below can borrow freely.
        let meta = self.meta_for(&now.key, cx);
        let rating_value = meta.map(|m| m.rating).unwrap_or(0);
        let title = meta.map(|m| m.title.clone()).unwrap_or_default();
        let title = if title.is_empty() {
            now.path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| now.path().display().to_string())
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
            texts.next = next.map(|line| format!("Next: {line}"));
            texts.queued = (count > 0).then(|| format!("{count} queued"));
        }
        // The inline art resolves only when a row carries the piece, the
        // header lines' rule; the thumb cache does the caching.
        let thumb: Option<Thumb> = items.contains(&InfoPiece::Art).then(|| {
            let path = now.path().to_path_buf();
            self.state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx))
        });
        // The chips build ahead of the row loop, one per occurrence, so
        // the loop below can hold the crawl states mutably.
        let chips: Vec<Option<Stateful<Div>>> = (0..items
            .iter()
            .filter(|i| matches!(i, InfoPiece::Output))
            .count())
            .map(|ix| self.output_chip(ix, cx))
            .collect();

        // The rows keep their editor indices so the scales line up even
        // past an empty row, and each row's plan splits into crawlable
        // runs and the fixed pieces between them.
        let mut plans: Vec<(usize, Vec<RowBit>)> = editor_rows(&items)
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.is_empty())
            .map(|(ix, row)| (ix, row_bits(row, &texts)))
            .collect();
        // The end-of-queue note trails the first row's last run, where
        // the single line has always worn it.
        if ended {
            if let Some((_, bits)) = plans.first_mut() {
                let note = ("(queue finished)".to_string(), true);
                match bits.last_mut() {
                    Some(RowBit::Run(run)) => run.push(note),
                    _ => bits.push(RowBit::Run(vec![note])),
                }
            }
        }
        // Each row's slice of the crawl states, so the cycle can read and
        // reset one row's runs by index.
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

        // The row cycle: the arrangement's rows take turns in a single
        // line, so a tight strip carries a whole card's worth. Live only
        // with something to trade; a lone row reads as itself.
        let cycling = swap && plans.len() > 1;
        let (active, cycle_fade) = if cycling {
            self.advance_cycle(mode, dwell, &row_runs, window)
        } else {
            (0, 1.0)
        };

        // The hearts build ahead like the chips, one per occurrence at
        // its row's scale, so the row loop below can hold the crawl
        // states mutably.
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
        // The stars the same way; the id comes off the heart's resolve,
        // the value off the tags cache.
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
            // A row waiting its turn in the cycle renders nothing, but
            // its runs and fixed pieces still count past, so the crawl
            // states and the prebuilt elements stay lined up with their
            // rows.
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
                        RowBit::Fixed(_) => {}
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
            // A stored 1 reads as follow-panel, like the theme's own font
            // scale, so the stock line never forces a size of its own.
            if (scale - 1.0).abs() > 0.001 {
                row = row.text_size(rems(scale));
            }
            // The shown row wears the cycle's fade whole, pieces and all,
            // so a chip or a heart trades with its row instead of sitting
            // over the crossfade.
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
                        // Under the cycle the crawl parks at the end and
                        // hands over instead of bouncing home.
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
                            // A line-tall square, scaled with its row's
                            // text so the art keeps matching the line.
                            let side = palette::scaled_px(20.) * scale;
                            row = row.child(div().flex_none().w(side).h(side).child(
                                group_head::art_content(
                                    thumb,
                                    f32::from(tokens::RADIUS),
                                    12.,
                                    false,
                                ),
                            ));
                        }
                    }
                    RowBit::Fixed(InfoPiece::Spacer) => {
                        row = row.child(div().flex_1());
                    }
                    RowBit::Fixed(InfoPiece::Divider) => {
                        row = row.child(div().flex_1().h(px(1.)).bg(palette::border()));
                    }
                    RowBit::Fixed(_) => {}
                }
            }
            rows.push(row);
        }
        shell.children(rows)
    }
}

/// One run of text sitting still: bright segments hold their width and
/// the muted ones give way, the fixed line's behavior since it was two
/// pieces.
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

/// One copy of a run for the marquee's scroll box, refusing to wrap.
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

/// The crawling take on a text run, for the scroll and loop modes. The
/// scroll box does the clipping and hands back the overflow off the last
/// layout: scroll crawls out, rests, and crawls home again, while loop
/// doubles the line and wraps the offset for an unbroken ticker. `run_ix`
/// keeps the boxes' element ids apart across the panel's rows.
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
    // Both come off the last layout and start at zero, so a fresh panel
    // sits still until it knows better.
    let container = f32::from(marquee.handle.bounds().size.width);
    let overflow = f32::from(marquee.handle.max_offset().width);
    let moving = if mode == MarqueeMode::Loop {
        if marquee.looping {
            // The layout is doubled: peel the second copy and the gap
            // back off for the single line's width.
            let line = (overflow + container - MARQUEE_GAP) / 2.0;
            if line <= container + 0.5 {
                // Room came back; one copy fits again.
                marquee.reset();
                false
            } else {
                marquee.advance_loop(line + MARQUEE_GAP, speed);
                true
            }
        } else if overflow > 0.0 {
            // One copy overflows: double up and start the wrap.
            marquee.looping = true;
            true
        } else {
            false
        }
    } else {
        marquee.looping = false;
        if overflow > 0.0 {
            // Under the row cycle the crawl parks at the end and hands
            // over; the cycle brings the next row in back at the start.
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

    // No frames run while the line fits, so a resize that steals the
    // room would go unseen; the probe repaints with the panel and wakes
    // it whenever the overflow no longer matches the crawl.
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

    // Loop mode shows the line twice, a gap apart, so the wrap lands on
    // an identical picture.
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

    // min_w_0 lets the box shrink below its content in the panel's row;
    // without it the automatic minimum holds the box at the full line's
    // width. flex makes the box size its child row at max-content, since
    // as a default block the row would stretch to the box instead. Either
    // way lost, there is no overflow to crawl.
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
transport_panel!(TrackInfoPanel, "track info", "Track Info", min_w = 120.);

#[cfg(test)]
mod tests {
    use super::{editor_rows, row_bits, InfoPiece, PieceTexts, RowBit, TrackInfoConfig};
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

    /// A layout with no items field decodes to the classic line, chip and
    /// all, and the retired toggles still read: the chip off leaves the
    /// text alone, and a centered line keeps its leading spacer.
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

    /// The retired swap traded a heading against a byline; the cycle
    /// trades rows, so a layout saved with it folds into those two rows.
    /// The chip rides both, since a row cycling away would take it along.
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
        // A spacer pins the chip to the row's end, so it holds its place
        // as the rows trade.
        assert!(rows.iter().all(|row| {
            row.iter().position(|p| *p == InfoPiece::Spacer)
                < row.iter().position(|p| *p == InfoPiece::Output)
        }));
    }

    /// A layout that carries the list uses it as-is, same-row duplicates
    /// dropped, and round-trips through a save.
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

    /// The classic arrangement composes into one run of two phrases: the
    /// bright heading joined with spaces, the muted byline with " - ".
    #[test]
    fn the_stock_row_reads_as_the_classic_line() {
        let pieces = [
            InfoPiece::TrackNo,
            InfoPiece::Title,
            InfoPiece::Duration,
            InfoPiece::Artist,
            InfoPiece::Album,
        ];
        let bits = row_bits(&pieces, &texts());
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

    /// A fixed piece cuts the text into separate runs, and an empty field
    /// drops its piece without leaving a seam in the joins.
    #[test]
    fn fixed_pieces_cut_runs_and_empty_fields_drop() {
        let pieces = [
            InfoPiece::Title,
            InfoPiece::Spacer,
            InfoPiece::Year,
            InfoPiece::Artist,
        ];
        let bits = row_bits(&pieces, &texts());
        assert!(bits.len() == 3);
        assert!(matches!(&bits[0], RowBit::Run(run) if run.len() == 1));
        assert!(matches!(&bits[1], RowBit::Fixed(InfoPiece::Spacer)));
        let RowBit::Run(run) = &bits[2] else {
            panic!("expected a run");
        };
        // The year is empty, so the muted phrase is the artist alone.
        assert!(run == &vec![("USAO".to_string(), true)]);
    }

    /// The editor's rows keep the empty well a trailing break makes, and
    /// the join puts the breaks back exactly.
    #[test]
    fn editor_rows_keep_empties_and_rejoin() {
        let items = vec![InfoPiece::Title, InfoPiece::Break];
        let rows = editor_rows(&items);
        assert!(rows == vec![vec![InfoPiece::Title], vec![]]);
        assert!(rows.join(&InfoPiece::Break) == items);
    }
}
