//! The stats window: one OS window opened from the menubar beside
//! Settings, the listening record rolled up per ADR 11. A range knob
//! (all time, this year, this month) scopes the page: listens over time
//! as bars, artist and genre breakdowns as donuts, album rollups, and
//! the newest listens. Everything derives from the events table by SQL
//! on the shared catalog's connection; nothing counts along the way.
//! Rollups read entering the window and when a listen lands or the
//! catalog changes, never per frame; the charts are gpui paths and
//! quads, cheap at this scale.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Div, Global, Rgba, ScrollHandle, SharedString,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::Root;

use rox_library::listens::{NamePlays, Rollup, TrackPlays};

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::charts;
use crate::design::{palette, tokens};
use crate::history::HistoryEvent;
use crate::panel::{self, AppState};
use crate::panels::history::fmt_ago;
use crate::panels::library::{LibraryEvent, QUEUE_CAP};
use crate::settings::{Settings, StatsWindowState};
use crate::settings_ui::{self, section, SECTION_GAP};

/// How many rows the album rollup and the recents show.
const TOP_NAMES: usize = 15;
const RECENT_ROWS: usize = 15;

/// How many named slices a donut carries before the rest pools into
/// "other"; the legend mirrors the slices one for one.
const DONUT_SLICES: usize = 6;

/// The donut's side and the bar chart's height, in px.
const DONUT_SIDE: f32 = 132.;
const CHART_H: f32 = 96.;

/// The named slices' alpha ramp over the accent, brightest first; the
/// accent hue keeps the charts inside song theming.
const SLICE_ALPHAS: [u8; DONUT_SLICES] = [0xe6, 0xc0, 0x9a, 0x74, 0x52, 0x38];

const DAY: i64 = 86400;

/// The hover scope for a playable row: the play control sits invisible
/// in its slot until the row is hovered, the library's rating-cell move.
const ROW_GROUP: &str = "stats-row";

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
}

impl StatsRange {
    /// The range's lower bound in unix seconds; 0 counts every event.
    fn since(self, now: i64) -> i64 {
        match self {
            StatsRange::All => 0,
            StatsRange::Year => now - 365 * DAY,
            StatsRange::Month => now - 30 * DAY,
        }
    }

    /// The pick's key in the settings file, and the way back; an unknown
    /// key falls back to all time.
    fn key(self) -> &'static str {
        match self {
            StatsRange::All => "all",
            StatsRange::Year => "year",
            StatsRange::Month => "month",
        }
    }

    fn from_key(key: &str) -> StatsRange {
        match key {
            "year" => StatsRange::Year,
            "month" => StatsRange::Month,
            _ => StatsRange::All,
        }
    }
}

/// The range picker's options, the segmented control's labels.
const RANGES: &[(&str, StatsRange)] = &[
    ("All Time", StatsRange::All),
    ("This Year", StatsRange::Year),
    ("This Month", StatsRange::Month),
];

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
        .stats_window
        .filter(|s| s.width >= 400. && s.height >= 300.)
        .map(|s| (s.width, s.height))
        .unwrap_or((640., 720.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - Stats".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - Stats");
            let view = cx.new(|cx| StatsWindow::new(state, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the stats window");
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
    /// Listens inside the picked range, the donuts' whole.
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
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs remove_window, so the frame
        // persists through the should-close hook, the tag editor's move;
        // the range writes as it is picked.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let state = s.stats_window.get_or_insert_with(Default::default);
                state.width = frame.size.width.into();
                state.height = frame.size.height.into();
            });
            true
        });
        let range = Settings::load()
            .stats_window
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
            genres: library.listen_rollup(Rollup::Genre, since, TOP_NAMES),
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
            let state = s.stats_window.get_or_insert_with(StatsWindowState::default);
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
        let Ok(paths) = self.state.library.read(cx).paths_for(&ids) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play(paths, cx));
    }

    /// Queue a recents row and what follows it in the list, the history
    /// panel's move. A track deleted since its event resolves to no path
    /// and drops out of the queue quietly.
    fn play_recent(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.data.recents[ix..]
            .iter()
            .take(QUEUE_CAP)
            .map(|row| row.track_id)
            .collect();
        let Ok(paths) = self.state.library.read(cx).paths_for(&ids) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play(paths, cx));
    }

    /// The recency overview: one row per trailing window, whatever the
    /// range knob says.
    fn listens_section(&self) -> Div {
        let rows = [
            ("This Week", self.data.week),
            ("This Month", self.data.month),
            ("This Year", self.data.year),
            ("All Time", self.data.total),
        ];
        section(
            "Listens",
            None,
            div()
                .flex()
                .flex_col()
                .children(rows.into_iter().map(|(label, count)| {
                    stat_row(
                        div().child(label).into_any_element(),
                        count.to_string(),
                        None,
                    )
                })),
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
        let start = match self.range {
            StatsRange::All => "First listen",
            StatsRange::Year => "A year ago",
            StatsRange::Month => "30 days ago",
        };
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

    /// One donut breakdown: the top names' share of the range's listens
    /// as ring slices, the legend mirroring them one for one, the rest
    /// pooled into "other".
    fn donut_section(
        &self,
        label: &'static str,
        by: Rollup,
        rows: &[NamePlays],
        cx: &mut Context<Self>,
    ) -> Div {
        if rows.is_empty() {
            return section(label, None, empty_note(self.range));
        }
        let total = self.data.range_total.max(1);
        let named: Vec<&NamePlays> = rows.iter().take(DONUT_SLICES).collect();
        let named_plays: u64 = named.iter().map(|row| row.plays).sum();
        let other = total.saturating_sub(named_plays);

        let mut slices: Vec<(f32, Rgba)> = named
            .iter()
            .enumerate()
            .map(|(i, row)| {
                (
                    row.plays as f32 / total as f32,
                    palette::alpha(palette::accent(), SLICE_ALPHAS[i]),
                )
            })
            .collect();
        let other_color = palette::alpha(palette::text_muted(), 0x3c);
        if other > 0 {
            slices.push((other as f32 / total as f32, other_color));
        }

        let mut legend = div().flex_1().min_w_0().flex().flex_col();
        for (i, row) in named.iter().enumerate() {
            let name = row.name.clone();
            legend = legend.child(stat_row(
                legend_label(
                    palette::alpha(palette::accent(), SLICE_ALPHAS[i]),
                    row.name.clone(),
                ),
                row.plays.to_string(),
                Some(play_button(
                    move |this, cx| this.play_name(by, &name, cx),
                    cx,
                )),
            ));
        }
        if other > 0 {
            legend = legend.child(stat_row(
                legend_label(other_color, "Other".into()),
                other.to_string(),
                Some(play_spacer()),
            ));
        }

        let donut = div()
            .flex_none()
            .w(px(DONUT_SIDE))
            .h(px(DONUT_SIDE))
            .child(charts::donut(slices));
        section(
            label,
            None,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .child(donut)
                .child(legend),
        )
    }

    /// One name rollup as plain rows with their counts inside the range.
    fn name_section(
        &self,
        label: &'static str,
        by: Rollup,
        rows: &[NamePlays],
        cx: &mut Context<Self>,
    ) -> Div {
        let mut body = div().flex().flex_col();
        if rows.is_empty() {
            body = body.child(empty_note(self.range));
        }
        for row in rows {
            let name = row.name.clone();
            let label_el = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(row.name.clone())),
                )
                // The album rollup's secondary text, its album artist.
                .when(!row.sub.is_empty(), |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::text_secondary())
                            .child(SharedString::from(row.sub.clone())),
                    )
                });
            body = body.child(stat_row(
                label_el.into_any_element(),
                row.plays.to_string(),
                Some(play_button(
                    move |this, cx| this.play_name(by, &name, cx),
                    cx,
                )),
            ));
        }
        section(label, None, body)
    }

    /// The newest listens in range: title and artist - album per row,
    /// how long ago on the right.
    fn recents_section(&self, cx: &mut Context<Self>) -> Div {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut body = div().flex().flex_col();
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
            let label = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(row.title.clone())),
                )
                .when(!sub.is_empty(), |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::text_secondary())
                            .child(SharedString::from(sub)),
                    )
                });
            body = body.child(stat_row(
                label.into_any_element(),
                fmt_ago(now - row.last_played),
                Some(play_button(move |this, cx| this.play_recent(ix, cx), cx)),
            ));
        }
        section("Recent Listens", None, body)
    }
}

/// One row of a rollup: its label filling left, the readout right, and
/// a play slot between when the row queues something - the control
/// surfacing on row hover, the slot holding its width either way so
/// the readouts stay in column.
fn stat_row(label: gpui::AnyElement, readout: String, play: Option<gpui::AnyElement>) -> Div {
    div()
        .group(ROW_GROUP)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .border_b_1()
        .border_color(palette::border())
        .child(div().flex_1().min_w_0().child(label))
        .children(play)
        .child(
            div()
                .flex_none()
                .text_right()
                .text_color(palette::text_muted())
                .child(readout),
        )
}

/// A row's play control: invisible until the row is hovered, queueing
/// on click.
fn play_button(
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
            on_click,
            cx,
        ))
        .into_any_element()
}

/// The play slot's stand-in for rows with nothing to queue (the donuts'
/// "other" pool), so their readouts line up with their neighbors'.
fn play_spacer() -> gpui::AnyElement {
    div().flex_none().w(px(PLAY_SLOT_W)).into_any_element()
}

/// A legend row's label: the slice's swatch leading its name.
fn legend_label(color: Rgba, name: String) -> gpui::AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(div().flex_none().size(px(10.)).rounded(px(2.)).bg(color))
        .child(div().min_w_0().truncate().child(SharedString::from(name)))
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
            .child(self.donut_section("Top Artists", Rollup::Artist, &self.data.artists, cx))
            .child(self.name_section("Top Albums", Rollup::Album, &self.data.albums, cx))
            .child(self.donut_section("Top Genres", Rollup::Genre, &self.data.genres, cx))
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
                    // Always visible, not fading in on scroll: the thumb
                    // is what says more page hangs below the fold.
                    .child(div().absolute().inset_0().child(
                        Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                    )),
            )
    }
}
