//! The stats window: one OS window opened from the menubar beside
//! Settings, the listening record rolled up per ADR 11. A range knob
//! (all time down to this week) scopes the page: the recency counts
//! as cards, listens over time as bars, then the artists, albums, and
//! genres you played most, each shown with the art its own wall uses (a
//! face in a circle, a sleeve in a square, a genre's own color card), and
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
    Stateful, Subscription, Window, WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::Root;

use rox_core::fmt::{fmt_ago, fmt_date};
use rox_core::QUEUE_CAP;
use rox_library::listens::{NamePlays, Rollup, TrackPlays};
use rox_panel_kit::motif;
use rox_playback::engine::shuffle_slice;

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

/// The art size a rollup row draws at, and the smaller square a recents
/// row uses, in px.
const ART: f32 = 40.;
const ROW_ART: f32 = 28.;

/// The bar chart's height, in px.
const CHART_H: f32 = 96.;

const DAY: i64 = 86400;

/// The hover scope for a playable row: the play control stays invisible
/// in its slot until the row is hovered, the library's rating-cell move.
const ROW_GROUP: &str = "stats-row";

/// The same for a genre card, whose play glyph is drawn in its corner.
const CARD_GROUP: &str = "stats-card";

/// The play slot's width, [`panel::icon_control`]'s footprint, reserved
/// even on rows without a control so the counts stay in column.
const PLAY_SLOT_W: f32 = 28.;

/// How far back the page counts. Trailing windows, no calendar math,
/// like the recency rows, plus one stretch picked off the chart.
#[derive(Clone, Copy, Default, PartialEq)]
enum StatsRange {
    #[default]
    All,
    Year,
    Month,
    Week,
    /// A bar's worth: the calendar day a bucket fell on, or a wider
    /// bucket's own stretch. `until` is exclusive. Never persisted; the
    /// knob's own picks come back on reopen.
    Span {
        since: i64,
        until: i64,
    },
}

impl StatsRange {
    /// The range's lower bound in unix seconds; 0 counts every event.
    fn since(self, now: i64) -> i64 {
        match self {
            StatsRange::All => 0,
            StatsRange::Year => now - 365 * DAY,
            StatsRange::Month => now - 30 * DAY,
            StatsRange::Week => now - 7 * DAY,
            StatsRange::Span { since, .. } => since,
        }
    }

    /// The range's exclusive upper bound; the trailing windows have none.
    fn until(self) -> i64 {
        match self {
            StatsRange::Span { until, .. } => until,
            _ => i64::MAX,
        }
    }

    /// The pick's key in the settings file, and the way back; an unknown
    /// key falls back to all time. A chart pick has no key.
    fn key(self) -> Option<&'static str> {
        match self {
            StatsRange::All => Some("all"),
            StatsRange::Year => Some("year"),
            StatsRange::Month => Some("month"),
            StatsRange::Week => Some("week"),
            StatsRange::Span { .. } => None,
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

    /// The overview card this range scopes the page to; a chart pick
    /// has none.
    fn card(self) -> Option<&'static str> {
        match self {
            StatsRange::All => Some(rox_i18n::t_static("stats-range-all")),
            StatsRange::Year => Some(rox_i18n::t_static("stats-range-year")),
            StatsRange::Month => Some(rox_i18n::t_static("stats-range-month")),
            StatsRange::Week => Some(rox_i18n::t_static("stats-range-week")),
            StatsRange::Span { .. } => None,
        }
    }

    /// Whether a chart pick covers one calendar day, which changes how
    /// its ends are named.
    fn single_day(self) -> bool {
        match self {
            StatsRange::Span { since, until } => fmt_date(since) == fmt_date(until - 1),
            _ => false,
        }
    }

    /// A chart pick's own segment on the knob, so the page always shows
    /// what it's scoped to: the day, or the stretch's two ends.
    fn label(self) -> Option<SharedString> {
        match self {
            StatsRange::Span { since, .. } if self.single_day() => Some(fmt_date(since).into()),
            StatsRange::Span { since, until } => Some(rox_i18n::t!(
                "stats-range-span",
                from = fmt_date(since),
                to = fmt_date(until - 1)
            )),
            _ => None,
        }
    }

    /// What the chart's left edge is called.
    fn chart_start(self) -> SharedString {
        match self {
            StatsRange::All => rox_i18n::t!("stats-chart-start-all"),
            StatsRange::Year => rox_i18n::t!("stats-chart-start-year"),
            StatsRange::Month => rox_i18n::t!("stats-chart-start-month"),
            StatsRange::Week => rox_i18n::t!("stats-chart-start-week"),
            StatsRange::Span { since, .. } => fmt_date(since).into(),
        }
    }

    /// And its right edge: now for the trailing windows and for a pick
    /// still running, else where the pick ended.
    fn chart_end(self, now: i64) -> SharedString {
        match self {
            StatsRange::Span { until, .. } if until <= now && self.single_day() => {
                rox_i18n::t!("stats-chart-end-day")
            }
            StatsRange::Span { until, .. } if until <= now => fmt_date(until - 1).into(),
            _ => rox_i18n::t!("stats-now"),
        }
    }
}

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
/// holds the library the rollups read through, the recorder whose
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
        rox_i18n::t!("stats-window-title"),
        bounds,
        Some(settings_ui::MIN_SIZE),
        move |window, cx| cx.new(|cx| StatsWindow::new(state, window, cx)),
    );
    cx.set_global(OpenStats(handle));
}

/// Everything the window shows, measured whole on each refresh.
#[derive(Default)]
struct StatsData {
    /// Listens recorded inside each trailing window: week, month, year,
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
    /// What the browsing model costs to hold: the projection's row count
    /// and the heap it occupies. Read off the shared projection, which is
    /// already in memory, so this is arithmetic over its columns' capacities
    /// rather than a measurement of anything.
    tracks: usize,
    heap_bytes: usize,
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
    /// A new listen moves every number here.
    _history_changed: Subscription,
    /// A rescan can retag tracks, which re-buckets the rollups.
    _library_changed: Subscription,
    /// Arriving covers and faces notify their services; repaint so the
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
        // the range writes as it's picked.
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
        let until = self.range.until();
        let library = self.state.library.read(cx);
        // The chart's span: the range's own for the bounded picks; all
        // time runs from the first listen, bucketed to come out near 48
        // bars whatever the record's age. The third is where the last
        // bar stands.
        let (chart_since, bucket, chart_end) = match self.range {
            // Six-hour buckets over a week, so the bars still read as a
            // shape rather than seven blocks.
            StatsRange::Week => (now - 7 * DAY, DAY / 4, now),
            StatsRange::Month => (now - 30 * DAY, DAY, now),
            StatsRange::Year => (now - 365 * DAY, 7 * DAY, now),
            StatsRange::All => match library.first_listen() {
                Some(first) if first < now => {
                    let span = (now - first).max(DAY);
                    (first, (span / 48).max(DAY), now)
                }
                _ => (now - 30 * DAY, DAY, now),
            },
            // Twenty-four bars over the stretch, an hour each for a day.
            // The last bar ends at the edge rather than starting on it.
            StatsRange::Span { since, until } => (since, ((until - since) / 24).max(60), until - 1),
        };
        self.data = StatsData {
            week: library.listens_since(now - 7 * DAY),
            month: library.listens_since(now - 30 * DAY),
            year: library.listens_since(now - 365 * DAY),
            total: library.listens_since(0),
            range_total: library.listens_between(since, until),
            bars: library.listen_histogram(chart_since, bucket, chart_end, until),
            chart_since,
            bucket,
            artists: library.listen_rollup(Rollup::Artist, since, until, TOP_NAMES),
            albums: library.listen_rollup(Rollup::Album, since, until, TOP_NAMES),
            genres: library.listen_rollup(Rollup::Genre, since, until, TOP_GENRES),
            recents: library.recent_listens(since, until, RECENT_ROWS),
            tracks: library.projection().map_or(0, |p| p.live_len()),
            heap_bytes: library.projection().map_or(0, |p| p.heap_bytes()),
        };
        cx.notify();
    }

    fn set_range(&mut self, range: StatsRange, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
        // The chart re-buckets under a pointer that hasn't moved, so the
        // old pick would name a bar the new chart may not have.
        self.bar_hover.clear();
        // The pick is written as it's made, so it persists across a quit
        // that never runs the close hook; the frame keeps writing on close.
        if let Some(key) = range.key() {
            Settings::update(move |s| {
                let state = s
                    .windows
                    .stats
                    .get_or_insert_with(StatsWindowState::default);
                state.range = key.into();
            });
        }
        self.refresh(cx);
    }

    /// Scope the page to a clicked bar: the calendar day it fell on when
    /// the buckets are a day or finer, else the bucket's own stretch, so
    /// a week bar out of the year view reads as that week and a click
    /// inside it can go on down to a day.
    fn pick_bar(&mut self, ix: usize, cx: &mut Context<Self>) {
        // The index came off a drawn frame; a stale one past the bars
        // names nothing.
        if ix >= self.data.bars.len() {
            return;
        }
        let began = self.data.chart_since + ix as i64 * self.data.bucket;
        let range = if self.data.bucket <= DAY {
            let Some((since, until)) = local_day(began) else {
                return;
            };
            StatsRange::Span { since, until }
        } else {
            StatsRange::Span {
                since: began,
                until: began + self.data.bucket,
            }
        };
        self.set_range(range, cx);
    }

    /// Queue one rollup name's library tracks on the shared player under
    /// the queue cap. An album plays in its own order; an artist or a
    /// genre plays a random draw from the whole pool, since the top row
    /// is the one you already know front to back and the same first
    /// album every press wears thin. A name whose tracks are all gone
    /// resolves to nothing and queues nothing, quietly.
    fn play_name(&mut self, by: Rollup, name: &str, cx: &mut Context<Self>) {
        let ids = match by {
            Rollup::Artist | Rollup::Genre => {
                // Cap after the shuffle, not before: a cap on the query
                // would draw from the first albums only.
                let mut ids = self
                    .state
                    .library
                    .read(cx)
                    .ids_for_rollup(by, name, usize::MAX);
                shuffle_slice(&mut ids);
                ids.truncate(QUEUE_CAP);
                ids
            }
            _ => self
                .state
                .library
                .read(cx)
                .ids_for_rollup(by, name, QUEUE_CAP),
        };
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
    /// service has, where the row falls back to a cover.
    fn portrait(&self, name: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        self.state
            .portraits
            .update(cx, |portraits, cx| portraits.get(name, cx))
    }

    /// The recency overview: one card per trailing window, whatever the
    /// range knob is set to. The window the knob is on takes the accent,
    /// which ties the rest of the page to a card.
    fn listens_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let cards = [
            (
                rox_i18n::t_static("stats-range-week"),
                self.data.week,
                StatsRange::Week,
            ),
            (
                rox_i18n::t_static("stats-range-month"),
                self.data.month,
                StatsRange::Month,
            ),
            (
                rox_i18n::t_static("stats-range-year"),
                self.data.year,
                StatsRange::Year,
            ),
            (
                rox_i18n::t_static("stats-range-all"),
                self.data.total,
                StatsRange::All,
            ),
        ];
        let scoped = self.range.card();
        // What the browsing model weighs, beside the counts it feeds. The
        // page is otherwise all record and no cost, and the cost is the
        // one number a big library wants stated plainly.
        let held = (self.data.tracks > 0).then(|| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!(
                    "stats-library-held",
                    tracks = rox_i18n::format::format_int(self.data.tracks as i64),
                    size = heap_size(self.data.heap_bytes),
                ))
                .into_any_element()
        });
        section(
            rox_i18n::t!("stats-section-listens"),
            held,
            div().flex().flex_row().gap(tokens::SPACE_SM).children(
                cards
                    .into_iter()
                    .enumerate()
                    .map(|(i, (label, count, range))| {
                        stat_card(i, label, count, range, Some(label) == scoped, cx)
                    }),
            ),
        )
    }

    /// Listens over time as bars, empty stretches included, colored up
    /// the accent ramp by height. Hovering a bucket reads its count and
    /// age out in the caption row, which otherwise names the span's
    /// ends.
    fn chart_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        if self.data.range_total == 0 {
            return section(
                rox_i18n::t!("stats-section-listens-over-time"),
                None,
                empty_note(self.range),
            );
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let start = self.range.chart_start();
        let end = self.range.chart_end(now);
        // A day's hour bars are the floor: nothing narrower to open, so
        // the chart stops offering.
        let pickable = !self.range.single_day();
        // The hovered bucket's readout: its count, how long ago the bucket
        // began, and the calendar day that was, in the caption's middle.
        // Under a day the bars need the clock too.
        let picked = self.bar_hover.index().and_then(|ix| {
            let count = *self.data.bars.get(ix)?;
            let began = self.data.chart_since + ix as i64 * self.data.bucket;
            let ago = fmt_ago(now - began);
            let date = if self.data.bucket < DAY {
                fmt_stamp(began)
            } else {
                fmt_date(began)
            };
            Some(
                rox_i18n::t!(
                    "stats-bucket-listens",
                    count = count,
                    ago = ago,
                    date = date
                )
                .to_string(),
            )
        });
        let chart = charts::bars(
            self.data.bars.clone(),
            &self.bar_hover,
            palette::alpha(palette::accent(), 0x59),
            palette::accent(),
            palette::highlight(),
            pickable.then_some(|this: &mut Self, ix: usize, cx: &mut Context<Self>| {
                this.pick_bar(ix, cx)
            }),
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
                    .child(end),
            );
        section(
            rox_i18n::t!("stats-section-listens-over-time"),
            Some(bars_note(self.data.bucket, pickable)),
            body,
        )
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
    ) -> Stateful<Div> {
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
            // service has still gets a cover rather than a blank.
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
                            // The column does the clipping; a truncating
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
                        rox_i18n::t_static("stats-play-these-tracks"),
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
    fn genre_section(&self, rows: &[NamePlays], cx: &mut Context<Self>) -> Stateful<Div> {
        if rows.is_empty() {
            return section(
                rox_i18n::t!("stats-section-top-genres"),
                None,
                empty_note(self.range),
            );
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
        section(rox_i18n::t!("stats-section-top-genres"), None, grid)
    }

    /// One genre card: the gradient the genre grid gives that name, its
    /// motif under the text, the play glyph in the corner on hover.
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
            // showing which genre outran which.
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
    fn recents_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
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
                        rox_i18n::t_static("stats-play-this-track"),
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
        section(rox_i18n::t!("stats-section-recent-listens"), None, body)
    }
}

/// A heap figure as MB or GB, one decimal. The projection is tens of MB
/// at the scale the ADRs were sized for and near a gigabyte at ten
/// million tracks, so those are the only two units this ever needs.
fn heap_size(bytes: usize) -> String {
    let mb = bytes as f64 / 1_000_000.;
    if mb < 1000. {
        rox_i18n::format::format_unit(mb, 1, "MB")
    } else {
        rox_i18n::format::format_unit(mb / 1000., 1, "GB")
    }
}

/// A unix second as the locale's date and wall-clock time, for a bar
/// finer than a day.
fn fmt_stamp(unix: i64) -> String {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let Some(local) = Local.timestamp_opt(unix, 0).single() else {
        return String::new();
    };
    rox_i18n::format::format_datetime(
        local.year(),
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
    )
}

/// What a bar stands for and what a click on one does, in the chart's
/// corner, so the level the page is at reads without guessing and the
/// floor announces itself.
fn bars_note(bucket: i64, pickable: bool) -> AnyElement {
    const HOUR: i64 = 3600;
    let text = if !pickable {
        rox_i18n::t!("stats-bars-hourly")
    } else if bucket < DAY {
        let hours = ((bucket + HOUR / 2) / HOUR).max(1) as u64;
        rox_i18n::t!("stats-bars-hours", hours = hours)
    } else if bucket == DAY {
        rox_i18n::t!("stats-bars-daily")
    } else if bucket == 7 * DAY {
        rox_i18n::t!("stats-bars-weekly")
    } else {
        let days = ((bucket + DAY / 2) / DAY) as u64;
        rox_i18n::t!("stats-bars-days", days = days)
    };
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
        .into_any_element()
}

/// The local calendar day around a unix second: its midnight and the
/// next, so a chart pick means the day the clock showed rather than 24
/// hours off a bucket edge. None for a second chrono can't place.
fn local_day(unix: i64) -> Option<(i64, i64)> {
    use chrono::{Days, Local, NaiveDate, NaiveTime, TimeZone};
    let date = Local.timestamp_opt(unix, 0).single()?.date_naive();
    let midnight = |date: NaiveDate| {
        date.and_time(NaiveTime::MIN)
            .and_local_timezone(Local)
            .earliest()
            .map(|t| t.timestamp())
    };
    Some((
        midnight(date)?,
        midnight(date.checked_add_days(Days::new(1))?)?,
    ))
}

/// One trailing window's card: the count large over its name, the page's
/// opening line. `scoped` marks the window the range knob is on; a click
/// moves the knob there.
fn stat_card(
    ix: usize,
    label: &'static str,
    count: u64,
    range: StatsRange,
    scoped: bool,
    cx: &mut Context<StatsWindow>,
) -> Stateful<Div> {
    div()
        .id(("stats-card", ix))
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
        .cursor_pointer()
        .when(!scoped, |d| {
            d.hover(|d| d.border_color(palette::alpha(palette::accent(), 0x80)))
        })
        .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette::text_bright())
                .child(SharedString::from(rox_i18n::format::format_int(
                    count as i64,
                ))),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(palette::text_muted())
                .child(label),
        )
}

/// A row's place in its rollup. The top three take the accent, which
/// makes a chart read as a chart at a glance.
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
/// quiet placeholder in the same shape, so an arriving image fills without
/// shifting the row. The rounding is applied to the image itself: gpui
/// content masks stay rectangular, so a round frame under a square image
/// would paint over its own corners. The square box around it does the
/// cropping, since `Cover` overruns the image element on the art's long
/// side and the image can't mask its own overrun.
fn art_frame(image: Option<Arc<Image>>, shape: ArtShape, name: &str, side: gpui::Pixels) -> Div {
    let round = |element: gpui::Img| match shape {
        ArtShape::Circle => element.rounded_full(),
        ArtShape::Square => element.rounded(tokens::RADIUS),
    };
    let content: AnyElement = match image {
        Some(image) => div()
            .size(side)
            .overflow_hidden()
            .child(round(img(image).size_full().object_fit(ObjectFit::Cover)))
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
/// under the name, which turns a list into a chart.
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
        .child(SharedString::from(rox_i18n::format::format_int(
            plays as i64,
        )))
}

/// A count with its noun, singular at one, for the genre cards.
fn plays_label(plays: u64) -> String {
    rox_i18n::t!("stats-plays-count", count = plays).to_string()
}

/// A row's play control: invisible until the row is hovered, queueing
/// on click. Every row uses the same glyph, so the tip is keyed by the
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
            StatsRange::All => rox_i18n::t!("stats-empty-all"),
            _ => rox_i18n::t!("stats-empty-range"),
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
            // The range scopes every section under it, so it holds its own
            // bar at the top rather than scrolling away with the first one.
            // The right inset matches the page's scrollbar lane, so the
            // picker lines up with the sections beneath it.
            // A chart pick joins the knob as its own segment, the one
            // place the page names what it's scoped to.
            let mut options = vec![
                (rox_i18n::t!("stats-range-all"), StatsRange::All),
                (rox_i18n::t!("stats-range-year"), StatsRange::Year),
                (rox_i18n::t!("stats-range-month"), StatsRange::Month),
                (rox_i18n::t!("stats-range-week"), StatsRange::Week),
            ];
            if let Some(label) = self.range.label() {
                options.push((label, self.range));
            }
            let range = div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .pl(tokens::SPACE_MD)
                .pr(tokens::SPACE_MD + px(16.))
                .py(tokens::SPACE_SM)
                .border_b_1()
                .border_color(palette::border())
                .child(
                    panel::setting_row(
                        rox_i18n::t!("stats-range-label"),
                        None,
                        panel::choices_shared(
                            &options,
                            self.range,
                            |this: &mut Self, range, cx| this.set_range(range, cx),
                            cx,
                        ),
                    )
                    .flex_1()
                    .min_w_0(),
                );
            let page = div()
                .flex()
                .flex_col()
                .gap(SECTION_GAP)
                .child(self.listens_section(cx))
                .child(self.chart_section(cx))
                .child(self.name_section(
                    rox_i18n::t_static("stats-section-top-artists"),
                    Rollup::Artist,
                    &self.data.artists,
                    ArtShape::Circle,
                    cx,
                ))
                .child(self.name_section(
                    rox_i18n::t_static("stats-section-top-albums"),
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
                        .flex()
                        .flex_col()
                        .bg(palette::bg_elevated())
                        .child(range)
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .relative()
                                .child(
                                    div()
                                        .id("stats-page")
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.scroll)
                                        .p(tokens::SPACE_MD)
                                        // Room for the scrollbar's 16px lane,
                                        // so the counts and play controls
                                        // never end up under the thumb.
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
                        ),
                )
                .into_any_element()
        })
    }
}
