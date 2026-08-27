//! The history panel: the listen record as a track list, per ADR 11 and
//! the scope's history surface. Three views over the same events (the
//! newest listens first, tracks by play count, and the library tracks no
//! event has ever named), picked per panel, so a duplicate can watch
//! each. Rows read at panel-open and listen-append cadence off the
//! library's events table, never per frame; clicks select and double
//! clicks queue from the row, the library panel's moves. Its own panel,
//! never a mode of the library.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, uniform_list, App, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, ScrollStrategy, SharedString,
    Stateful, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_core::fmt::fmt_ago;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::listens::{NeverOrder, TrackPlays};
use rox_library::projection::{parse_query, track_matches, FilterSet, TrackFields};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::group_head::Headers;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};
use rox_services::history::HistoryEvent;

/// One row's height; the list is a uniform_list, so every row is the same.
const ROW_H: f32 = 30.;

/// How many rows a view reads. The panel is a window into the record,
/// not an export; the events themselves are unbounded.
const ROWS_CAP: usize = 500;

/// Which cut of the events the panel shows.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryView {
    #[default]
    Recent,
    Most,
    Never,
}

impl HistoryView {
    fn label(self) -> SharedString {
        match self {
            HistoryView::Recent => rox_i18n::t!("history-view-recent"),
            HistoryView::Most => rox_i18n::t!("history-view-most"),
            HistoryView::Never => rox_i18n::t!("history-view-never"),
        }
    }
}

/// How the Never Played view orders its tracks. Recent and Most get their
/// own order out of the events table (newest first and by count), so this
/// is the one view with nothing to sort it but the tags.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NeverSort {
    /// The canonical album artist, album, disc, track order.
    #[default]
    Browse,
    Title,
    Artist,
    Album,
    Year,
    Duration,
    Rating,
    Added,
}

impl NeverSort {
    /// The store's matching order key. The sort runs in SQL rather than
    /// over the rows already read, so it picks the top of the library
    /// instead of re-arranging the first [`ROWS_CAP`] of the browse order.
    fn order(self) -> NeverOrder {
        match self {
            NeverSort::Browse => NeverOrder::Browse,
            NeverSort::Title => NeverOrder::Title,
            NeverSort::Artist => NeverOrder::Artist,
            NeverSort::Album => NeverOrder::Album,
            NeverSort::Year => NeverOrder::Year,
            NeverSort::Duration => NeverOrder::Duration,
            NeverSort::Rating => NeverOrder::Rating,
            NeverSort::Added => NeverOrder::Added,
        }
    }
}

/// The Never Played sorts, in menu and settings order.
fn never_sorts() -> Vec<(SharedString, NeverSort)> {
    vec![
        (rox_i18n::t!("history-sort-browse"), NeverSort::Browse),
        (rox_i18n::t!("info-item-title"), NeverSort::Title),
        (rox_i18n::t!("head-piece-artist"), NeverSort::Artist),
        (rox_i18n::t!("head-piece-album"), NeverSort::Album),
        (rox_i18n::t!("head-piece-year"), NeverSort::Year),
        (rox_i18n::t!("info-item-duration"), NeverSort::Duration),
        (rox_i18n::t!("info-item-rating"), NeverSort::Rating),
        (rox_i18n::t!("history-sort-date-added"), NeverSort::Added),
    ]
}

/// The track columns, in render order. Plays and Last Played are the record's
/// own, drawn here; the rest are the shared columns [`track_columns::cell`]
/// draws. The view sets the query order, not which columns show.
///
/// `track_columns::checklist`/`columns_submenu` want a `'static` slice, so
/// this rebuilds and leaks once per active locale rather than on every call,
/// mirroring `rox_i18n::t_static`'s own per-locale cache.
fn columns() -> Vec<Column> {
    vec![
        Column {
            key: "cover",
            label: rox_i18n::t!("columns-cover"),
            default_on: false,
        },
        Column {
            key: "number",
            label: rox_i18n::t!("columns-number"),
            default_on: false,
        },
        Column {
            key: "name",
            label: rox_i18n::t!("columns-name"),
            default_on: true,
        },
        Column {
            key: "artist",
            label: rox_i18n::t!("head-piece-artist"),
            default_on: true,
        },
        Column {
            key: "album",
            label: rox_i18n::t!("head-piece-album"),
            default_on: false,
        },
        Column {
            key: "year",
            label: rox_i18n::t!("head-piece-year"),
            default_on: false,
        },
        Column {
            key: "genre",
            label: rox_i18n::t!("head-piece-genre"),
            default_on: false,
        },
        Column {
            key: "duration",
            label: rox_i18n::t!("info-item-duration"),
            default_on: false,
        },
        Column {
            key: "plays",
            label: rox_i18n::t!("status-item-plays"),
            default_on: true,
        },
        Column {
            key: "lastplayed",
            label: rox_i18n::t!("history-column-last-played"),
            default_on: true,
        },
        Column {
            key: "rating",
            label: rox_i18n::t!("info-item-rating"),
            default_on: true,
        },
        Column {
            key: "favourite",
            label: rox_i18n::t!("info-item-favourite"),
            default_on: true,
        },
    ]
}

/// A flattened display row: an album heading (Recent view only), or a track
/// at its index into `tracks`.
enum Row {
    Album(u32),
    AlbumMeta(u32),
    Track(u32),
}

/// A history track's grouping inputs, borrowed for the album run aggregate.
fn group_track(t: &TrackPlays) -> GroupTrack<'_> {
    GroupTrack {
        album: &t.album,
        album_artist: &t.album_artist,
        artist: &t.artist,
        year: t.year,
        genre: &t.genre,
        codec: &t.codec,
        bitrate_kbps: t.bitrate_kbps,
        sample_rate_hz: t.sample_rate_hz,
        bit_depth: t.bit_depth,
        duration_ms: t.duration_ms,
        track_id: t.track_id,
    }
}

/// The history panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Missing fields take the defaults,
/// so a layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub view: HistoryView,
    /// The album heading mode, honoured on the Recent view only, since the
    /// Most and Never orders never keep an album's tracks together.
    pub headers: Headers,
    /// The Never view's order, and whether it runs backwards. Recent and
    /// Most come out of the events table already ordered, so neither reads
    /// these.
    #[serde(default)]
    pub never_sort: NeverSort,
    #[serde(default)]
    pub never_desc: bool,
    /// The shown column keys; defaults to the registry's default-on set.
    pub columns: Vec<String>,
    /// Whether the search box shows; the query only filters while it does.
    #[serde(default)]
    pub search: bool,
    /// Follow the shared query, or filter by this panel's own box.
    #[serde(default)]
    pub query_source: QuerySource,
    /// The panel's own query, kept while following the shared one.
    #[serde(default)]
    pub query: String,
}

// Hand-written so the columns default to the registry set, both for a new
// panel and a saved layout from before columns existed.
impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            chrome: PanelChrome::default(),
            view: HistoryView::default(),
            headers: Headers::Off,
            never_sort: NeverSort::default(),
            never_desc: false,
            columns: track_columns::default_columns(&columns()),
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
        }
    }
}

pub struct HistoryPanel {
    state: AppState,
    config: HistoryConfig,
    /// The current view's tracks in query order, re-read when a listen is
    /// recorded or the catalog changes, cached between.
    tracks: Vec<TrackPlays>,
    /// The search box, shared by every searching view; shown per config.
    search: Entity<SearchBox>,
    /// A pending box reset from a source toggle or a shared-query change,
    /// applied on the next render where a window exists to set the input.
    resync_box: bool,
    /// The tracks this panel is pinned to while following the selection.
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// The query and filter the rows are built for, snapshotted whenever the
    /// query changes so `rebuild_rows` filters without a `cx`.
    applied_query: String,
    applied_filter: FilterSet,
    /// The display rows over `tracks`: the matching tracks flat, or broken by
    /// album headings on the Recent view.
    rows: Vec<Row>,
    /// The album runs the heading rows index, rebuilt with `rows`.
    albums: Vec<track_columns::AlbumGroup>,
    /// The favourited track ids, what each row's heart checks against.
    favourites: HashSet<i64>,
    /// The selected tracks, as indices into `tracks`. Shift extends, cmd
    /// (ctrl elsewhere) toggles, Ctrl+A takes the lot, the library's rules.
    /// A refresh re-reads the tracks, so it clears with them.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or toggle
    /// pick, as its index into `tracks`.
    anchor: Option<usize>,
    /// The playing track's path, the change detector for the highlight;
    /// the player notifies every pump, so the compare keeps sync cheap.
    playing_key: Option<TrackKey>,
    /// The playing track as its library id, the rows' key.
    playing: Option<i64>,
    /// The track under the last right press, for the context menu: the
    /// builder gets no position, so the press records it (the grid keys
    /// off hover for the same reason).
    menu_row: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _history_changed: Subscription,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
}

impl HistoryPanel {
    pub fn new(
        state: AppState,
        config: HistoryConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        // A landing cover repaints the heading tiles and the cover column.
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        // A rescan retags tracks and grows the never-played set; a favourite
        // change moves that column. A rating click only moved one cell through
        // the shared projection, and the play-keyed views never reorder on it,
        // so patch it in place instead of re-running the listens query.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Rated) {
                    this.patch_ratings(cx);
                    return;
                }
                if matches!(
                    event,
                    LibraryEvent::Updated | LibraryEvent::PlaylistsChanged
                ) {
                    this.refresh(cx);
                }
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: re-filter and reset the box
        // to it on the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        // Restored as selection-following, it opens on whatever is picked
        // now, rather than blank until the next pick.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        // Follow the app-wide selection while pinned to it.
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        let mut this = HistoryPanel {
            state,
            config,
            tracks: Vec::new(),
            search,
            resync_box: false,
            selection_ids,
            applied_query: String::new(),
            applied_filter: FilterSet::default(),
            rows: Vec::new(),
            albums: Vec::new(),
            favourites: HashSet::new(),
            selected: HashSet::new(),
            anchor: None,
            playing_key: None,
            playing: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _history_changed,
            _library_changed,
            _player_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
            _selection_changed,
        };
        this.refresh(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, resolve the playing path to
    /// its id (one store lookup), the library panel's move. The highlight
    /// matches rows by that id, so in the recent view every listen of the
    /// playing track gets it.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.key);
        if path == self.playing_key {
            return;
        }
        self.playing_key = path;
        self.playing = self
            .playing_key
            .as_ref()
            .and_then(|key| self.state.library.read(cx).id_for_key(key));
        cx.notify();
    }

    /// Re-read the current view's tracks off the events table, then lay out
    /// the display rows.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.read(cx);
        self.tracks = match self.config.view {
            HistoryView::Recent => library.recent_listens(0, ROWS_CAP),
            HistoryView::Most => library.most_played(ROWS_CAP),
            HistoryView::Never => library.never_played(
                self.config.never_sort.order(),
                self.config.never_desc,
                ROWS_CAP,
            ),
        };
        self.favourites = library.favourite_ids();
        self.selected.clear();
        self.anchor = None;
        self.menu_row = None;
        self.refresh_query(cx);
        self.rebuild_rows();
        cx.notify();
    }

    /// Re-read ratings for the current tracks in place after a star click,
    /// instead of re-running the listens query. The rating moved through the
    /// shared projection already, the view (recent, most, never) is keyed on
    /// play counts a rating never touches, and the display rows index into
    /// `tracks` by position, so the changed cell just repaints.
    fn patch_ratings(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.tracks.iter().map(|t| t.track_id).collect();
        let ratings = self.state.library.read(cx).ratings_for(&ids);
        for t in &mut self.tracks {
            if let Some(&r) = ratings.get(&t.track_id) {
                t.rating = r;
            }
        }
        cx.notify();
    }

    /// Snapshot the active query and filter, so `rebuild_rows` filters the
    /// tracks without a `cx`. The shared query while following it, the box's
    /// own text otherwise.
    fn refresh_query(&mut self, cx: &Context<Self>) {
        self.applied_query = self.effective_query(cx);
        self.applied_filter = self.effective_filter(cx);
    }

    /// Whether a history track passes the active query and filter.
    fn matches(&self, terms: &[rox_library::projection::Term], t: &TrackPlays) -> bool {
        let fields = TrackFields {
            db_id: Some(t.track_id),
            title: &t.title,
            artist: &t.artist,
            album_artist: &t.album_artist,
            album: &t.album,
            genre: &t.genre,
            year: t.year,
            codec: &t.codec,
            path: &t.path,
        };
        track_matches(terms, &fields)
            && self
                .applied_filter
                .matches(&fields, crate::settings::fold_case())
    }

    /// Whether the album headings apply: on, and only in the Recent view,
    /// where the order is at least chronological. Most and Never never keep
    /// an album's tracks consecutive, so the headings would be noise.
    fn grouping(&self) -> bool {
        self.config.view == HistoryView::Recent && self.config.headers != Headers::Off
    }

    /// Lay the display rows over `tracks`: flat, or broken into album runs
    /// with a heading over each. Only the display shape changes, so a headings
    /// or column flip that leaves the tracks alone calls this, not `refresh`.
    fn rebuild_rows(&mut self) {
        let terms = parse_query(&self.applied_query);
        let visible: Vec<u32> = (0..self.tracks.len() as u32)
            .filter(|&i| self.matches(&terms, &self.tracks[i as usize]))
            .collect();
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        if !self.grouping() {
            rows.extend(visible.into_iter().map(Row::Track));
            self.rows = rows;
            self.albums = albums;
            return;
        }
        let mut i = 0;
        while i < visible.len() {
            let mut j = i + 1;
            let head = &self.tracks[visible[i] as usize];
            while j < visible.len()
                && self.tracks[visible[j] as usize].album == head.album
                && self.tracks[visible[j] as usize].album_artist == head.album_artist
            {
                j += 1;
            }
            let group: Vec<GroupTrack> = visible[i..j]
                .iter()
                .map(|&ti| group_track(&self.tracks[ti as usize]))
                .collect();
            albums.push(track_columns::album_group(&group));
            let g = (albums.len() - 1) as u32;
            rows.push(Row::Album(g));
            if self.config.headers == Headers::Expanded {
                rows.push(Row::AlbumMeta(g));
            }
            rows.extend(visible[i..j].iter().copied().map(Row::Track));
            i = j;
        }
        self.rows = rows;
        self.albums = albums;
    }

    fn set_view(&mut self, view: HistoryView, cx: &mut Context<Self>) {
        if self.config.view == view {
            return;
        }
        self.config.view = view;
        self.refresh(cx);
    }

    /// The Never view's order. The sort is part of the query, so a change
    /// re-reads rather than shuffling the rows in hand.
    fn set_never_sort(&mut self, sort: NeverSort, cx: &mut Context<Self>) {
        if self.config.never_sort == sort {
            return;
        }
        self.config.never_sort = sort;
        self.refresh(cx);
    }

    fn set_never_desc(&mut self, desc: bool, cx: &mut Context<Self>) {
        if self.config.never_desc == desc {
            return;
        }
        self.config.never_desc = desc;
        self.refresh(cx);
    }

    /// Where the playing track is, as a display row and its index into
    /// `tracks`. Often nowhere: a listen is only recorded once the play passes
    /// the scrobble threshold, so a track partway through its first play
    /// is in the Never list until it has ever scrobbled and on the Recent
    /// page only if an older play of it is still inside [`ROWS_CAP`]. A
    /// file outside the library has no id to match at all. The menu reads
    /// this to decide whether the jump is worth offering.
    fn playing_row(&self) -> Option<(usize, usize)> {
        let playing = self.playing?;
        self.rows
            .iter()
            .enumerate()
            .find_map(|(ix, row)| match row {
                Row::Track(ti) if self.tracks[*ti as usize].track_id == playing => {
                    Some((ix, *ti as usize))
                }
                _ => None,
            })
    }

    /// Scroll the playing track into view and select it, the move every
    /// other track surface's menu has.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some((ix, ti)) = self.playing_row() else {
            return;
        };
        self.select(ti, Modifiers::default(), cx);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    /// Map the shared box's events onto the panel: a changed query re-filters,
    /// and a focus or dismiss repaints the tab title row that holds the box.
    fn on_search_event(
        &mut self,
        _search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => self.on_query_box_changed(cx),
            SearchEvent::FocusChanged => {
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    /// Show or hide the panel's own search box, re-filtering. The config is
    /// part of the layout dump, so the tab-panel repaint writes it to disk.
    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// Put a click on a track row: plain selects just it, shift extends from
    /// the anchor over the visible tracks between, cmd (ctrl elsewhere)
    /// toggles, the library's rules. `ti` indexes `tracks`, not the display
    /// rows; the shift range runs over the display order, so it crosses
    /// album headings the way the eye does.
    fn select(&mut self, ti: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if ti >= self.tracks.len() {
            return;
        }
        if modifiers.shift {
            let pos_of = |t: usize| {
                self.rows
                    .iter()
                    .position(|row| matches!(row, Row::Track(i) if *i as usize == t))
            };
            let Some(ix) = pos_of(ti) else {
                return;
            };
            let anchor_ix = self.anchor.and_then(pos_of).unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            let range: Vec<usize> = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    Row::Track(i) => Some(*i as usize),
                    _ => None,
                })
                .collect();
            // Ctrl+Shift stacks the range onto the selection so you can
            // skip a run and grab a second block; plain shift replaces.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.into_iter().collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(ti);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(ti) {
                self.selected.remove(&ti);
            }
            self.anchor = Some(ti);
        } else {
            self.selected = HashSet::from([ti]);
            self.anchor = Some(ti);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// Ctrl+A: take every visible track, the filter's rows, so the selection
    /// matches what shows. Anchors at the first so a follow-up shift-click
    /// narrows from the top.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let tracks: Vec<usize> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(ti) => Some(*ti as usize),
                _ => None,
            })
            .collect();
        if tracks.is_empty() {
            return;
        }
        self.anchor = tracks.first().copied();
        self.selected = tracks.into_iter().collect();
        self.publish_selection(cx);
        cx.notify();
    }

    /// The selected track ids in display order, deduplicated: the Recent
    /// view lists a track once per listen, and the shared selection and the
    /// menu actions want each track once.
    fn selected_track_ids(&self) -> Vec<i64> {
        let mut seen = HashSet::new();
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(ti) => {
                    let ti = *ti as usize;
                    self.selected
                        .contains(&ti)
                        .then(|| self.tracks[ti].track_id)
                }
                _ => None,
            })
            .filter(|&id| seen.insert(id))
            .collect()
    }

    /// Publish the selected track ids on the shared selection for the
    /// panels that display it.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    /// Escape drops the selection and the shared scope with it. The local
    /// publish skips empty sets, so the clear goes to the selection
    /// entity directly.
    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(Vec::new(), source, cx));
        cx.notify();
    }

    /// Ctrl+A takes every visible track; Escape drops the selection.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
        } else if key == "escape" {
            self.deselect(cx);
        }
    }

    /// A double click queues the track with the surrounding view as its
    /// timeline: earlier tracks seed behind the cursor for Prev, later ones
    /// fill Next, the clicked track plays. Bounded to a window around the
    /// click with a share kept for history. A track deleted since its event
    /// resolves to no path and drops out of the queue quietly.
    fn play_from(&mut self, ti: usize, cx: &mut Context<Self>) {
        // Window over the visible tracks in query order, the rows on screen, not
        // the raw list. Windowing over `self.tracks` would pull query-hidden
        // tracks into the queue. `ti` indexes `self.tracks`; find where it is
        // among the visible rows first.
        let visible: Vec<usize> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(i) => Some(*i as usize),
                _ => None,
            })
            .collect();
        let Some(pos) = visible.iter().position(|&i| i == ti) else {
            return;
        };
        let lo = pos
            .saturating_sub(QUEUE_CAP / 2)
            .min(visible.len().saturating_sub(QUEUE_CAP));
        let hi = (lo + QUEUE_CAP).min(visible.len());
        let ids: Vec<i64> = visible[lo..hi]
            .iter()
            .map(|&i| self.tracks[i].track_id)
            .collect();
        let click = pos - lo;
        // keys_for drops deleted ids, so the compacted queue is shorter than
        // the window and the raw click offset no longer lines up. The start is
        // how many ids ahead of the click actually resolved. If the clicked
        // track is itself one of the deleted ones, bail rather than play its
        // neighbour, which is what would end up at that index.
        let resolved = {
            let library = self.state.library.read(cx);
            let (Ok(keys), Ok(before), Ok(clicked)) = (
                library.keys_for(&ids),
                library.keys_for(&ids[..click]),
                library.keys_for(&ids[click..=click]),
            ) else {
                return;
            };
            if keys.is_empty() || clicked.is_empty() {
                return;
            }
            (keys, before.len())
        };
        let (keys, start) = resolved;
        self.state
            .player
            .update(cx, |player, cx| player.play_at(keys, start, cx));
    }

    /// The visible slice of the list: album headings (Recent view) and track
    /// rows, drawn through the shared column surface.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    Row::Album(g) => {
                        let g = *g;
                        let headers = self.config.headers;
                        track_columns::album_name_row(
                            ix,
                            &mut self.albums[g as usize],
                            headers,
                            &self.state,
                            cx,
                        )
                    }
                    Row::AlbumMeta(g) => {
                        let g = *g;
                        track_columns::album_meta_row(
                            ix,
                            &mut self.albums[g as usize],
                            &self.state,
                            cx,
                        )
                    }
                    Row::Track(ti) => {
                        let ti = *ti as usize;
                        self.track_row(ix, ti, now, cx)
                    }
                })
            })
            .collect()
    }

    /// One track row: its interactions keyed on the track index, its cells
    /// the shown columns: the shared ones plus the record's own Plays and
    /// Last Played.
    fn track_row(&self, ix: usize, ti: usize, now: i64, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.tracks[ti];
        let playing = self.playing == Some(t.track_id);
        let selected = self.selected.contains(&ti);
        let favourite = self.favourites.contains(&t.track_id);
        let mut row = div()
            .id(("history-row", ix))
            // The hover group the rating and favourite cells reveal on.
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(palette::scaled_px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            // The playing track uses the highlight role, a faint cut apart
            // from the accent-washed selection, the library's look.
            .when(playing && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // Take focus so Ctrl+A and Escape reach the panel's key
                    // handler.
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play_from(ti, cx);
                    } else {
                        this.select(ti, event.modifiers, cx);
                    }
                }),
            )
            // The right press records the track and, outside the selection,
            // reselects it, so the menu acts on what is highlighted.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ti);
                    if !this.selected.contains(&ti) {
                        this.select(ti, Modifiers::default(), cx);
                    }
                }),
            );
        let cover = track_columns::cover_thumb(
            &self.state,
            (!t.path.is_empty()).then(|| std::path::Path::new(&t.path)),
            self.column_shown("cover"),
            cx,
        );
        let cell = track_columns::Cell {
            pos: (ti + 1) as u32,
            title: &t.title,
            artist: &t.artist,
            album: &t.album,
            year: t.year,
            genre: &t.genre,
            duration_ms: t.duration_ms,
            rating: t.rating,
            track_id: t.track_id,
            favourite,
            playing,
            plays: t.plays as u32,
            cover,
        };
        for col in columns() {
            if !self.column_shown(col.key) {
                continue;
            }
            let c = match track_columns::cell(col.key, &cell, &self.state) {
                Some(c) => c,
                // Last Played is the record's own column; the rest, plays
                // included, are shared. Blank when there is nothing to say.
                None => match col.key {
                    "lastplayed" => muted_cell(if t.last_played == 0 {
                        String::new()
                    } else {
                        fmt_ago(now - t.last_played)
                    }),
                    _ => continue,
                },
            };
            row = row.child(c);
        }
        row
    }

    /// The Display section: the view pick, the columns, and the headings on
    /// the Recent view only, where the order keeps albums together. The same
    /// knobs the settings window edits.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let view = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for view in [HistoryView::Recent, HistoryView::Most, HistoryView::Never] {
                submenu = submenu.item(panel::check_row(
                    view.label(),
                    None,
                    move |this: &Self| this.config.view == view,
                    move |this, cx| this.set_view(view, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu
            .label(rox_i18n::t!("panel-menu-display"))
            .item(PopupMenuItem::submenu(
                rox_i18n::t!("panel-page-view"),
                view,
            ))
            .item(PopupMenuItem::submenu(
                rox_i18n::t!("library-columns"),
                track_columns::columns_submenu(columns(), window, cx),
            ));
        match self.config.view {
            HistoryView::Recent => menu.item(PopupMenuItem::submenu(
                rox_i18n::t!("panel-headings"),
                track_columns::headings_submenu(window, cx),
            )),
            // Recent and Most come out of the events table ordered; Never
            // is the view with a sort to pick.
            HistoryView::Never => menu.item(PopupMenuItem::submenu(
                rox_i18n::t!("history-sort-menu"),
                self.sort_submenu(window, cx),
            )),
            HistoryView::Most => menu,
        }
    }

    /// The Never view's sort keys, with the direction as a check under them.
    fn sort_submenu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let panel = cx.entity();
        PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for (label, sort) in never_sorts() {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.never_sort == sort,
                    move |this, cx| this.set_never_sort(sort, cx),
                    &panel,
                ));
            }
            submenu.separator().item(panel::check_row(
                rox_i18n::t!("history-descending"),
                None,
                |this: &Self| this.config.never_desc,
                |this, cx| {
                    let desc = this.config.never_desc;
                    this.set_never_desc(!desc, cx);
                },
                &panel,
            ))
        })
    }
}

impl ColumnHost for HistoryPanel {
    fn column_shown(&self, key: &str) -> bool {
        self.config.columns.iter().any(|k| k == key)
    }

    fn set_column(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        let has = self.column_shown(key);
        if on && !has {
            self.config.columns.push(key.to_string());
        } else if !on {
            self.config.columns.retain(|k| k != key);
        }
        cx.notify();
    }
}

impl HeadingHost for HistoryPanel {
    fn headers(&self) -> Headers {
        self.config.headers
    }

    /// Set the heading mode and relay out the rows; the tracks are unchanged,
    /// so no re-query, just a fresh row plan.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.config.headers == headers {
            return;
        }
        self.config.headers = headers;
        self.rebuild_rows();
        cx.notify();
    }
}

impl QueryFilter for HistoryPanel {
    fn shared_query(&self) -> &Entity<crate::query::shared_query::SharedQuery> {
        &self.state.query
    }
    fn query_box(&self) -> &Entity<SearchBox> {
        &self.search
    }
    fn query_source(&self) -> QuerySource {
        self.config.query_source
    }
    fn set_query_source_value(&mut self, source: QuerySource) {
        self.config.query_source = source;
    }
    fn local_query(&self) -> String {
        self.config.query.clone()
    }
    fn set_local_query(&mut self, query: String) {
        self.config.query = query;
    }
    fn query_box_shown(&self) -> bool {
        self.config.search
    }
    fn set_query_box_shown(&mut self, shown: bool) {
        self.config.search = shown;
    }
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>) {
        self.refresh_query(cx);
        self.rebuild_rows();
        cx.notify();
    }
    fn set_query_resync(&mut self, pending: bool) {
        self.resync_box = pending;
    }
    fn selection(&self) -> &Entity<crate::selection::Selection> {
        &self.state.selection
    }
    fn selection_ids(&self) -> &[i64] {
        &self.selection_ids
    }
    fn set_selection_ids(&mut self, ids: Vec<i64>) {
        self.selection_ids = ids;
    }
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }
}

impl PanelSettings for HistoryPanel {
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
        &[("Content", icons::CLOCK)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let recent = self.config.view == HistoryView::Recent;
        let never = self.config.view == HistoryView::Never;
        let desc = self.config.never_desc;
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("history-view-row"),
                Some(rox_i18n::t!("history-view-row.description")),
                panel::choices_shared(
                    &[
                        (
                            rox_i18n::t!("history-view-recent-short"),
                            HistoryView::Recent,
                        ),
                        (rox_i18n::t!("history-view-most"), HistoryView::Most),
                        (rox_i18n::t!("history-view-never"), HistoryView::Never),
                    ],
                    self.config.view,
                    |this: &mut Self, view, cx| this.set_view(view, cx),
                    cx,
                ),
            ))
            // Only the Never view has a sort to pick: the other two come
            // out of the events table in their own order.
            .when(never, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("history-sort-menu"),
                    Some(rox_i18n::t!("history-sort-menu.description")),
                    panel::choices_shared(
                        &never_sorts(),
                        self.config.never_sort,
                        |this: &mut Self, sort, cx| this.set_never_sort(sort, cx),
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("history-descending"),
                    Some(rox_i18n::t!("history-descending.description")),
                    panel::toggle(
                        desc,
                        |this: &mut Self, on, cx| this.set_never_desc(on, cx),
                        cx,
                    ),
                ))
            })
            .child(panel::setting_block(
                rox_i18n::t!("library-columns"),
                Some(rox_i18n::t!("panel-columns-description")),
                None,
                track_columns::checklist(&columns(), self, cx),
            ))
            // The album orders only stay together in the Recent view; the
            // headings are off the table on Most and Never.
            .when(recent, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("panel-headings"),
                    Some(rox_i18n::t!("history-headings")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("headers-off"), Headers::Off),
                            (rox_i18n::t!("headers-compact"), Headers::Compact),
                            (rox_i18n::t!("headers-expanded"), Headers::Expanded),
                        ],
                        self.config.headers,
                        |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }

    /// The Behavior page's search section: show the box, and follow the
    /// shared query or filter by the panel's own.
    fn behavior(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        Some(crate::query::shared_query::search_section(
            self.config.search,
            |this: &mut Self, on, cx| this.set_search(on, cx),
            self.config.query_source,
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            cx,
        ))
    }
}

impl EventEmitter<PanelEvent> for HistoryPanel {}

impl Focusable for HistoryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for HistoryPanel {
    fn panel_name(&self) -> &'static str {
        "history"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("history-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The search box shares the title bar row while the panel is in a
    /// group; solo or popped out the body hosts it instead.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.config.search {
            return None;
        }
        Some(
            self.search
                .update(cx, |search, cx| search.element(cx))
                .w(px(180.)),
        )
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The body serves its own row context menus, so the tab panel's body
    /// right-click stays out; the panel dropdown is on the tab and comes
    /// after the track actions.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Jump goes at the top, the move every other track surface's menu
        // opens with, but only while the playing track is on the list.
        // The views here are cuts of the record rather than the library,
        // so most of the time it isn't, and an entry that jumps nowhere
        // is worse than no entry.
        let weak = cx.entity().downgrade();
        let menu = match self.playing_row() {
            Some(_) => menu.item(
                PopupMenuItem::new(rox_i18n::t!("panel-jump-to-playing"))
                    .icon(Icon::default().path(icons::DISC))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.jump_to_playing(cx));
                        }
                    }),
            ),
            None => menu,
        };
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        // Follow the shared search query, or filter by this panel's own box.
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            |this: &mut Self, on, cx| this.set_search(on, cx),
            window,
            cx,
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                HistoryPanel::new(state, config, window, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl HistoryPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // A pending box reset (a source toggle or a shared-query change)
        // is applied here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)));
        let content = if self.rows.is_empty() {
            // Tracks hidden by the query read differently from an empty record.
            let message = if !self.tracks.is_empty() {
                rox_i18n::t!("picker-no-matches")
            } else {
                match self.config.view {
                    HistoryView::Never => rox_i18n::t!("history-empty-never"),
                    _ => rox_i18n::t!("history-empty-recent"),
                }
            };
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child(message),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .border_b_1()
                        .border_color(palette::border())
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(self.config.view.label()),
                )
                .child(
                    uniform_list("history-rows", self.rows.len(), move |range, _, cx| {
                        this.upgrade()
                            .map(|this| this.update(cx, |this, cx| this.list_rows(range, cx)))
                            .unwrap_or_default()
                    })
                    .track_scroll(self.scroll.clone())
                    .flex_1()
                    .w_full(),
                )
        };
        // A right press arrives here in the capture phase, before any row's
        // bubble handler records itself, so a press off the rows leaves
        // no target and the menu below falls back to the panel's own.
        let content =
            content.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        // The row context menu: the track actions every song surface
        // shares, then the panel menu after them, so a click over the
        // list never dead-ends at Play.
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            // The clicked track plus the selection it acts on. The right
            // press already pulled the track into the set, so this is the
            // lit set.
            let target = {
                let panel = this.read(cx);
                panel
                    .menu_row
                    .filter(|ti| *ti < panel.tracks.len())
                    .map(|ti| (ti, panel.selected_track_ids()))
            };
            let Some((ti, ids)) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let state = this.read(cx).state.clone();
            let panel = weak.clone();
            // Play queues the clicked track and what follows in the view's
            // order, the double click's move; the rest of the actions take
            // the whole selection.
            let menu = panel::track_actions(
                menu,
                state,
                ids,
                rox_i18n::t!("library-play"),
                window,
                cx,
                move |_, cx| {
                    if let Some(this) = panel.upgrade() {
                        this.update(cx, |this, cx| this.play_from(ti, cx));
                    }
                },
            );
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
}

/// A trailing muted column: the record's own Plays and Last Played, right
/// of the flexible text columns.
fn muted_cell(text: String) -> Div {
    div()
        .flex_none()
        .text_color(palette::text_muted())
        .child(SharedString::from(text))
}
