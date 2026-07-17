//! The stats window: one OS window opened from the menubar beside
//! Settings, the listening record rolled up per ADR 11 - play counts per
//! track, artist, album, and genre, plus how many listens landed this
//! week, month, and year. Everything derives from the events table by
//! SQL on the shared catalog's connection; nothing counts along the way.
//! Rollups read entering the window and when a listen lands or the
//! catalog changes, never per frame.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Global, ScrollHandle,
    SharedString, Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::Root;

use rox_library::listens::{NamePlays, Rollup, TrackPlays};

use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::history::HistoryEvent;
use crate::panel::AppState;
use crate::panels::library::{Library, LibraryEvent};
use crate::settings_ui::{self, section, SECTION_GAP};

/// How many rows the track and name rollups show.
const TOP_TRACKS: usize = 25;
const TOP_NAMES: usize = 15;

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
    let bounds = Bounds::centered(None, size(px(640.), px(560.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - stats".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - stats");
            let view = cx.new(|cx| StatsWindow::new(state, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the stats window");
    cx.set_global(OpenStats(handle));
}

/// Everything the window shows, measured whole on each refresh.
#[derive(Default)]
struct StatsData {
    /// Listens landed inside each trailing window: week, month, year,
    /// and all time.
    week: u64,
    month: u64,
    year: u64,
    total: u64,
    tracks: Vec<TrackPlays>,
    artists: Vec<NamePlays>,
    albums: Vec<NamePlays>,
    genres: Vec<NamePlays>,
}

struct StatsWindow {
    library: Entity<Library>,
    data: StatsData,
    /// The page's scroll position, shared with the scrollbar.
    scroll: ScrollHandle,
    /// The shared art bake and this window's slice of the backdrop, so
    /// the window backs with the playing track's art like every other.
    now_art: Entity<NowPlayingArt>,
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
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
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
        let mut this = StatsWindow {
            library: state.library,
            data: StatsData::default(),
            scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _history_changed,
            _library_changed,
            _backdrop_changed,
        };
        this.refresh(cx);
        this
    }

    /// Roll the events up whole: the recency counts over trailing
    /// windows, then the four groupings.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let library = self.library.read(cx);
        const DAY: i64 = 86400;
        self.data = StatsData {
            week: library.listens_since(now - 7 * DAY),
            month: library.listens_since(now - 30 * DAY),
            year: library.listens_since(now - 365 * DAY),
            total: library.listens_since(0),
            tracks: library.most_played(TOP_TRACKS),
            artists: library.listen_rollup(Rollup::Artist, TOP_NAMES),
            albums: library.listen_rollup(Rollup::Album, TOP_NAMES),
            genres: library.listen_rollup(Rollup::Genre, TOP_NAMES),
        };
        cx.notify();
    }

    /// The recency section: one row per trailing window.
    fn recency(&self) -> Div {
        let rows = [
            ("this week", self.data.week),
            ("this month", self.data.month),
            ("this year", self.data.year),
            ("all time", self.data.total),
        ];
        section(
            "listens",
            None,
            div()
                .flex()
                .flex_col()
                .children(rows.into_iter().map(|(label, count)| {
                    stat_row(
                        div().child(label).into_any_element(),
                        count.to_string(),
                    )
                })),
        )
    }

    /// The track rollup: title and artist - album per row, most played
    /// first.
    fn track_section(&self) -> Div {
        let mut body = div().flex().flex_col();
        if self.data.tracks.is_empty() {
            body = body.child(empty_note());
        }
        for row in &self.data.tracks {
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
            body = body.child(stat_row(label.into_any_element(), row.plays.to_string()));
        }
        section("tracks", None, body)
    }

    /// One name rollup: artist, album, or genre rows with their counts.
    fn name_section(&self, label: &'static str, rows: &[NamePlays]) -> Div {
        let mut body = div().flex().flex_col();
        if rows.is_empty() {
            body = body.child(empty_note());
        }
        for row in rows {
            body = body.child(stat_row(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(row.name.clone()))
                    .into_any_element(),
                row.plays.to_string(),
            ));
        }
        section(label, None, body)
    }
}

/// One row of a rollup: its label filling left, the count right.
fn stat_row(label: gpui::AnyElement, count: String) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .border_b_1()
        .border_color(palette::border())
        .child(div().flex_1().min_w_0().child(label))
        .child(
            div()
                .flex_none()
                .text_right()
                .text_color(palette::text_muted())
                .child(count),
        )
}

/// What a rollup shows before any listen has landed.
fn empty_note() -> Div {
    div()
        .py(tokens::SPACE_XS)
        .text_color(palette::text_muted())
        .child("no listens yet")
}

impl Render for StatsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(self.recency())
            .child(self.track_section())
            .child(self.name_section("artists", &self.data.artists))
            .child(self.name_section("albums", &self.data.albums))
            .child(self.name_section("genres", &self.data.genres));

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
            .children(self.backdrop.layer(&self.now_art, window, cx))
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
