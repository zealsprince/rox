//! The stats window: one OS window opened from the menubar beside
//! Settings, the listening record rolled up per ADR 11. A range knob
//! (all time down to this week) scopes the page: the recency counts
//! as cards, listens over time as bars, then the artists, albums, and
//! genres you played most, each wearing the art its own wall wears - a
//! face in a circle, a sleeve in a square, a genre's own color card - and
//! the newest listens under them. Everything derives from the events
//! table by SQL on the shared catalog's connection; nothing counts along
//! the way.
//!
//! Rollups read entering the window and when a listen lands or the
//! catalog changes, never per frame; the chart and the cards' geometry
//! are gpui quads, cheap at this scale. The art comes through the same
//! two services the panels draw from, so a face or a cover already in
//! hand costs nothing here.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, relative, size, svg, AnyElement,
    App, Bounds, Context, Div, FontWeight, Global, Image, ObjectFit, ScrollHandle, SharedString,
    Subscription, Window, WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::Root;

use rox_core::fmt::fmt_ago;
use rox_core::QUEUE_CAP;
use rox_library::listens::{NamePlays, Rollup, TrackPlays};
use rox_panel_kit::motif;

use rox_core::settings::{Settings, StatsWindowState};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::charts;
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, section, SECTION_GAP};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::LibraryEvent;
use rox_services::history::HistoryEvent;
use rox_services::thumbs::Thumb;

/// How many rows the artist and album rollups show, how many cards the
/// genres get, and how far back the recents run. Ten is the natural unit
/// for a chart; the genres tile three across, so they go by threes.
const TOP_NAMES: usize = 10;
const TOP_GENRES: usize = 9;
const RECENT_ROWS: usize = 15;

/// Genre cards per lane, and how tall a card stands.
const GENRE_COLS: usize = 3;
const GENRE_CARD_H: f32 = 76.;

/// The art a rollup row wears, and the smaller square a recents row
/// carries, in px.
const ART: f32 = 40.;
const ROW_ART: f32 = 28.;

/// The bar chart's height, in px.
const CHART_H: f32 = 96.;

const DAY: i64 = 86400;

/// The hover scope for a playable row: the play control sits invisible
/// in its slot until the row is hovered, the library's rating-cell move.
const ROW_GROUP: &str = "stats-row";

/// The same for a genre card, whose play glyph rides its corner.
const CARD_GROUP: &str = "stats-card";

/// The play slot's width, [`panel::icon_control`]'s footprint, reserved
/// even on rows without a control so the counts stay in column.
const PLAY_SLOT_W: f32 = 28.;

/// How far back the page counts. Trailing windows, no calendar math,
/// like the recency rows.
#[derive(Clone, Copy, Default, PartialEq)]
enum StatsRange {
    #[default]
    All,
    Year,
    Month,
    Week,
}

impl StatsRange {
    /// The range's lower bound in unix seconds; 0 counts every event.
    fn since(self, now: i64) -> i64 {
        match self {
            StatsRange::All => 0,
            StatsRange::Year => now - 365 * DAY,
            StatsRange::Month => now - 30 * DAY,
            StatsRange::Week => now - 7 * DAY,
        }
    }

    /// The pick's key in the settings file, and the way back; an unknown
    /// key falls back to all time.
    fn key(self) -> &'static str {
        match self {
            StatsRange::All => "all",
            StatsRange::Year => "year",
            StatsRange::Month => "month",
            StatsRange::Week => "week",
        }
    }

    fn from_key(key: &str) -> StatsRange {
        match key {
            "year" => StatsRange::Year,
            "month" => StatsRange::Month,
            "week" => StatsRange::Week,
            _ => StatsRange::All,
        }
    }

    /// The overview card this range scopes the page to, and what the
    /// chart's left edge is called.
    fn card(self) -> &'static str {
        match self {
            StatsRange::All => "All Time",
            StatsRange::Year => "This Year",
            StatsRange::Month => "This Month",
            StatsRange::Week => "This Week",
        }
    }

    fn chart_start(self) -> &'static str {
        match self {
            StatsRange::All => "First listen",
            StatsRange::Year => "A year ago",
            StatsRange::Month => "30 days ago",
            StatsRange::Week => "7 days ago",
        }
    }
}

/// The range picker's options, the segmented control's labels, widest
/// window first.
const RANGES: &[(&str, StatsRange)] = &[
    ("All Time", StatsRange::All),
    ("This Year", StatsRange::Year),
    ("This Month", StatsRange::Month),
    ("This Week", StatsRange::Week),
];

/// The shape a rollup row's art takes: a face reads as a circle, a
/// record as a rounded square, the artist and album walls' own tells.
#[derive(Clone, Copy, PartialEq)]
enum ArtShape {
    Circle,
    Square,
}

/// The open stats window, if any: opening again focuses it instead of
/// stacking a second one, same as the settings window.
struct OpenStats(WindowHandle<Root>);

impl Global for OpenStats {}

/// Open the stats window, or bring the open one to the front. The state
/// carries the library the rollups read through, the recorder whose
/// events wake the refresh, and the shared art bake for the backdrop.
pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenStats>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The last closed window's size, sanity-floored, the tag editor's
    // restore shape.
    let (width, height) = Settings::load()
        .windows
        .stats
        .filter(|s| s.width >= 400. && s.height >= 300.)
        .map(|s| (s.width, s.height))
        .unwrap_or((640., 720.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        "rox - Stats",
        bounds,
        Some(settings_ui::MIN_SIZE),
        move |window, cx| cx.new(|cx| StatsWindow::new(state, window, cx)),
    );
    cx.set_global(OpenStats(handle));
}

/// Everything the window shows, measured whole on each refresh.
#[derive(Default)]
struct StatsData {
    /// Listens landed inside each trailing window: week, month, year,
    /// and all time. Range-independent, the page's overview.
    week: u64,
    month: u64,
    year: u64,
    total: u64,
    /// Listens inside the picked range, the page's own whole.
    range_total: u64,
    /// The chart's buckets over the range, oldest first, and the span
    /// they were cut from, so the hover readout can name a bucket's
    /// time.
    bars: Vec<u64>,
    chart_since: i64,
    bucket: i64,
    /// The range-bounded rollups and the newest listens in range.
    artists: Vec<NamePlays>,
    albums: Vec<NamePlays>,
    genres: Vec<NamePlays>,
    recents: Vec<TrackPlays>,
}

struct StatsWindow {
    /// The shared state: the library the rollups read through, the
    /// player the play controls queue on, and the art bake the backdrop
    /// paints from.
    state: AppState,
    range: StatsRange,
    data: StatsData,
    /// The bar chart's hover pick, shared with its paint and handlers.
    bar_hover: charts::BarHover,
    /// The page's scroll position, shared with the scrollbar.
    scroll: ScrollHandle,
    backdrop: WindowBackdrop,
    /// A landed listen moves every number here.
    _history_changed: Subscription,
    /// A rescan can retag tracks, which re-buckets the rollups.
    _library_changed: Subscription,
    /// Landing covers and faces notify their services; repaint so the
    /// rows fill in.
    _thumbs_changed: Subscription,
    _portraits_changed: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl StatsWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        let _portraits_changed = cx.observe(&state.portraits, |_, _, cx| cx.notify());
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs remove_window, so the frame
        // persists through the should-close hook, the tag editor's move;
        // the range writes as it is picked.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let state = s.windows.stats.get_or_insert_with(Default::default);
                state.width = frame.size.width.into();
                state.height = frame.size.height.into();
            });
            true
        });
        let range = Settings::load()
            .windows
            .stats
            .map(|s| StatsRange::from_key(&s.range))
            .unwrap_or_default();
        let mut this = StatsWindow {
            state,
            range,
            data: StatsData::default(),
            bar_hover: charts::BarHover::default(),
            scroll: ScrollHandle::new(),
            backdrop: WindowBackdrop::default(),
            _history_changed,
            _library_changed,
            _thumbs_changed,
            _portraits_changed,
            _backdrop_changed,
        };
        this.refresh(cx);
        this
    }

    /// Roll the events up whole: the recency counts over trailing
    /// windows, the chart's buckets, then the range-bounded groupings
    /// and recents.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let since = self.range.since(now);
        let library = self.state.library.read(cx);
        // The chart's span: the range's own for the bounded picks; all
        // time runs from the first listen, bucketed to land near 48
        // bars whatever the record's age.
        let (chart_since, bucket) = match self.range {
            // Six-hour buckets over a week, so the bars still read as a
            // shape rather than seven blocks.
            StatsRange::Week => (now - 7 * DAY, DAY / 4),
            StatsRange::Month => (now - 30 * DAY, DAY),
            StatsRange::Year => (now - 365 * DAY, 7 * DAY),
            StatsRange::All => match library.first_listen() {
                Some(first) if first < now => {
                    let span = (now - first).max(DAY);
                    (first, (span / 48).max(DAY))
                }
                _ => (now - 30 * DAY, DAY),
            },
        };
        self.data = StatsData {
            week: library.listens_since(now - 7 * DAY),
            month: library.listens_since(now - 30 * DAY),
            year: library.listens_since(now - 365 * DAY),
            total: library.listens_since(0),
            range_total: library.listens_since(since),
            bars: library.listen_histogram(chart_since, bucket, now),
            chart_since,
            bucket,
            artists: library.listen_rollup(Rollup::Artist, since, TOP_NAMES),
            albums: library.listen_rollup(Rollup::Album, since, TOP_NAMES),
            genres: library.listen_rollup(Rollup::Genre, since, TOP_GENRES),
            recents: library.recent_listens(since, RECENT_ROWS),
        };
        cx.notify();
    }

    fn set_range(&mut self, range: StatsRange, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
        // The pick persists as it lands, so it survives a quit that
        // never runs the close hook; the frame keeps writing on close.
        Settings::update(move |s| {
            let state = s
                .windows
                .stats
                .get_or_insert_with(StatsWindowState::default);
            state.range = range.key().into();
        });
        self.refresh(cx);
    }

    /// Queue one rollup name's library tracks on the shared player, in
    /// browse order under the queue cap. A name whose tracks are all
    /// gone resolves to nothing and queues nothing, quietly.
    fn play_name(&mut self, by: Rollup, name: &str, cx: &mut Context<Self>) {
        let ids = self
            .state
            .library
            .read(cx)
            .ids_for_rollup(by, name, QUEUE_CAP);
        let Ok(keys) = self.state.library.read(cx).keys_for(&ids) else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play(keys, cx));
    }

    /// Queue a recents row and what follows it in the list, the history
    /// panel's move. A track deleted since its event resolves to no path
    /// and drops out of the queue quietly.
    fn play_recent(&mut self, ix: usize, cx: &mut Context<Self>) {
        // The index came off a drawn frame, and refresh rebuilds recents on
        // history and library events, so a stale click can point past the
        // end; bail rather than panic.
        let Some(rows) = self.data.recents.get(ix..) else {
            return;
        };
        let ids: Vec<i64> = rows
            .iter()
            .take(QUEUE_CAP)
            .map(|row| row.track_id)
            .collect();
        let Ok(keys) = self.state.library.read(cx).keys_for(&ids) else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play(keys, cx));
    }

    /// One track path's cover through the shared thumbnail service; None
    /// while it loads, for a track with no art, and for a row whose file
    /// is gone.
    fn cover(&self, path: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        if path.is_empty() {
            return None;
        }
        match self
            .state
            .thumbs
            .update(cx, |thumbs, cx| thumbs.get(Path::new(path), cx))
        {
            Thumb::Ready(image) => Some(image),
            _ => None,
        }
    }

    /// An artist's face through the shared portrait service, the artist
    /// wall's own source; None while it looks up and for a name no
    /// service knows, where the row falls back to a cover.
    fn portrait(&self, name: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        self.state
            .portraits
            .update(cx, |portraits, cx| portraits.get(name, cx))
    }

    /// The recency overview: one card per trailing window, whatever the
    /// range knob says. The window the knob is on wears the accent, which
    /// is what ties the rest of the page to a card.
    fn listens_section(&self) -> Div {
        let cards = [
            ("This Week", self.data.week),
            ("This Month", self.data.month),
            ("This Year", self.data.year),
            ("All Time", self.data.total),
        ];
        let scoped = self.range.card();
        section(
            "Listens",
            None,
            div().flex().flex_row().gap(tokens::SPACE_SM).children(
                cards
                    .into_iter()
                    .map(|(label, count)| stat_card(label, count, label == scoped)),
            ),
        )
    }

    /// Listens over time as bars, empty stretches included, colored up
    /// the accent ramp by height. Hovering a bucket reads its count and
    /// age out in the caption row, which otherwise names the span's
    /// ends.
    fn chart_section(&self, cx: &mut Context<Self>) -> Div {
        if self.data.range_total == 0 {
            return section("Listens Over Time", None, empty_note(self.range));
        }
        let start = self.range.chart_start();
        // The hovered bucket's readout: its count and how long ago the
        // bucket began, in the caption's middle.
        let picked = self.bar_hover.index().and_then(|ix| {
            let count = *self.data.bars.get(ix)?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let began = self.data.chart_since + ix as i64 * self.data.bucket;
            let noun = if count == 1 { "listen" } else { "listens" };
            Some(format!("{count} {noun}, {}", fmt_ago(now - began)))
        });
        let chart = charts::bars(
            self.data.bars.clone(),
            &self.bar_hover,
            palette::alpha(palette::accent(), 0x59),
            palette::accent(),
            palette::highlight(),
            cx,
        );
        let body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(div().w_full().h(px(CHART_H)).child(chart))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(start)
                    .when_some(picked, |d, picked| {
                        d.child(
                            div()
                                .text_color(palette::text_secondary())
                                .child(SharedString::from(picked)),
                        )
                    })
                    .child("Now"),
            );
        section("Listens Over Time", None, body)
    }

    /// One name rollup as art-led rows: the rank, the artist's face or
    /// the record's sleeve, the name over a bar reading its share of the
    /// section's leader, and the count on the right.
    fn name_section(
        &self,
        label: &'static str,
        by: Rollup,
        rows: &[NamePlays],
        shape: ArtShape,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut body = div().flex().flex_col().gap(px(2.));
        if rows.is_empty() {
            body = body.child(empty_note(self.range));
        }
        // The bars read against the leader rather than the range's whole:
        // at ten rows out of a year of listening every bar would otherwise
        // be a sliver.
        let lead = rows.first().map_or(1, |row| row.plays).max(1);
        for (i, row) in rows.iter().enumerate() {
            let name = row.name.clone();
            // The face first, a record of theirs behind it: an artist no
            // service knows still gets a cover rather than a blank.
            let art = match shape {
                ArtShape::Circle => self
                    .portrait(&row.name, cx)
                    .or_else(|| self.cover(&row.art, cx)),
                ArtShape::Square => self.cover(&row.art, cx),
            };
            body = body.child(
                div()
                    .group(ROW_GROUP)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .hover(|d| d.bg(palette::alpha(palette::bg_control(), 0x80)))
                    .child(rank(i))
                    .child(art_frame(art, shape, &row.name, px(ART)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            // The column carries the clipping; a truncating
                            // line inside it must not, since min-width 0 on
                            // the cross axis collapses the line to its
                            // ellipsis.
                            .overflow_hidden()
                            .child(div().truncate().child(SharedString::from(row.name.clone())))
                            // The album rollup's secondary text, its album
                            // artist.
                            .when(!row.sub.is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(palette::text_secondary())
                                        .child(SharedString::from(row.sub.clone())),
                                )
                            })
                            .child(share_bar(row.plays as f32 / lead as f32)),
                    )
                    .child(play_button(
                        (label, i),
                        "Play these tracks",
                        move |this, cx| this.play_name(by, &name, cx),
                        cx,
                    ))
                    .child(plays_readout(row.plays)),
            );
        }
        section(label, None, body)
    }

    /// The genres you played most as the genre wall's own cards: each
    /// one's deterministic color under its own geometry, the name and
    /// count set on it. Clicking a card plays the genre.
    fn genre_section(&self, rows: &[NamePlays], cx: &mut Context<Self>) -> Div {
        if rows.is_empty() {
            return section("Top Genres", None, empty_note(self.range));
        }
        let mut grid = div().flex().flex_col().gap(tokens::SPACE_SM);
        for (lane, chunk) in rows.chunks(GENRE_COLS).enumerate() {
            let mut cards = div().flex().flex_row().gap(tokens::SPACE_SM);
            for (col, row) in chunk.iter().enumerate() {
                cards = cards.child(self.genre_card(lane * GENRE_COLS + col, row, cx));
            }
            // A short last lane keeps its cards card-sized rather than
            // stretching them across the row.
            for _ in chunk.len()..GENRE_COLS {
                cards = cards.child(div().flex_1().min_w_0());
            }
            grid = grid.child(cards);
        }
        section("Top Genres", None, grid)
    }

    /// One genre card: the gradient the genre grid gives that name, its
    /// motif under the text, the play glyph riding the corner on hover.
    fn genre_card(&self, ix: usize, row: &NamePlays, cx: &mut Context<Self>) -> AnyElement {
        let (color, partner) = palette::genre_color_pair(&row.name);
        let seed = palette::genre_seed(&row.name);
        let ink = palette::text_on(color);
        let name = row.name.clone();
        let played = name.clone();
        div()
            .id(("genre", ix))
            .group(CARD_GROUP)
            .flex_1()
            .min_w_0()
            .h(px(GENRE_CARD_H))
            .relative()
            .overflow_hidden()
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            // The lean is the grid's: angle off the seed, second stop the
            // genre's own drift along the wheel.
            .bg(linear_gradient(
                ((seed >> 45) % 360) as f32,
                linear_color_stop(color, 0.0),
                linear_color_stop(partner, 1.0),
            ))
            .child(motif(seed, ink))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .p(tokens::SPACE_SM)
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap(px(2.))
                    .overflow_hidden()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ink)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::alpha(ink, 0xb0))
                            .child(SharedString::from(plays_label(row.plays))),
                    ),
            )
            // Every card is the same size, so the rank is the only thing
            // saying which genre outran which.
            .child(
                div()
                    .absolute()
                    .top(tokens::SPACE_SM)
                    .left(tokens::SPACE_SM)
                    .text_xs()
                    .text_color(palette::alpha(ink, 0x8c))
                    .child(SharedString::from((ix + 1).to_string())),
            )
            .child(
                div()
                    .absolute()
                    .top(tokens::SPACE_SM)
                    .right(tokens::SPACE_SM)
                    .opacity(0.)
                    .group_hover(CARD_GROUP, |s| s.opacity(1.))
                    .child(svg().path(icons::PLAY).size(px(14.)).text_color(ink)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.play_name(Rollup::Genre, &played, cx);
            }))
            .into_any_element()
    }

    /// The newest listens in range: the cover, the title over artist and
    /// album, how long ago on the right.
    fn recents_section(&self, cx: &mut Context<Self>) -> Div {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut body = div().flex().flex_col().gap(px(2.));
        if self.data.recents.is_empty() {
            body = body.child(empty_note(self.range));
        }
        for (ix, row) in self.data.recents.iter().enumerate() {
            let sub = match (row.artist.is_empty(), row.album.is_empty()) {
                (false, false) => format!("{} - {}", row.artist, row.album),
                (false, true) => row.artist.clone(),
                (true, false) => row.album.clone(),
                (true, true) => String::new(),
            };
            let art = self.cover(&row.path, cx);
            body = body.child(
                div()
                    .group(ROW_GROUP)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .hover(|d| d.bg(palette::alpha(palette::bg_control(), 0x80)))
                    .child(art_frame(art, ArtShape::Square, &row.title, px(ROW_ART)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .truncate()
                                    .child(SharedString::from(row.title.clone())),
                            )
                            .when(!sub.is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(palette::text_secondary())
                                        .child(SharedString::from(sub)),
                                )
                            }),
                    )
                    .child(play_button(
                        ("recent", ix),
                        "Play this track",
                        move |this, cx| this.play_recent(ix, cx),
                        cx,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(fmt_ago(now - row.last_played))),
                    ),
            );
        }
        section("Recent Listens", None, body)
    }
}

/// One trailing window's card: the count large over its name, the page's
/// opening line. `scoped` marks the window the range knob is on.
fn stat_card(label: &'static str, count: u64, scoped: bool) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(tokens::SPACE_SM)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .border_1()
        .border_color(if scoped {
            palette::accent()
        } else {
            palette::border()
        })
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette::text_bright())
                .child(SharedString::from(count.to_string())),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(palette::text_muted())
                .child(label),
        )
}

/// A row's place in its rollup. The top three take the accent, which is
/// what makes a chart read as a chart at a glance.
fn rank(ix: usize) -> Div {
    div()
        .flex_none()
        .w(px(16.))
        .text_xs()
        .text_right()
        .text_color(if ix < 3 {
            palette::accent()
        } else {
            palette::text_faint()
        })
        .child(SharedString::from((ix + 1).to_string()))
}

/// A row's art: the face or sleeve when one is in hand, otherwise the
/// quiet placeholder in the same shape, so a landing image fills without
/// shifting the row. The rounding rides the image itself - gpui content
/// masks stay rectangular, so a round frame under a square image would
/// paint over its own corners.
fn art_frame(image: Option<Arc<Image>>, shape: ArtShape, name: &str, side: gpui::Pixels) -> Div {
    let round = |element: gpui::Img| match shape {
        ArtShape::Circle => element.rounded_full(),
        ArtShape::Square => element.rounded(tokens::RADIUS),
    };
    let content: AnyElement = match image {
        Some(image) => round(
            img(image)
                .size(side)
                .overflow_hidden()
                .object_fit(ObjectFit::Cover),
        )
        .into_any_element(),
        // A face falls back to its initial, a record to the music glyph:
        // a wall of identical placeholders tells you nothing.
        None => {
            let empty = div()
                .size(side)
                .flex()
                .items_center()
                .justify_center()
                .bg(palette::bg_control());
            match shape {
                ArtShape::Circle => empty
                    .rounded_full()
                    .text_color(palette::text_faint())
                    .child(SharedString::from(initial(name)))
                    .into_any_element(),
                ArtShape::Square => empty
                    .rounded(tokens::RADIUS)
                    .child(
                        svg()
                            .path(icons::MUSIC)
                            .size(side * 0.4)
                            .text_color(palette::text_faint()),
                    )
                    .into_any_element(),
            }
        }
    };
    div().flex_none().child(content)
}

/// A name's leading character, uppercased, for a face with no picture.
fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

/// How a row's count stands against its section's leader: a hairline
/// under the name, which is what turns a list into a chart.
fn share_bar(fraction: f32) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .rounded_full()
        .bg(palette::alpha(palette::text_muted(), 0x2e))
        .child(
            div()
                .h_full()
                // Even the quietest row keeps a visible stub, so the bar
                // never reads as a missing value.
                .w(relative(fraction.clamp(0.02, 1.0)))
                .rounded_full()
                .bg(palette::alpha(palette::accent(), 0xcc)),
        )
}

/// A row's play count, right of the play slot and in column with the
/// rows above and below.
fn plays_readout(plays: u64) -> Div {
    div()
        .flex_none()
        .min_w(px(30.))
        .text_right()
        .text_color(palette::text_muted())
        .child(SharedString::from(plays.to_string()))
}

/// A count with its noun, singular at one, for the genre cards.
fn plays_label(plays: u64) -> String {
    if plays == 1 {
        "1 play".to_string()
    } else {
        format!("{plays} plays")
    }
}

/// A row's play control: invisible until the row is hovered, queueing
/// on click. Every row wears the same glyph, so the tip is keyed by the
/// row's own id: a shared one would leave the whole column hovering on
/// one timer.
fn play_button(
    id: impl Into<gpui::ElementId>,
    tip: &'static str,
    on_click: impl Fn(&mut StatsWindow, &mut Context<StatsWindow>) + 'static,
    cx: &mut Context<StatsWindow>,
) -> gpui::AnyElement {
    div()
        .flex_none()
        .w(px(PLAY_SLOT_W))
        .opacity(0.)
        .group_hover(ROW_GROUP, |s| s.opacity(1.))
        .child(panel::icon_control(
            icons::PLAY,
            palette::text_muted(),
            panel::Tip::keyed(id, tip),
            on_click,
            cx,
        ))
        .into_any_element()
}

/// What a section shows before any listen lands inside the range.
fn empty_note(range: StatsRange) -> Div {
    div()
        .py(tokens::SPACE_XS)
        .text_color(palette::text_muted())
        .child(match range {
            StatsRange::All => "No listens yet",
            _ => "No listens in this range",
        })
}

impl Render for StatsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The page renders under the player's art tint like the workspace
        // that opened it, and claims the widget theme while it holds focus,
        // so the stats read in the playing track's colors.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
            let page = div()
                .flex()
                .flex_col()
                .gap(SECTION_GAP)
                .child(panel::setting_row(
                    "Range",
                    None,
                    panel::choices(
                        RANGES,
                        self.range,
                        |this: &mut Self, range, cx| this.set_range(range, cx),
                        cx,
                    ),
                ))
                .child(self.listens_section())
                .child(self.chart_section(cx))
                .child(self.name_section(
                    "Top Artists",
                    Rollup::Artist,
                    &self.data.artists,
                    ArtShape::Circle,
                    cx,
                ))
                .child(self.name_section(
                    "Top Albums",
                    Rollup::Album,
                    &self.data.albums,
                    ArtShape::Square,
                    cx,
                ))
                .child(self.genre_section(&self.data.genres, cx))
                .child(self.recents_section(cx));

            div()
                .size_full()
                .flex()
                .flex_row()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // The backdrop paints first, under the page; without it
                // translucent surfaces would sink into the window's own
                // black instead of the playing track's art.
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        .bg(palette::bg_elevated())
                        .child(
                            div()
                                .id("stats-page")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .p(tokens::SPACE_MD)
                                // Room for the scrollbar's 16px lane, so the
                                // counts and play controls never sit under
                                // the thumb.
                                .pr(tokens::SPACE_MD + px(16.))
                                .child(page),
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
