//! The play queue panel (ADR 16): the explicit up-next queue, what Play Next
//! and Add to Queue put ahead of the playing track. The album or library you
//! started from plays on as the context and is not listed here, so the queue
//! stays what you hand-picked; a now-playing strip heads the numbered rows
//! so the panel says where the queue picks up from. Rows play now on double
//! click, drop from the right-click menu, and drag to reorder. Its own
//! panel, never a mode of the library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, Entity, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, SharedString,
    Stateful, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_library::projection::{parse_query, track_matches, FilterSet, TrackFields};
use rox_library::store::TrackMeta;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::group_head::Headers;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::query::search::{SearchBox, SearchEvent};
use crate::settings::Settings;
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};
use crate::track_ui::track_drag::PlayDrag;

/// One row's height; the list is a uniform_list, so every row agrees.
const ROW_H: f32 = 30.;

/// The track columns, in render order. Number here is the queue position.
/// Every key is one the shared [`track_columns::cell`] draws.
const COLUMNS: &[Column] = &[
    Column { key: "cover", label: "Cover", default_on: false },
    Column { key: "number", label: "Number", default_on: true },
    Column { key: "name", label: "Name", default_on: true },
    Column { key: "artist", label: "Artist", default_on: true },
    Column { key: "album", label: "Album", default_on: false },
    Column { key: "year", label: "Year", default_on: false },
    Column { key: "genre", label: "Genre", default_on: false },
    Column { key: "duration", label: "Duration", default_on: false },
    Column { key: "plays", label: "Plays", default_on: false },
    Column { key: "rating", label: "Rating", default_on: false },
    Column { key: "favourite", label: "Favourite", default_on: false },
];

/// The queue panel's config: the shared chrome, the album heading mode, and
/// which per-track columns show.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The album heading mode over the queue; Off by default.
    pub headers: Headers,
    /// The shown column keys; defaults to the registry's default-on set.
    pub columns: Vec<String>,
    /// Whether the search box shows; the query only filters while it does.
    #[serde(default)]
    pub search: bool,
    /// Follow the shared query, or filter by this panel's own box.
    #[serde(default)]
    pub query_source: QuerySource,
    /// The panel's own query, kept while following the shared one so the
    /// switch back to own has something to restore.
    #[serde(default)]
    pub query: String,
}

/// Where a queue panel's view edits (columns, headings) are saved. A docked
/// panel's config rides the layout dump, nudged through its host tab panel;
/// the modal and popped-out queue the widget opens have no layout behind
/// them, so they keep their view in settings instead and read it back on the
/// next open.
#[derive(Clone, Copy, PartialEq)]
enum Persist {
    Layout,
    Settings,
}

// Hand-written so the columns default to the registry set for a new panel
// and a pre-columns layout alike.
impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig {
            chrome: PanelChrome::default(),
            headers: Headers::Off,
            columns: track_columns::default_columns(COLUMNS),
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
        }
    }
}

/// A flattened display row: an album heading, or a queue entry at its index
/// into `tracks`.
enum QRow {
    Album(u32),
    AlbumMeta(u32),
    Track(u32),
}

/// One resolved queue entry: the entry's stable id (what edits and the
/// selection address), its track id for the editors and rating, its position,
/// and the tags to draw. A queued file that has left the library resolves to
/// just its file name.
struct TrackRow {
    entry_id: u64,
    track_id: Option<i64>,
    pos: u32,
    title: String,
    artist: String,
    album: String,
    album_artist: String,
    year: u16,
    genre: String,
    codec: String,
    bitrate_kbps: u16,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: PathBuf,
}

/// A queue entry's grouping inputs, borrowed for the album run aggregate. A
/// track id of 0 stands in for a queued file the library does not know, so it
/// never matches a real album's art.
fn group_track(t: &TrackRow) -> GroupTrack<'_> {
    GroupTrack {
        album: &t.album,
        album_artist: &t.album_artist,
        artist: &t.artist,
        year: t.year,
        genre: &t.genre,
        codec: &t.codec,
        bitrate_kbps: t.bitrate_kbps,
        duration_ms: t.duration_ms,
        track_id: t.track_id.unwrap_or(0),
    }
}

/// The playing track's display data, for the now-playing strip that heads
/// the queue. Not a queue entry - the playing song plays on as context - so
/// it resolves apart from the rows, but through the same columns so the strip
/// lines up with them.
struct Playing {
    track_id: Option<i64>,
    title: String,
    artist: String,
    album: String,
    year: u16,
    genre: String,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: PathBuf,
}

/// The value carried through a row drag: the entries being moved, in queue
/// order, and the grabbed row's title for the drag preview. Dragging a row
/// inside a multi-selection carries the whole set; outside it, just that row.
#[derive(Clone)]
struct QueueDrag {
    ids: Arc<[u64]>,
    title: SharedString,
}

/// The label that floats under the pointer while a queue row is dragged. A
/// multi-row drag shows the grabbed title with a count of the rest.
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
    /// Where view edits land: the layout dump for a docked panel, or settings
    /// for the widget's windowed queue.
    persist: Persist,
    /// The resolved queue entries in order.
    tracks: Vec<TrackRow>,
    /// The search box, shared by every searching view; shown per config.
    search: Entity<SearchBox>,
    /// A pending box reset from a source toggle or a shared-query change,
    /// applied on the next render where a window exists to set the input.
    resync_box: bool,
    /// The query and filter the rows are built for, snapshotted whenever the
    /// query changes so `rebuild_rows` filters without a `cx`.
    applied_query: String,
    applied_filter: FilterSet,
    /// The display rows over `tracks`: the matching entries flat, or broken
    /// by album headings.
    rows: Vec<QRow>,
    /// The album runs the heading rows index, rebuilt with `rows`.
    albums: Vec<track_columns::AlbumGroup>,
    /// The favourited track ids, what each row's heart checks against.
    favourites: HashSet<i64>,
    /// Loose tags for queued files the library does not know, read off the
    /// file with `read_one` and cached per path so each is read at most once.
    /// None marks a file that could not even be stat'd, so it is not retried;
    /// the rebuild that resolves the rows runs off the pump, not per frame, so
    /// this IO stays off the render path. Rev-keyed rebuilds reuse it.
    loose_tags: HashMap<PathBuf, Option<rox_library::TrackRow>>,
    /// The playing track's title and artist, for the now-playing strip
    /// heading the list. Follows whatever plays, queued or context, so the
    /// panel always says where the queue picks up from.
    playing: Option<Playing>,
    /// The last queue revision the rows were built from; with the playing path,
    /// the cheap change detector so the per-pump observe only re-reads the
    /// queue when an edit lands or a track advances (which shrinks the queue).
    rev: Option<u64>,
    playing_path: Option<PathBuf>,
    /// The selected entries, by entry id, so a rebuild or a regroup keeps the
    /// highlight on the same entries wherever they land. Shift extends, cmd
    /// (ctrl elsewhere) toggles, Ctrl+A takes the lot, the library's rules.
    selected: HashSet<u64>,
    /// Bumped whenever the selection or the row order changes, keying the
    /// drag-set cache so a grab inside a big selection shares one Arc across
    /// every visible selected row instead of rescanning the rows per row.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[u64]>)>,
    /// Where the next shift-click extends from: the last plain or toggle
    /// pick, held as an entry id so it survives a rebuild too.
    anchor: Option<u64>,
    /// The entry under the last right press, for the context menu.
    menu_row: Option<u64>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
}

impl QueuePanel {
    pub fn new(
        state: AppState,
        config: QueueConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| this.sync(cx));
        // A landing cover repaints the heading tiles and the cover column.
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        // A retag or rescan changes the tags a row draws, a rating or
        // favourite change moves those columns; force a rebuild.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(
                    event,
                    LibraryEvent::Updated | LibraryEvent::Rated | LibraryEvent::PlaylistsChanged
                ) {
                    this.rev = None;
                    this.sync(cx);
                }
            },
        );
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: re-filter and reset the box
        // to it on the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        let mut this = QueuePanel {
            state,
            config,
            persist: Persist::Layout,
            tracks: Vec::new(),
            search,
            resync_box: false,
            applied_query: String::new(),
            applied_filter: FilterSet::default(),
            rows: Vec::new(),
            albums: Vec::new(),
            favourites: HashSet::new(),
            loose_tags: HashMap::new(),
            playing: None,
            rev: None,
            playing_path: None,
            selected: HashSet::new(),
            drag_gen: 0,
            drag_set: None,
            anchor: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _library_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
        };
        this.sync(cx);
        this
    }

    /// The widget's queue window: the modal and the popped-out queue, which
    /// have no dock layout behind them. Reads its view from settings and
    /// writes edits back there, so its columns and headings survive a close
    /// and a relaunch the way a docked panel's ride the layout dump.
    pub fn windowed(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = Settings::load()
            .queue_view
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut this = Self::new(state, config, window, cx);
        this.persist = Persist::Settings;
        this
    }

    /// Save a view edit. A docked panel nudges its host tab panel so the
    /// workspace's debounced layout save picks the change up; the panel's own
    /// events never reach the dock, but the tab panel's do. The windowed
    /// queue writes its config to settings instead. Without this a column or
    /// heading flip only reaches disk on a clean close, so a relaunch can lose
    /// it.
    fn save_config(&self, cx: &mut Context<Self>) {
        match self.persist {
            Persist::Layout => {
                if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
                    tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                }
            }
            Persist::Settings => {
                let value = serde_json::to_value(self.config.clone()).ok();
                Settings::update(move |s| s.queue_view = value);
            }
        }
    }

    /// Re-read the explicit queue. Bails on the cheap revision and playing-path
    /// compare, so a steady queue costs two reads per tick and nothing else;
    /// rebuilds the rows only when an edit bumps the revision or a track
    /// advance drops a played item off the front.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.player.read(cx).queue_rev();
        let playing_path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if rev == self.rev && playing_path == self.playing_path {
            return;
        }
        self.rev = rev;
        self.playing_path = playing_path;
        let queued = self.state.player.read(cx).queued();
        let library = self.state.library.read(cx);
        // Resolve every queued and playing file to its id and tags once, in a
        // single query each. The plays, loose-tag, and row passes below all read
        // from this instead of hitting id_for and meta_for twice apiece per
        // entry, which was four round trips a row on every rebuild.
        let resolved: Vec<Option<(i64, TrackMeta)>> =
            queued.iter().map(|e| library.resolve_path(&e.path)).collect();
        let playing_resolved: Option<(i64, TrackMeta)> = self
            .playing_path
            .as_ref()
            .and_then(|path| library.resolve_path(path));
        // Total play counts for the queue's tracks and the playing one, one
        // projection pass, for the plays column.
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
        // Read loose tags for every queued or playing file the library does
        // not know, once, before the resolve passes read the cache. Each
        // read_one hits the disk, so it is gated on a rev-change rebuild here
        // and cached per path; a steady queue reads nothing.
        for (path, meta) in queued
            .iter()
            .map(|e| &e.path)
            .zip(resolved.iter())
            .chain(self.playing_path.as_ref().map(|p| (p, &playing_resolved)))
        {
            if meta.is_none() && !self.loose_tags.contains_key(path) {
                self.loose_tags
                    .insert(path.clone(), rox_library::scanner::read_one(path));
            }
        }
        self.playing = self.playing_path.as_ref().map(|path| {
            let track_id = playing_resolved.as_ref().map(|(id, _)| *id);
            let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
            match playing_resolved {
                Some((_, m)) => Playing {
                    track_id,
                    title: m.title,
                    artist: m.artist,
                    album: m.album,
                    year: m.year,
                    genre: m.genre,
                    duration_ms: m.duration_ms,
                    rating: m.rating,
                    plays: count,
                    path: path.clone(),
                },
                // Out of library: the playing file's own tags, read and
                // cached above; the file name alone when it could not be stat'd.
                None => match self.loose_tags.get(path).and_then(Option::as_ref) {
                    Some(r) => Playing {
                        track_id,
                        title: r.title.clone(),
                        artist: r.artist.clone(),
                        album: r.album.clone(),
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
            .zip(resolved)
            .enumerate()
            .map(|(i, (entry, resolved))| {
                let track_id = resolved.as_ref().map(|(id, _)| *id);
                let pos = (i + 1) as u32;
                let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
                match resolved {
                    Some((_, m)) => TrackRow {
                        entry_id: entry.id,
                        track_id,
                        pos,
                        title: m.title,
                        artist: m.artist,
                        album: m.album,
                        album_artist: m.album_artist,
                        year: m.year,
                        genre: m.genre,
                        codec: m.codec,
                        bitrate_kbps: m.bitrate_kbps,
                        duration_ms: m.duration_ms,
                        rating: m.rating,
                        plays: count,
                        path: entry.path.clone(),
                    },
                    // Out of library: draw the file's own tags, read off the
                    // disk and cached above. Fall back to just the file name
                    // only when the file could not be stat'd.
                    None => match self.loose_tags.get(&entry.path).and_then(Option::as_ref) {
                        Some(r) => TrackRow {
                            entry_id: entry.id,
                            track_id,
                            pos,
                            title: r.title.clone(),
                            artist: r.artist.clone(),
                            album: r.album.clone(),
                            album_artist: r.album_artist.clone(),
                            year: r.year,
                            genre: r.genre.clone(),
                            codec: r.codec.clone(),
                            bitrate_kbps: r.bitrate_kbps,
                            duration_ms: r.duration_ms,
                            rating: 0,
                            plays: count,
                            path: entry.path.clone(),
                        },
                        None => TrackRow {
                            entry_id: entry.id,
                            track_id,
                            pos,
                            title: file_label(&entry.path),
                            artist: String::new(),
                            album: String::new(),
                            album_artist: String::new(),
                            year: 0,
                            genre: String::new(),
                            codec: String::new(),
                            bitrate_kbps: 0,
                            duration_ms: 0,
                            rating: 0,
                            plays: count,
                            path: entry.path.clone(),
                        },
                    },
                }
            })
            .collect();
        // Selection and anchor ride by entry id, so a reorder's dragged set
        // (kept selected in `reorder`) stays lit at its new spot; prune only
        // what left the queue.
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

    /// Snapshot the active query and filter, so `rebuild_rows` filters the
    /// entries without a `cx`. The shared query while following it, the box's
    /// own text otherwise.
    fn refresh_query(&mut self, cx: &Context<Self>) {
        self.applied_query = self.effective_query(cx);
        self.applied_filter = self.effective_filter(cx);
    }

    /// Whether a queue entry passes the active query and filter.
    fn matches(&self, terms: &[rox_library::projection::Term], t: &TrackRow) -> bool {
        let fields = TrackFields {
            title: &t.title,
            artist: &t.artist,
            album_artist: &t.album_artist,
            album: &t.album,
            genre: &t.genre,
            year: t.year,
            path: t.path.to_str().unwrap_or_default(),
        };
        track_matches(terms, &fields) && self.applied_filter.matches(&fields)
    }

    /// Lay the display rows over the entries that pass the active query:
    /// flat, or broken into album runs with a heading over each. A headings,
    /// column, or query flip that leaves the queue itself alone calls this,
    /// not `sync`.
    fn rebuild_rows(&mut self) {
        // The row order drives drag order, so a rebuild invalidates the cached
        // drag set even when the selected ids are unchanged.
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

    /// Map the shared box's events onto the queue: a changed query re-filters,
    /// and a focus or dismiss repaints the tab title row where the box lives.
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

    /// Show or hide the panel's own search box, re-filtering and persisting.
    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.save_config(cx);
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// The entry id at a display row, if it is a track row.
    fn entry_at(&self, ix: usize) -> Option<u64> {
        match self.rows.get(ix)? {
            QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
            _ => None,
        }
    }

    /// The display row index of an entry, if it is on screen.
    fn index_of(&self, entry: u64) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(row, QRow::Track(ti)
                if self.tracks.get(*ti as usize).map(|t| t.entry_id) == Some(entry))
        })
    }

    /// The selected entries in view order, so a drag or remove keeps the
    /// order you see rather than a set's arbitrary one.
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

    /// The multi-selection drag set as a shared Arc, resolved through
    /// `selected_ids` once per selection or row change and cached after.
    fn drag_ids(&mut self) -> Arc<[u64]> {
        if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.drag_gen) {
            let ids: Arc<[u64]> = self.selected_ids().into();
            self.drag_set = Some((self.drag_gen, ids));
        }
        self.drag_set
            .as_ref()
            .map(|(_, ids)| ids.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    /// Put a click on a track row: plain selects just it, shift extends from
    /// the anchor over the tracks between, cmd (ctrl elsewhere) toggles - the
    /// library's rules, keyed on the entry id so a rebuild keeps the mark.
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
            // Ctrl+Shift stacks the range onto the selection so you can
            // skip a run and grab a second block; plain shift replaces.
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

    /// Ctrl+A: take every visible entry - the filter's rows, so selection
    /// matches what Delete removes. Anchors at the first so a follow-up
    /// shift-click narrows from the top.
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

    /// Resolve the selected entries to track ids in queue order and publish
    /// them on the shared selection for the panels that display it.
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
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }

    /// A double click plays that entry now. Through the player's
    /// move-then-jump, so the entries above it stay queued instead of falling
    /// behind the cursor as history and vanishing from the panel.
    fn jump(&self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.entry_at(ix) else {
            return;
        };
        self.state.player.read(cx).play_queued(id);
    }

    /// Clear Queue: drop every entry. Through `remove_ids`, so the panel
    /// empties right away even while paused.
    fn clear(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self.tracks.iter().map(|t| t.entry_id).collect();
        self.remove_ids(&ids, cx);
    }

    /// Drop a set of queued entries by id, from our own tracks right away too,
    /// so the change shows even while paused, when the player's pump is quiet
    /// and the sync that would rebuild from the engine does not run; the next
    /// sync reconciles against the engine either way. The lowest removed spot
    /// keeps the mark, so a run of deletes stays put.
    fn remove_ids(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        self.state
            .player
            .read(cx)
            .remove_many_from_queue(ids.to_vec());
        // Set membership, not a linear scan per track: a full clear runs this
        // over every row, so `contains` on a slice would be O(n^2).
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

    /// The context menu's remove: the whole selection when the clicked entry
    /// is part of it, else just that entry.
    fn remove(&mut self, entry: u64, cx: &mut Context<Self>) {
        let ids = if self.selected.contains(&entry) {
            self.selected_ids()
        } else {
            vec![entry]
        };
        self.remove_ids(&ids, cx);
    }

    /// Delete or Backspace drops the selected rows. Ctrl+A takes the whole
    /// queue.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let ids = self.selected_ids();
            self.remove_ids(&ids, cx);
        }
    }

    /// Rows dropped onto position `target`: move them to just before that row,
    /// i.e. right after the nearest queued entry above it that isn't one of the
    /// dragged ones. At the top of the queue there is none, so anchor to the
    /// playing track, which lands the group at the front of the queue rather
    /// than the front of the whole timeline. A multi-row drag chains each entry
    /// after the last, so the group keeps its queue order and lands as one
    /// contiguous run.
    fn reorder(&mut self, dragged: &[u64], target: usize, cx: &mut Context<Self>) {
        if dragged.is_empty() {
            return;
        }
        // The nearest entry above the target display row that is not itself
        // dragged; heading rows carry no entry, so they are skipped.
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
        // Keep the moved group lit once the reordered queue rebuilds; the
        // entry-id selection rides the rebuild on its own.
        self.selected = dragged.iter().copied().collect();
        self.anchor = dragged.first().copied();
        self.drag_gen += 1;
    }

    /// A track dragged in from the library (or another play-drag source)
    /// enqueues on drop, appended after the queue, the Add to Queue
    /// semantics. Enqueue, not Play Next, so a drop lands the tracks at the
    /// back rather than jumping them ahead of what is already queued. The
    /// paths ride bare through the player, which handles out-of-library files.
    fn enqueue_dropped(&mut self, drag: &PlayDrag, cx: &mut Context<Self>) {
        if drag.is_empty() {
            return;
        }
        let paths = drag.paths.to_vec();
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(paths, cx));
    }

    /// An OS file dropped onto the queue panel enqueues, same as a track
    /// dragged in from the library. The window body plays drops now, so the
    /// queue panel stays the one surface that adds without interrupting.
    fn enqueue_external(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let paths = crate::open_files::resolve_audio_paths(paths.paths().to_vec());
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(paths, cx));
    }

    /// The visible slice of the list: album headings and queue entries, drawn
    /// through the shared column surface.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        // The whole multi-selection drag set, resolved once per frame (and
        // cached across frames until the selection or rows move) so a grab
        // inside it hands every selected row one shared Arc, not a rescan each.
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
                            &self.state,
                            cx,
                        )
                    }
                    QRow::AlbumMeta(g) => {
                        let g = *g;
                        track_columns::album_meta_row(ix, &mut self.albums[g as usize], &self.state, cx)
                    }
                    QRow::Track(ti) => {
                        let ti = *ti as usize;
                        self.track_row(ix, ti, multi_drag.as_ref(), cx)
                    }
                })
            })
            .collect()
    }

    /// One queue entry row: reorder-drag, multi-select, and remove keyed on
    /// the entry id, its cells the shown columns. A queued file the library
    /// does not know shows no rating or favourite.
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
        // Dragging a row inside a multi-selection carries the whole set in
        // queue order, the shared Arc `list_rows` resolved once; outside it,
        // just this entry.
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
            // The hover group the rating and favourite cells reveal on.
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(px(ROW_H))
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
            // A track dragged in from the library (or elsewhere) enqueues on
            // drop. gpui dispatches on_drop by payload type, so this sits
            // alongside the reorder drop above rather than replacing it.
            .on_drop(cx.listener(move |this, drag: &PlayDrag, _, cx| {
                this.enqueue_dropped(drag, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // Take focus so Delete on the selection reaches the panel's
                    // key handler.
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.jump(ix, cx);
                    } else if event.modifiers.shift || event.modifiers.secondary() {
                        this.select(ix, event.modifiers, cx);
                    } else if !this.selected.contains(&entry) {
                        // A plain press on an unselected row picks it now, so a
                        // drag from here carries it. A press on an already-lit
                        // row keeps the set for a whole-group drag.
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A plain click that never became a drag collapses a
                // multi-selection down to the row clicked.
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
                    // A right click outside the set reselects just that row, so
                    // the menu acts on what is lit.
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
        for col in COLUMNS {
            if !self.column_shown(col.key) {
                continue;
            }
            if !has_track && (col.key == "rating" || col.key == "favourite") {
                continue;
            }
            if let Some(c) = track_columns::cell(col.key, &cell, &self.state) {
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

    /// Set the heading mode and relay out the rows; the queue is unchanged,
    /// so no re-read, just a fresh row plan.
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

    /// The View page: the column checklist and the album heading mode.
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
                "Columns",
                Some("Which track columns show"),
                None,
                track_columns::checklist(COLUMNS, self, cx),
            ))
            .child(panel::setting_row(
                "Headings",
                Some("Break the queue into album runs; Expanded adds the cover and stats"),
                panel::choices(
                    &[
                        ("Off", Headers::Off),
                        ("Compact", Headers::Compact),
                        ("Expanded", Headers::Expanded),
                    ],
                    self.config.headers,
                    |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    /// The Behavior page's search section: show the box, and follow the
    /// shared query or filter by the panel's own, the searching views' knob.
    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Queue")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The search box shares the title bar row while the panel sits in a
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
            PopupMenuItem::new("Clear Queue")
                .icon(Icon::default().path(icons::TRASH))
                .disabled(self.tracks.is_empty())
                .on_click(move |_, _, cx| {
                    if let Some(this) = clear.upgrade() {
                        this.update(cx, |this, cx| this.clear(cx));
                    }
                }),
        );
        // Display section: the view knobs under their own label, ahead of the
        // Panel section, the library's shape.
        let menu = menu
            .label("Display")
            .item(PopupMenuItem::submenu(
                "Columns",
                track_columns::columns_submenu(COLUMNS, window, cx),
            ))
            .item(PopupMenuItem::submenu(
                "Headings",
                track_columns::headings_submenu(window, cx),
            ));
        // Follow the shared search query, or filter by this panel's own box.
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
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let menu = panel::duplicate_item(menu, &cx.entity(), self.tab_panel.clone(), |this, window, cx| {
            let (state, config) = {
                let panel = this.read(cx);
                (panel.state.clone(), panel.config.clone())
            };
            QueuePanel::new(state, config, window, cx)
        });
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
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
        // A pending box reset (a source toggle or a shared-query change)
        // lands here, where a window exists to set the input's text.
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
        // The now-playing strip heading the list: display only, the marker
        // the numbered rows queue up behind. Drawn through the same columns as
        // the rows so it lines up with them, with a play icon standing in for
        // the number and the playing track's own cover.
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
                .h(px(ROW_H))
                .px(tokens::SPACE_SM)
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .bg(palette::alpha(palette::highlight(), 0x12));
            for col in COLUMNS {
                if !self.column_shown(col.key) {
                    continue;
                }
                if !has_track && (col.key == "rating" || col.key == "favourite") {
                    continue;
                }
                let c = if col.key == "number" {
                    // The play icon takes the number's slot, right-aligned in
                    // the same width so the title lines up with the rows'.
                    div()
                        .flex_none()
                        .w(px(22.))
                        .flex()
                        .justify_end()
                        .items_center()
                        .child(
                            svg()
                                .path(icons::PLAY)
                                .size(px(12.))
                                .text_color(palette::accent()),
                        )
                } else {
                    match track_columns::cell(col.key, &cell, &self.state) {
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
            // Entries hidden by the query read differently from an empty queue.
            let message = if !self.tracks.is_empty() {
                "No matches"
            } else {
                "Queue is empty"
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
        // A drop over the body, including the empty-queue message and the
        // space below the rows, enqueues too, so a drag need not land on a
        // row. A drop on a row is caught by the row's own handler first.
        let content = content
            .drag_over::<PlayDrag>(|style, _, _, _| style.bg(palette::alpha(palette::accent(), 0x0f)))
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
            // The clicked entry plus the selection it acts on. The right press
            // already pulled the entry into the set, so this is what is lit.
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
            let remove_label = if count > 1 {
                format!("Remove {count} from Queue")
            } else {
                "Remove from Queue".to_string()
            };
            let mut menu = menu
                .item(
                    PopupMenuItem::new("Play")
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
            // The shared edit/reveal actions when the entries are known tracks.
            if !track_ids.is_empty() {
                let state = this.read(cx).state.clone();
                menu = panel::track_actions(
                    menu.separator(),
                    state,
                    track_ids,
                    "Play Now",
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

/// A queued file's last path component as a fallback label, when the track
/// is not in the library to give a title.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
