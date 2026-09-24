//! The stats window: the listening record rolled up per ADR 11. A range knob
//! scopes the page: recency counts, listens over time, the top artists,
//! albums and genres, and the newest listens. Everything derives from the
//! events table by SQL; nothing counts along the way. Rollups read on open
//! and when a listen lands or the catalog changes, never per frame.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, FontWeight, Global, Image, ObjectFit,
    ScrollHandle, SharedString, Stateful, Subscription, Window, WindowHandle, div, img,
    linear_color_stop, linear_gradient, prelude::*, px, relative, size, svg,
};
use gpui_component::Root;
use gpui_component::scroll::Scrollbar;
use gpui_component::tooltip::Tooltip;

use rox_core::QUEUE_CAP;
use rox_core::fmt::{fmt_ago, fmt_date};
use rox_library::listens::{self, NamePlays, Rollup, TrackPlays};
use rox_panel_kit::motif;
use rox_playback::engine::shuffle_slice;

use rox_core::settings::{Settings, StatsWindowState};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::charts;
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, dialog_button, section};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::{LibraryEvent, LocalCopy};
use rox_services::history::HistoryEvent;
use rox_services::thumbs::Thumb;

const TOP_NAMES: usize = 10;
const TOP_GENRES: usize = 9;
const RECENT_ROWS: usize = 15;

const GENRE_COLS: usize = 3;
const GENRE_CARD_H: f32 = 76.;

const ART: f32 = 40.;
const ROW_ART: f32 = 28.;

const CHART_H: f32 = 96.;

const DAY: i64 = 86400;

/// The play control stays invisible until its row is hovered.
const ROW_GROUP: &str = "stats-row";

const CARD_GROUP: &str = "stats-card";

/// Reserved even on rows without a control so the counts stay in column.
const PLAY_SLOT_W: f32 = 28.;

/// Trailing windows, no calendar math, plus one stretch picked off the chart.
#[derive(Clone, Copy, Default, PartialEq)]
enum StatsRange {
    #[default]
    All,
    Year,
    Month,
    Week,
    /// A clicked bar's stretch. `until` is exclusive. Never persisted.
    Span {
        since: i64,
        until: i64,
    },
}

impl StatsRange {
    fn since(self, now: i64) -> i64 {
        match self {
            StatsRange::All => 0,
            StatsRange::Year => now - 365 * DAY,
            StatsRange::Month => now - 30 * DAY,
            StatsRange::Week => now - 7 * DAY,
            StatsRange::Span { since, .. } => since,
        }
    }

    fn until(self) -> i64 {
        match self {
            StatsRange::Span { until, .. } => until,
            _ => i64::MAX,
        }
    }

    /// An unknown key falls back to all time. A chart pick has no key.
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

    fn card(self) -> Option<&'static str> {
        match self {
            StatsRange::All => Some(rox_i18n::t_static("stats-range-all")),
            StatsRange::Year => Some(rox_i18n::t_static("stats-range-year")),
            StatsRange::Month => Some(rox_i18n::t_static("stats-range-month")),
            StatsRange::Week => Some(rox_i18n::t_static("stats-range-week")),
            StatsRange::Span { .. } => None,
        }
    }

    fn single_day(self) -> bool {
        match self {
            StatsRange::Span { since, until } => fmt_date(since) == fmt_date(until - 1),
            _ => false,
        }
    }

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

    fn chart_start(self) -> SharedString {
        match self {
            StatsRange::All => rox_i18n::t!("stats-chart-start-all"),
            StatsRange::Year => rox_i18n::t!("stats-chart-start-year"),
            StatsRange::Month => rox_i18n::t!("stats-chart-start-month"),
            StatsRange::Week => rox_i18n::t!("stats-chart-start-week"),
            StatsRange::Span { since, .. } => fmt_date(since).into(),
        }
    }

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

#[derive(Clone, Copy, PartialEq)]
enum ArtShape {
    Circle,
    Square,
}

struct OpenStats(WindowHandle<Root>);

impl Global for OpenStats {}

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

#[derive(Default)]
struct StatsData {
    week: u64,
    month: u64,
    year: u64,
    total: u64,
    range_total: u64,
    /// Range-independent: the clear doesn't care what the knob says.
    imported: u64,
    bars: Vec<u64>,
    chart_since: i64,
    bucket: i64,
    /// Arithmetic over the shared projection's capacities, not a measurement.
    tracks: usize,
    heap_bytes: usize,
    artists: Vec<NamePlays>,
    albums: Vec<NamePlays>,
    genres: Vec<NamePlays>,
    recents: Vec<TrackPlays>,
    /// The library's own copy of each live recent's song, since a radio row is
    /// the station. Supplies the cover and what the play button queues.
    recent_files: Vec<Option<LocalCopy>>,
}

struct StatsWindow {
    state: AppState,
    range: StatsRange,
    data: StatsData,
    bar_hover: charts::BarHover,
    /// The button only raises the question; the dialog does the deleting.
    clearing: bool,
    dialog_focus: FocusHandle,
    scroll: ScrollHandle,
    backdrop: WindowBackdrop,
    _history_changed: Subscription,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _portraits_changed: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own wake.
    _backdrop_changed: Subscription,
}

impl StatsWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        // A play-count import or a rescan moves every number here.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated | LibraryEvent::PlaysReloaded) {
                    this.refresh(cx);
                }
            },
        );
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        let _portraits_changed = cx.observe(&state.portraits, |_, _, cx| cx.notify());
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs remove_window, so persist the frame here.
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
            clearing: false,
            dialog_focus: cx.focus_handle(),
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let since = self.range.since(now);
        let until = self.range.until();
        let library = self.state.library.read(cx);
        // All time runs from the first listen, bucketed to about 48 bars whatever
        // the record's age.
        let (chart_since, bucket, chart_end) = match self.range {
            // Six-hour buckets, so a week reads as a shape rather than seven blocks.
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
            // The last bar ends at the edge rather than starting on it.
            StatsRange::Span { since, until } => (since, ((until - since) / 24).max(60), until - 1),
        };
        let recents = library.recent_listens(since, until, RECENT_ROWS);
        let names: Vec<(&str, &str)> = recents
            .iter()
            .map(|row| match row.live {
                true => (row.artist.as_str(), row.title.as_str()),
                false => ("", ""),
            })
            .collect();
        let recent_files = library.local_copies(&names);

        self.data = StatsData {
            week: library.listens_since(now - 7 * DAY),
            month: library.listens_since(now - 30 * DAY),
            year: library.listens_since(now - 365 * DAY),
            total: library.listens_since(0),
            range_total: library.listens_between(since, until),
            imported: library.listens_tally().imported,
            bars: library.listen_histogram(chart_since, bucket, chart_end, until),
            chart_since,
            bucket,
            artists: library.listen_rollup(Rollup::Artist, since, until, TOP_NAMES),
            albums: library.listen_rollup(Rollup::Album, since, until, TOP_NAMES),
            genres: library.listen_rollup(Rollup::Genre, since, until, TOP_GENRES),
            recents,
            recent_files,
            tracks: library.projection().map_or(0, |p| p.browse_len()),
            heap_bytes: library.projection().map_or(0, |p| p.heap_bytes()),
        };
        cx.notify();
    }

    fn set_range(&mut self, range: StatsRange, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
        // The old hover would name a bar the re-bucketed chart may not have.
        self.bar_hover.clear();
        // Written as picked, since a quit never runs the close hook.
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

    fn clear_listens(&mut self, what: listens::Clear, cx: &mut Context<Self>) {
        self.clearing = false;
        self.state
            .library
            .update(cx, |library, cx| library.clear_listens(what, cx));
        cx.notify();
    }

    /// Two answers when there's imported history: what the import wrote, or
    /// everything.
    fn clear_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.clearing {
            return None;
        }
        // Only refocus when focus is outside, or Tab could never reach the buttons.
        if !self.dialog_focus.contains_focused(window, cx) {
            window.focus(&self.dialog_focus);
        }
        // With nothing imported, the split would offer to clear none then all.
        let split = self.data.imported > 0;
        let body = if split {
            rox_i18n::t!(
                "listens-clear-body",
                imported = self.data.imported,
                total = self.data.total
            )
        } else {
            rox_i18n::t!(
                "listens-clear-body-plain",
                listens = rox_i18n::t!("listens-count", count = self.data.total).to_string()
            )
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .track_focus(&self.dialog_focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    if event.keystroke.key == "escape" {
                        this.clearing = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .w(px(380.))
                        .p(tokens::SPACE_MD)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(div().child(rox_i18n::t!("listens-clear-title")))
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(body),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(tokens::SPACE_SM)
                                .child(dialog_button(
                                    rox_i18n::t!("settings-common-cancel"),
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.clearing = false;
                                        cx.notify();
                                    }),
                                ))
                                .children(split.then(|| {
                                    dialog_button(
                                        rox_i18n::t!("listens-clear-imported"),
                                        false,
                                        cx.listener(|this, _, _, cx| {
                                            this.clear_listens(listens::Clear::Imported, cx)
                                        }),
                                    )
                                }))
                                .child(dialog_button(
                                    if split {
                                        rox_i18n::t!("listens-clear-everything")
                                    } else {
                                        rox_i18n::t!("settings-confirm-clear")
                                    },
                                    true,
                                    cx.listener(|this, _, _, cx| {
                                        this.clear_listens(listens::Clear::Everything, cx)
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// A day or finer scopes to the calendar day, else to the bucket's stretch,
    /// so a click can drill down from a week to a day.
    fn pick_bar(&mut self, ix: usize, cx: &mut Context<Self>) {
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

    /// An album plays in order; an artist or genre plays a random draw from
    /// the whole pool.
    fn play_name(&mut self, by: Rollup, name: &str, cx: &mut Context<Self>) {
        let ids = match by {
            Rollup::Artist | Rollup::Genre => {
                // Cap after the shuffle, or the draw only reaches the first albums.
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

    fn play_recent(&mut self, ix: usize, cx: &mut Context<Self>) {
        // A stale click can point past the end after a refresh.
        let Some(rows) = self.data.recents.get(ix..) else {
            return;
        };
        // Never queue the station for a radio row: it would play what's on air
        // now, not the song. The library's copy wins where it has one.
        let local = self
            .data
            .recent_files
            .get(ix)
            .and_then(|local| local.as_ref());
        if let Some(local) = local {
            let Ok(keys) = self.state.library.read(cx).keys_for(&[local.track_id]) else {
                return;
            };
            if keys.is_empty() {
                return;
            }
            self.state
                .player
                .update(cx, |player, cx| player.play(keys, cx));
            return;
        }

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

    fn portrait(&self, name: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        self.state
            .portraits
            .update(cx, |portraits, cx| portraits.get(name, cx))
    }

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
        // A day's hour bars are the floor, so the chart stops offering picks.
        let pickable = !self.range.single_day();
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
        // Against the leader, or at ten rows out of a year every bar is a sliver.
        let lead = rows.first().map_or(1, |row| row.plays).max(1);
        for (i, row) in rows.iter().enumerate() {
            let name = row.name.clone();
            // An artist with no face still gets a cover.
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
                            // The column clips; a truncating line inside must not, or min-width 0
                            // collapses it to its ellipsis.
                            .overflow_hidden()
                            .child(div().truncate().child(SharedString::from(row.name.clone())))
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
            for _ in chunk.len()..GENRE_COLS {
                cards = cards.child(div().flex_1().min_w_0());
            }
            grid = grid.child(cards);
        }
        section(rox_i18n::t!("stats-section-top-genres"), None, grid)
    }

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
            // The song's own cover first, then the station's favicon, keyed on the
            // row's own path.
            let local = self.data.recent_files.get(ix).and_then(|l| l.as_ref());
            let art = local
                .and_then(|local| self.cover(&local.path.to_string_lossy(), cx))
                .or_else(|| self.cover(&row.path, cx));
            let plays = match local.is_some() {
                true => rox_i18n::t_static("history-live-plays-file"),
                false => rox_i18n::t_static("history-live-plays-station"),
            };
            body = body.child(
                div()
                    .id(("recent-row", ix))
                    .group(ROW_GROUP)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .hover(|d| d.bg(palette::alpha(palette::bg_control(), 0x80)))
                    .when(row.live, |d| {
                        d.tooltip(move |window, cx| Tooltip::new(plays).build(window, cx))
                    })
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
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::SPACE_XS)
                                    .overflow_hidden()
                                    .when(row.live, |d| {
                                        d.child(
                                            svg()
                                                .path(icons::RADIO)
                                                .size(px(12.))
                                                .flex_none()
                                                .text_color(palette::text_muted()),
                                        )
                                    })
                                    .child(
                                        div()
                                            .truncate()
                                            .child(SharedString::from(row.title.clone())),
                                    ),
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

/// The projection is tens of MB at the scale the ADRs target and near a
/// gigabyte at ten million tracks, so MB and GB are all this needs.
fn heap_size(bytes: usize) -> String {
    let mb = bytes as f64 / 1_000_000.;
    if mb < 1000. {
        rox_i18n::format::format_unit(mb, 1, "MB")
    } else {
        rox_i18n::format::format_unit(mb / 1000., 1, "GB")
    }
}

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

/// Midnight to midnight, so a pick is the day the clock showed rather than
/// 24 hours off a bucket edge.
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

/// The image does its own rounding, since gpui content masks stay
/// rectangular; the square box crops `Cover`'s overrun.
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

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

fn share_bar(fraction: f32) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .rounded_full()
        .bg(palette::alpha(palette::text_muted(), 0x2e))
        .child(
            div()
                .h_full()
                // A visible stub, so the bar never reads as a missing value.
                .w(relative(fraction.clamp(0.02, 1.0)))
                .rounded_full()
                .bg(palette::alpha(palette::accent(), 0xcc)),
        )
}

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

fn plays_label(plays: u64) -> String {
    rox_i18n::t!("stats-plays-count", count = plays).to_string()
}

/// The tip is keyed by the row's id: a shared key would put the whole
/// column on one hover timer.
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

/// Stretched rather than sized, so it matches the knob's height whatever
/// the font or scale.
fn clear_button(inert: bool, cx: &mut Context<StatsWindow>) -> Stateful<Div> {
    let mut button = settings_ui::icon_button(
        icons::BROOM,
        inert,
        cx.listener(|this: &mut StatsWindow, _, _, cx| {
            this.clearing = true;
            cx.notify();
        }),
    )
    .filled()
    .flex()
    .items_center()
    .justify_center()
    .px(tokens::SPACE_SM);
    button.style().align_self = Some(gpui::AlignSelf::Stretch);
    let label = rox_i18n::t!("listens-clear-button");
    let mut lane = div()
        .id("stats-clear-history")
        .flex()
        .flex_none()
        .pl(tokens::SPACE_SM)
        .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        .child(button);
    lane.style().align_self = Some(gpui::AlignSelf::Stretch);
    lane
}

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
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
            // The range bar stays fixed above the scrolling page, with the page's own
            // insets since nothing in it scrolls.
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
                .pr(tokens::SPACE_MD)
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
                )
                .child(clear_button(self.data.total == 0, cx));
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
                                        // Room for the scrollbar's 16px lane.
                                        .pr(tokens::SPACE_MD + px(16.))
                                        .child(page),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .child(Scrollbar::vertical(&self.scroll)),
                                ),
                        ),
                )
                .children(self.clear_overlay(window, cx))
                .into_any_element()
        })
    }
}
