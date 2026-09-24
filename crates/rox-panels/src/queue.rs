//! The play queue panel (ADR 16): the explicit up-next queue, what Play
//! Next and Add to Queue put ahead of the playing track. The context you
//! started from plays on and is not listed, so the queue stays what you
//! hand-picked. A now-playing strip heads the rows. Its own panel, never a
//! mode of the library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, Div, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, KeyDownEvent,
    Modifiers, MouseButton, MouseDownEvent, SharedString, Stateful, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, svg, uniform_list,
};
use gpui_component::Icon;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::cue::{Origin, SourceId, TrackKey};
use rox_library::projection::{FilterSet, Filterable, TrackFields, parse_query};
use rox_library::store::TrackMeta;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::group_head::Headers;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::Settings;
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};
use crate::track_ui::track_drag::PlayDrag;

const ROW_H: f32 = 30.;

/// Number here is the queue position.
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
            default_on: true,
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
            default_on: false,
        },
        Column {
            key: "rating",
            label: rox_i18n::t!("info-item-rating"),
            default_on: false,
        },
        Column {
            key: "favourite",
            label: rox_i18n::t!("info-item-favourite"),
            default_on: false,
        },
    ]
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub headers: Headers,
    pub columns: Vec<String>,
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub query_source: QuerySource,
    /// Kept while following the shared query, for the switch back.
    #[serde(default)]
    pub query: String,
}

/// A docked panel's view lives in the layout dump. The modal and
/// popped-out queue have no layout, so they keep their view in settings.
#[derive(Clone, Copy, PartialEq)]
enum Persist {
    Layout,
    Settings,
}

// Hand-written so the columns default to the registry set.
impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig {
            chrome: PanelChrome::default(),
            headers: Headers::Off,
            columns: track_columns::default_columns(&columns()),
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
        }
    }
}

enum QRow {
    Album(u32),
    AlbumMeta(u32),
    Track(u32),
}

struct TrackRow {
    entry_id: u64,
    track_id: Option<i64>,
    pos: u32,
    /// Off the key, not the path: a remote track's path is the source's own
    /// id and says nothing about the source.
    origin: Origin,
    title: String,
    artist: String,
    album: String,
    /// Empty for a queued file the library doesn't know.
    title_reading: String,
    artist_reading: String,
    album_reading: String,
    album_artist: String,
    year: u16,
    genre: String,
    codec: String,
    bitrate_kbps: u16,
    sample_rate_hz: u32,
    bit_depth: u8,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: PathBuf,
    /// The key's own, so a file dropped on the queue reads as local.
    source: SourceId,
}

impl Filterable for TrackRow {
    fn fields(&self) -> TrackFields<'_> {
        TrackFields {
            db_id: self.track_id,
            title: &self.title,
            artist: &self.artist,
            album_artist: &self.album_artist,
            album: &self.album,
            genre: &self.genre,
            year: self.year,
            codec: &self.codec,
            path: self.path.to_str().unwrap_or_default(),
            source: &self.source,
        }
    }
}

fn origin_glyph(origin: Origin) -> Option<&'static str> {
    match origin {
        Origin::Local => None,

        Origin::Radio => Some(icons::RADIO),

        // The closest the icon set has to a server, the settings sidebar's
        // library glyph.
        Origin::Subsonic => Some(icons::DATABASE),
    }
}

fn mark_slot(path: &'static str, color: gpui::Rgba) -> Div {
    div()
        .flex_none()
        .w(px(22.))
        .flex()
        .justify_end()
        .items_center()
        .child(svg().path(path).size(px(12.)).text_color(color))
}

/// Track id 0 stands in for a file the library doesn't know, so it never
/// matches a real album's art.
fn group_track(t: &TrackRow) -> GroupTrack<'_> {
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
        track_id: t.track_id.unwrap_or(0),
    }
}

/// The playing song isn't a queue entry, so it resolves apart from the
/// rows, but through the same columns so the strip lines up.
struct Playing {
    track_id: Option<i64>,
    title: String,
    artist: String,
    album: String,
    title_reading: String,
    artist_reading: String,
    album_reading: String,
    year: u16,
    genre: String,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: PathBuf,
}

/// Inside a multi-selection a drag takes the whole set; outside it, just
/// that row.
#[derive(Clone)]
struct QueueDrag {
    ids: Arc<[u64]>,
    title: SharedString,
}

struct QueueDragPreview {
    title: SharedString,
    extra: usize,
}

impl Render for QueueDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.extra > 0 {
            SharedString::from(format!("{} +{}", self.title, self.extra))
        } else {
            self.title.clone()
        };
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .text_color(palette::text())
            .child(label)
    }
}

pub struct QueuePanel {
    state: AppState,
    config: QueueConfig,
    /// The layout dump for a docked panel, settings for the widget's windowed
    /// queue.
    persist: Persist,
    tracks: Vec<TrackRow>,
    search: Entity<SearchBox>,
    /// Applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// Snapshotted on query change, so `rebuild_rows` filters without a `cx`.
    applied_query: String,
    applied_filter: FilterSet,
    rows: Vec<QRow>,
    albums: Vec<track_columns::AlbumGroup>,
    favourites: HashSet<i64>,
    /// Read with `read_one` at most once per path. None marks a file that
    /// couldn't be stat'd, so it isn't retried.
    loose_tags: HashMap<PathBuf, Option<rox_library::TrackRow>>,
    /// Follows whatever plays, queued or context.
    playing: Option<Playing>,
    /// With the playing key, the change detector: the per-pump observe only
    /// re-reads the queue on an edit or a track advance.
    rev: Option<u64>,
    playing_key: Option<TrackKey>,
    /// A stream turns its song over without the key or the queue moving.
    live_rev: Option<u64>,
    /// By entry id, so a rebuild or a regroup keeps the highlight.
    selected: HashSet<u64>,
    /// Bumped on a selection or row-order change, keying the drag-set cache
    /// so every visible selected row shares one Arc.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[u64]>)>,
    anchor: Option<u64>,
    menu_row: Option<u64>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
}

impl QueuePanel {
    pub fn new(
        state: AppState,
        config: QueueConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| this.sync(cx));
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        // A rating click patches in place; a full sync would rebuild every row
        // per star.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Rated) {
                    this.patch_ratings(cx);
                    return;
                }
                // A play-count import only moves the plays column.
                if matches!(event, LibraryEvent::PlaysReloaded) {
                    this.patch_plays(cx);
                    return;
                }
                if matches!(
                    event,
                    LibraryEvent::Updated | LibraryEvent::PlaylistsChanged
                ) {
                    this.rev = None;
                    this.sync(cx);
                }
            },
        );
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        let mut this = QueuePanel {
            state,
            config,
            persist: Persist::Layout,
            tracks: Vec::new(),
            search,
            resync_box: false,
            selection_ids,
            applied_query: String::new(),
            applied_filter: FilterSet::default(),
            rows: Vec::new(),
            albums: Vec::new(),
            favourites: HashSet::new(),
            loose_tags: HashMap::new(),
            playing: None,
            rev: None,
            playing_key: None,
            live_rev: None,
            selected: HashSet::new(),
            drag_gen: 0,
            drag_set: None,
            anchor: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _library_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
            _selection_changed,
        };
        this.sync(cx);
        this
    }

    /// The widget's modal and popped-out queue: no dock layout, so the view
    /// reads from and writes to settings.
    pub fn windowed(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = Settings::load()
            .windows
            .queue_view
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut this = Self::new(state, config, window, cx);
        this.persist = Persist::Settings;
        this
    }

    /// A docked panel emits LayoutChanged through its host tab panel, since
    /// the panel's own events never reach the dock. Without this an edit only
    /// lands on a clean close.
    fn save_config(&self, cx: &mut Context<Self>) {
        match self.persist {
            Persist::Layout => {
                if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
                    tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                }
            }
            Persist::Settings => {
                let value = serde_json::to_value(self.config.clone()).ok();
                Settings::update(move |s| s.windows.queue_view = value);
            }
        }
    }

    /// Bails on the revision and playing-key compare, so a steady queue costs
    /// two reads a tick.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.player.read(cx).queue_rev();
        let live_rev = self.state.player.read(cx).title_rev();
        let playing_key = self.state.player.read(cx).now_playing().map(|now| now.key);
        if rev == self.rev && live_rev == self.live_rev && playing_key == self.playing_key {
            return;
        }
        self.rev = rev;
        self.live_rev = live_rev;
        self.playing_key = playing_key;
        let queued = self.state.player.read(cx).queued();
        // Through the pool mirror: the engine's entries hold a bare path, so two
        // cue tracks of one image would resolve to the same row.
        let keys: Vec<TrackKey> = {
            let player = self.state.player.read(cx);
            queued.iter().map(|e| player.key_for(e)).collect()
        };
        let library = self.state.library.read(cx);
        // One resolve per key, shared by the passes below.
        let resolved: Vec<Option<(i64, TrackMeta)>> =
            keys.iter().map(|key| library.resolve_key(key)).collect();
        // The station's current song over the strip's tags; it only arrives in
        // band.
        let playing_resolved: Option<(i64, TrackMeta)> = self
            .playing_key
            .as_ref()
            .and_then(|key| library.resolve_key(key))
            .and_then(|(id, meta)| Some((id, self.state.player.read(cx).live_over(Some(meta))?)));
        // One projection pass for the plays column.
        let plays = {
            let mut ids: Vec<i64> = resolved
                .iter()
                .filter_map(|r| r.as_ref().map(|(id, _)| *id))
                .collect();
            if let Some((id, _)) = &playing_resolved {
                ids.push(*id);
            }
            library.plays_for(&ids)
        };
        // `read_one` hits the disk, so it only runs on a rev-change rebuild and is
        // cached per path.
        for (path, meta) in keys.iter().map(|key| &key.path).zip(resolved.iter()).chain(
            self.playing_key
                .as_ref()
                .map(|key| (&key.path, &playing_resolved)),
        ) {
            if meta.is_none() && !self.loose_tags.contains_key(path) {
                self.loose_tags
                    .insert(path.clone(), rox_library::scanner::read_one(path));
            }
        }
        let playing_sort = playing_resolved
            .as_ref()
            .map(|(id, _)| library.sort_names_for_id(*id))
            .unwrap_or_default();
        self.playing = self.playing_key.as_ref().map(|key| {
            let path = &key.path;
            let track_id = playing_resolved.as_ref().map(|(id, _)| *id);
            let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
            match playing_resolved {
                Some((_, m)) => Playing {
                    track_id,
                    title: m.title,
                    artist: m.artist,
                    album: m.album,
                    title_reading: playing_sort.title.clone(),
                    artist_reading: playing_sort.artist.clone(),
                    album_reading: playing_sort.album.clone(),
                    year: m.year,
                    genre: m.genre,
                    duration_ms: m.duration_ms,
                    rating: m.rating,
                    plays: count,
                    path: path.clone(),
                },
                None => match self.loose_tags.get(path).and_then(Option::as_ref) {
                    Some(r) => Playing {
                        track_id,
                        title: r.title.clone(),
                        artist: r.artist.clone(),
                        album: r.album.clone(),
                        title_reading: String::new(),
                        artist_reading: String::new(),
                        album_reading: String::new(),
                        year: r.year,
                        genre: r.genre.clone(),
                        duration_ms: r.duration_ms,
                        rating: 0,
                        plays: count,
                        path: path.clone(),
                    },
                    None => Playing {
                        track_id,
                        title: file_label(path),
                        artist: String::new(),
                        album: String::new(),
                        title_reading: String::new(),
                        artist_reading: String::new(),
                        album_reading: String::new(),
                        year: 0,
                        genre: String::new(),
                        duration_ms: 0,
                        rating: 0,
                        plays: count,
                        path: path.clone(),
                    },
                },
            }
        });
        self.favourites = library.favourite_ids();
        self.tracks = queued
            .iter()
            .zip(&keys)
            .zip(resolved)
            .enumerate()
            .map(|(i, ((entry, key), resolved))| {
                let track_id = resolved.as_ref().map(|(id, _)| *id);
                let origin = key.origin();
                let pos = (i + 1) as u32;
                let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
                let sort = track_id
                    .map(|id| library.sort_names_for_id(id))
                    .unwrap_or_default();
                match resolved {
                    Some((_, m)) => TrackRow {
                        entry_id: entry.id,
                        track_id,
                        pos,
                        origin,
                        title: m.title,
                        artist: m.artist,
                        album: m.album,
                        title_reading: sort.title,
                        artist_reading: sort.artist,
                        album_reading: sort.album,
                        album_artist: m.album_artist,
                        year: m.year,
                        genre: m.genre,
                        codec: m.codec,
                        bitrate_kbps: m.bitrate_kbps,
                        sample_rate_hz: m.sample_rate_hz,
                        bit_depth: m.bit_depth,
                        duration_ms: m.duration_ms,
                        rating: m.rating,
                        plays: count,
                        path: key.path.clone(),
                        source: key.source.clone(),
                    },
                    None => match self.loose_tags.get(&key.path).and_then(Option::as_ref) {
                        Some(r) => TrackRow {
                            entry_id: entry.id,
                            track_id,
                            pos,
                            origin,
                            title: r.title.clone(),
                            artist: r.artist.clone(),
                            album: r.album.clone(),
                            title_reading: String::new(),
                            artist_reading: String::new(),
                            album_reading: String::new(),
                            album_artist: r.album_artist.clone(),
                            year: r.year,
                            genre: r.genre.clone(),
                            codec: r.codec.clone(),
                            bitrate_kbps: r.bitrate_kbps,
                            sample_rate_hz: r.sample_rate_hz,
                            bit_depth: r.bit_depth,
                            duration_ms: r.duration_ms,
                            rating: 0,
                            plays: count,
                            path: key.path.clone(),
                            source: key.source.clone(),
                        },
                        None => TrackRow {
                            entry_id: entry.id,
                            track_id,
                            pos,
                            origin,
                            title: file_label(&key.path),
                            artist: String::new(),
                            album: String::new(),
                            title_reading: String::new(),
                            artist_reading: String::new(),
                            album_reading: String::new(),
                            album_artist: String::new(),
                            year: 0,
                            genre: String::new(),
                            codec: String::new(),
                            bitrate_kbps: 0,
                            sample_rate_hz: 0,
                            bit_depth: 0,
                            duration_ms: 0,
                            rating: 0,
                            plays: count,
                            path: key.path.clone(),
                            source: key.source.clone(),
                        },
                    },
                }
            })
            .collect();
        // Keyed by entry id, so a reordered set stays lit; prune only what left
        // the queue.
        let live: HashSet<u64> = self.tracks.iter().map(|t| t.entry_id).collect();
        self.selected.retain(|id| live.contains(id));
        if self.anchor.is_some_and(|id| !live.contains(&id)) {
            self.anchor = None;
        }
        self.menu_row = None;
        self.refresh_query(cx);
        self.rebuild_rows();
        cx.notify();
    }

    /// In place: the rows index `tracks` by position, so nothing rebuilds. The
    /// playing strip caches its rating too.
    fn patch_ratings(&mut self, cx: &mut Context<Self>) {
        let mut ids: Vec<i64> = self.tracks.iter().filter_map(|t| t.track_id).collect();
        if let Some(id) = self.playing.as_ref().and_then(|p| p.track_id) {
            ids.push(id);
        }
        let ratings = self.state.library.read(cx).ratings_for(&ids);
        for t in &mut self.tracks {
            if let Some(&r) = t.track_id.and_then(|id| ratings.get(&id)) {
                t.rating = r;
            }
        }
        if let Some(playing) = &mut self.playing
            && let Some(&r) = playing.track_id.and_then(|id| ratings.get(&id))
        {
            playing.rating = r;
        }
        cx.notify();
    }

    /// In place: a full sync would rebuild every row and drop the selection.
    fn patch_plays(&mut self, cx: &mut Context<Self>) {
        let mut ids: Vec<i64> = self.tracks.iter().filter_map(|t| t.track_id).collect();
        if let Some(id) = self.playing.as_ref().and_then(|p| p.track_id) {
            ids.push(id);
        }
        let plays = self.state.library.read(cx).plays_for(&ids);
        for t in &mut self.tracks {
            if let Some(&n) = t.track_id.and_then(|id| plays.get(&id)) {
                t.plays = n;
            }
        }
        if let Some(playing) = &mut self.playing
            && let Some(&n) = playing.track_id.and_then(|id| plays.get(&id))
        {
            playing.plays = n;
        }
        cx.notify();
    }

    fn refresh_query(&mut self, cx: &Context<Self>) {
        self.applied_query = self.effective_query(cx);
        self.applied_filter = self.effective_filter(cx);
    }

    fn matches(&self, terms: &[rox_library::projection::Term], t: &TrackRow) -> bool {
        t.passes(terms, &self.applied_filter, crate::settings::fold_case())
    }

    /// A headings, column, or query flip calls this, not `sync`.
    fn rebuild_rows(&mut self) {
        // Row order drives drag order, so a rebuild invalidates the drag set.
        self.drag_gen += 1;
        let terms = parse_query(&self.applied_query);
        let visible: Vec<u32> = (0..self.tracks.len() as u32)
            .filter(|&i| self.matches(&terms, &self.tracks[i as usize]))
            .collect();
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        if self.config.headers == Headers::Off {
            rows.extend(visible.into_iter().map(QRow::Track));
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
            rows.push(QRow::Album(g));
            if self.config.headers == Headers::Expanded {
                rows.push(QRow::AlbumMeta(g));
            }
            rows.extend(visible[i..j].iter().copied().map(QRow::Track));
            i = j;
        }
        self.rows = rows;
        self.albums = albums;
    }

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

    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.save_config(cx);
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    fn entry_at(&self, ix: usize) -> Option<u64> {
        match self.rows.get(ix)? {
            QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
            _ => None,
        }
    }

    fn index_of(&self, entry: u64) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(row, QRow::Track(ti)
                if self.tracks.get(*ti as usize).map(|t| t.entry_id) == Some(entry))
        })
    }

    fn selected_ids(&self) -> Vec<u64> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                QRow::Track(ti) => {
                    let id = self.tracks.get(*ti as usize)?.entry_id;
                    self.selected.contains(&id).then_some(id)
                }
                _ => None,
            })
            .collect()
    }

    fn drag_ids(&mut self) -> Arc<[u64]> {
        if self.drag_set.as_ref().map(|(generation, _)| *generation) != Some(self.drag_gen) {
            let ids: Arc<[u64]> = self.selected_ids().into();
            self.drag_set = Some((self.drag_gen, ids));
        }
        self.drag_set
            .as_ref()
            .map(|(_, ids)| ids.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(entry) = self.entry_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor_ix = self.anchor.and_then(|a| self.index_of(a)).unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            let range: Vec<u64> = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
                    _ => None,
                })
                .collect();
            // Ctrl+Shift stacks the range; plain shift replaces.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.into_iter().collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(entry);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(entry) {
                self.selected.remove(&entry);
            }
            self.anchor = Some(entry);
        } else {
            self.selected = HashSet::from([entry]);
            self.anchor = Some(entry);
        }
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    /// The visible rows only, so the selection matches what Delete removes.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let entries = self
            .rows
            .iter()
            .filter_map(|row| match row {
                QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return;
        }
        self.anchor = entries.first().copied();
        self.selected = entries.into_iter().collect();
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .tracks
            .iter()
            .filter(|t| self.selected.contains(&t.entry_id))
            .filter_map(|t| t.track_id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    /// Through the player's move-then-jump, so the entries above stay queued
    /// instead of becoming history.
    fn jump(&self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.entry_at(ix) else {
            return;
        };
        self.state.player.read(cx).play_queued(id);
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.tracks.iter().map(|t| t.entry_id).collect();
        self.remove_ids(&ids, cx);
    }

    /// Drops from our own rows right away too, since the pump is quiet while
    /// paused and the sync won't run. The lowest removed spot keeps the mark.
    fn remove_ids(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        self.state
            .player
            .read(cx)
            .remove_many_from_queue(ids.to_vec());
        // A set, not a slice: a full clear runs this over every row.
        let drop: HashSet<u64> = ids.iter().copied().collect();
        let landing = self.tracks.iter().position(|t| drop.contains(&t.entry_id));
        self.tracks.retain(|t| !drop.contains(&t.entry_id));
        for (i, t) in self.tracks.iter_mut().enumerate() {
            t.pos = (i + 1) as u32;
        }
        self.selected.clear();
        self.anchor = None;
        if let Some(id) = landing
            .filter(|&ti| ti < self.tracks.len())
            .map(|ti| self.tracks[ti].entry_id)
        {
            self.selected.insert(id);
            self.anchor = Some(id);
        }
        self.rebuild_rows();
        self.publish_selection(cx);
        cx.notify();
    }

    fn remove(&mut self, entry: u64, cx: &mut Context<Self>) {
        let ids = if self.selected.contains(&entry) {
            self.selected_ids()
        } else {
            vec![entry]
        };
        self.remove_ids(&ids, cx);
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "escape" {
            self.deselect(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let ids = self.selected_ids();
            self.remove_ids(&ids, cx);
        }
    }

    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        self.drag_gen += 1;
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(Vec::new(), source, cx));
        cx.notify();
    }

    /// Moves the rows to just after the nearest undragged entry above
    /// `target`, or after the playing track at the top. Each entry chains
    /// after the last, so the group stays one run in queue order.
    fn reorder(&mut self, dragged: &[u64], target: usize, cx: &mut Context<Self>) {
        if dragged.is_empty() {
            return;
        }
        // Heading rows have no entry, so they're skipped.
        let above = self.rows[..target.min(self.rows.len())]
            .iter()
            .rev()
            .filter_map(|row| match row {
                QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
                _ => None,
            })
            .find(|id| !dragged.contains(id));
        let mut after = match above {
            Some(id) => Some(id),
            None => self.state.player.read(cx).playing_entry(),
        };
        let player = self.state.player.read(cx);
        for &id in dragged {
            player.move_in_queue(id, after);
            after = Some(id);
        }
        // Keep the moved group lit across the rebuild.
        self.selected = dragged.iter().copied().collect();
        self.anchor = dragged.first().copied();
        self.drag_gen += 1;
    }

    /// Enqueue, not Play Next, so a drop goes to the back.
    fn enqueue_dropped(&mut self, drag: &PlayDrag, cx: &mut Context<Self>) {
        if drag.is_empty() {
            return;
        }
        let keys = drag.keys.to_vec();
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(keys, cx));
    }

    /// The window body plays drops, so the queue stays the one surface that
    /// adds without interrupting.
    fn enqueue_external(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        // Whole files: nothing off the desktop names a subsong.
        let keys: Vec<TrackKey> =
            rox_library::open_files::resolve_audio_paths(paths.paths().to_vec())
                .into_iter()
                .map(TrackKey::from)
                .collect();
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(keys, cx));
    }

    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        // Resolved once per frame and cached until the selection or rows move.
        let multi_drag = (self.selected.len() > 1).then(|| self.drag_ids());
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    QRow::Album(g) => {
                        let g = *g;
                        let headers = self.config.headers;
                        track_columns::album_name_row(
                            ix,
                            &mut self.albums[g as usize],
                            headers,
                            &track_columns::HeadSlot::stock(
                                &track_columns::stock_name_pieces(headers),
                                &track_columns::stock_head_look(),
                            ),
                            &self.state,
                            cx,
                        )
                    }
                    QRow::AlbumMeta(g) => {
                        let g = *g;
                        track_columns::album_meta_row(
                            ix,
                            &mut self.albums[g as usize],
                            &track_columns::HeadSlot::stock(
                                &crate::group_head::stock_meta_line(),
                                &track_columns::stock_head_look(),
                            ),
                            &self.state,
                            cx,
                        )
                    }
                    QRow::Track(ti) => {
                        let ti = *ti as usize;
                        self.track_row(ix, ti, multi_drag.as_ref(), cx)
                    }
                })
            })
            .collect()
    }

    fn track_row(
        &self,
        ix: usize,
        ti: usize,
        multi_drag: Option<&Arc<[u64]>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.tracks[ti];
        let entry = t.entry_id;
        let has_track = t.track_id.is_some();
        let favourite = t
            .track_id
            .map(|id| self.favourites.contains(&id))
            .unwrap_or(false);
        let selected = self.selected.contains(&entry);
        // The shared Arc from `list_rows` when inside the selection.
        let ids: Arc<[u64]> = match multi_drag {
            Some(set) if selected => set.clone(),
            _ => Arc::from([entry]),
        };
        let drag = QueueDrag {
            ids,
            title: SharedString::from(t.title.clone()),
        };
        let mut row = div()
            .id(("queue-row", ix))
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
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_drag(drag, |drag, _pos, _window, cx| {
                cx.new(|_| QueueDragPreview {
                    title: drag.title.clone(),
                    extra: drag.ids.len().saturating_sub(1),
                })
            })
            .drag_over::<QueueDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &QueueDrag, _, cx| {
                this.reorder(&drag.ids, ix, cx);
            }))
            // gpui dispatches on_drop by payload type, so this sits beside the
            // reorder drop.
            .on_drop(cx.listener(move |this, drag: &PlayDrag, _, cx| {
                this.enqueue_dropped(drag, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.jump(ix, cx);
                    } else if event.modifiers.shift || event.modifiers.secondary() {
                        this.select(ix, event.modifiers, cx);
                    } else if !this.selected.contains(&entry) {
                        // A press on an unselected row picks it now so a drag takes it; on a lit
                        // row it keeps the set for a group drag.
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A click that never became a drag collapses the selection to this row.
                let mods = event.modifiers();
                if event.click_count() == 1
                    && !mods.shift
                    && !mods.secondary()
                    && this.selected.len() > 1
                    && this.selected.contains(&entry)
                {
                    this.select(ix, Modifiers::default(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(entry);
                    // A right click outside the set reselects just that row.
                    if !this.selected.contains(&entry) {
                        this.select(ix, Modifiers::default(), cx);
                    }
                }),
            );
        let cover = track_columns::cover_thumb(
            &self.state,
            Some(t.path.as_path()),
            self.column_shown("cover"),
            cx,
        );
        let cell = track_columns::Cell {
            pos: t.pos,
            title: &t.title,
            artist: &t.artist,
            album: &t.album,
            title_reading: &t.title_reading,
            artist_reading: &t.artist_reading,
            album_reading: &t.album_reading,
            year: t.year,
            genre: &t.genre,
            duration_ms: t.duration_ms,
            rating: t.rating,
            track_id: t.track_id.unwrap_or(0),
            favourite,
            playing: false,
            plays: t.plays,
            cover,
        };
        // The source mark replaces the number on a row that didn't come off disk.
        let mark = origin_glyph(t.origin);
        for col in columns() {
            if !self.column_shown(col.key) {
                continue;
            }
            if !has_track && (col.key == "rating" || col.key == "favourite") {
                continue;
            }
            if let Some(path) = mark.filter(|_| col.key == "number") {
                row = row.child(mark_slot(path, palette::text_muted()));
                continue;
            }
            if let Some(c) = track_columns::cell(col.key, &cell, &self.state, ROW_H, false) {
                row = row.child(c);
            }
        }
        row
    }
}

impl ColumnHost for QueuePanel {
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
        self.save_config(cx);
        cx.notify();
    }
}

impl HeadingHost for QueuePanel {
    fn headers(&self) -> Headers {
        self.config.headers
    }

    /// The queue is unchanged, so only the row plan rebuilds.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.config.headers == headers {
            return;
        }
        self.config.headers = headers;
        self.rebuild_rows();
        self.save_config(cx);
        cx.notify();
    }
}

impl QueryFilter for QueuePanel {
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

impl PanelSettings for QueuePanel {
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
        &[("View", icons::ROWS_3)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                rox_i18n::t!("library-columns"),
                Some(rox_i18n::t!("panel-columns-description")),
                None,
                track_columns::checklist(&columns(), self, cx),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-headings"),
                Some(rox_i18n::t!("queue-headings")),
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
            .into_any_element()
    }

    fn behavior(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        Some(crate::query::shared_query::search_section(
            self.config.search,
            |this: &mut Self, on, cx| this.set_search(on, cx),
            self.config.query_source,
            |this: &mut Self, source, cx| {
                this.pick_query_source(source, cx);
                this.save_config(cx);
            },
            cx,
        ))
    }
}

impl EventEmitter<PanelEvent> for QueuePanel {}

impl Focusable for QueuePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for QueuePanel {
    fn panel_name(&self) -> &'static str {
        "queue"
    }

    rox_panel_api::opens_settings!();

    fn accepts_drop(&self, cx: &App) -> bool {
        cx.active_drag_is::<PlayDrag>() || cx.active_drag_is::<ExternalPaths>()
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("queue-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

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

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

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
        let clear = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("queue-clear"))
                .icon(Icon::default().path(icons::TRASH))
                .disabled(self.tracks.is_empty())
                .on_click(move |_, _, cx| {
                    if let Some(this) = clear.upgrade() {
                        this.update(cx, |this, cx| this.clear(cx));
                    }
                }),
        );
        let menu = menu
            .label(rox_i18n::t!("panel-menu-display"))
            .item(PopupMenuItem::submenu(
                rox_i18n::t!("library-columns"),
                track_columns::columns_submenu(columns(), window, cx),
            ))
            .item(PopupMenuItem::submenu(
                rox_i18n::t!("panel-headings"),
                track_columns::headings_submenu(window, cx),
            ));
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this: &mut Self, source, cx| {
                this.pick_query_source(source, cx);
                this.save_config(cx);
            },
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
                QueuePanel::new(state, config, window, cx)
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

impl Render for QueuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl QueuePanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
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
        // The now-playing strip, drawn through the same columns as the rows.
        let root = if let Some(p) = self.playing.as_ref() {
            let has_track = p.track_id.is_some();
            let favourite = p
                .track_id
                .map(|id| self.favourites.contains(&id))
                .unwrap_or(false);
            let cover = track_columns::cover_thumb(
                &self.state,
                Some(p.path.as_path()),
                self.column_shown("cover"),
                cx,
            );
            let cell = track_columns::Cell {
                pos: 0,
                title: &p.title,
                artist: &p.artist,
                album: &p.album,
                title_reading: &p.title_reading,
                artist_reading: &p.artist_reading,
                album_reading: &p.album_reading,
                year: p.year,
                genre: &p.genre,
                duration_ms: p.duration_ms,
                rating: p.rating,
                track_id: p.track_id.unwrap_or(0),
                favourite,
                playing: true,
                plays: p.plays,
                cover,
            };
            let mut strip = div()
                .flex_none()
                .w_full()
                .h(palette::scaled_px(ROW_H))
                .px(tokens::SPACE_SM)
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .bg(palette::alpha(palette::highlight(), 0x12));
            for col in columns() {
                if !self.column_shown(col.key) {
                    continue;
                }
                if !has_track && (col.key == "rating" || col.key == "favourite") {
                    continue;
                }
                let c = if col.key == "number" {
                    // Right-aligned in the number's width, so the titles line up.
                    mark_slot(icons::PLAY, palette::accent())
                } else {
                    match track_columns::cell(col.key, &cell, &self.state, ROW_H, false) {
                        Some(c) => c,
                        None => continue,
                    }
                };
                strip = strip.child(c);
            }
            root.child(strip)
        } else {
            root
        };
        let content = if self.rows.is_empty() {
            let message = if !self.tracks.is_empty() {
                rox_i18n::t!("picker-no-matches")
            } else {
                rox_i18n::t!("queue-empty")
            };
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(tokens::SPACE_MD)
                    .text_center()
                    .text_color(palette::text_faint())
                    .child(message),
            )
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list("queue-rows", self.rows.len(), move |range, _, cx| {
                        this.upgrade()
                            .map(|this| this.update(cx, |this, cx| this.list_rows(range, cx)))
                            .unwrap_or_default()
                    })
                    .track_scroll(self.scroll.clone())
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&self.scroll)),
                )
        };
        // A drop anywhere on the body enqueues; a row's own handler catches its
        // drop first.
        let content = content
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x0f))
            })
            .on_drop(cx.listener(move |this, drag: &PlayDrag, _, cx| {
                this.enqueue_dropped(drag, cx);
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x0f))
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.enqueue_external(paths, cx);
            }));
        let content =
            content.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            // The right press already pulled the entry into the set.
            let target = {
                let panel = this.read(cx);
                let entry = panel
                    .menu_row
                    .filter(|e| panel.tracks.iter().any(|t| t.entry_id == *e));
                entry.map(|entry| {
                    let track_ids: Vec<i64> = panel
                        .tracks
                        .iter()
                        .filter(|t| panel.selected.contains(&t.entry_id))
                        .filter_map(|t| t.track_id)
                        .collect();
                    (entry, track_ids, panel.selected.len().max(1))
                })
            };
            let Some((entry, track_ids, count)) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let jump_panel = weak.clone();
            let remove_panel = weak.clone();
            let remove_label = rox_i18n::t!("queue-remove", count = count as u64).to_string();
            let mut menu = menu
                .item(
                    PopupMenuItem::new(rox_i18n::t!("library-play"))
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = jump_panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.state.player.read(cx).play_queued(entry)
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(remove_label)
                        .icon(Icon::default().path(icons::CLOSE))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = remove_panel.upgrade() {
                                this.update(cx, |this, cx| this.remove(entry, cx));
                            }
                        }),
                );
            if !track_ids.is_empty() {
                let state = this.read(cx).state.clone();
                menu = panel::track_actions(
                    menu.separator(),
                    state,
                    track_ids,
                    rox_i18n::t!("queue-play-now"),
                    window,
                    cx,
                    {
                        let panel = weak.clone();
                        move |_, cx| {
                            if let Some(this) = panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    this.state.player.read(cx).play_queued(entry)
                                });
                            }
                        }
                    },
                );
            }
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
