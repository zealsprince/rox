//! The playlists panel (ADR 16): a tree of playlists, each expanding to its
//! tracks. A track plays the playlist from that point on double click, drops
//! from the right-click menu, and drags to another playlist to move there or
//! within its own to reorder. Playlists rename and delete from their own
//! right-click, and New Playlist lives in the panel menu. Its own panel, never
//! a mode of the library.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    PathPromptOptions, SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity,
    Window,
};
use gpui_component::button::Button;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::group_head::Headers;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::track_ui::track_cells;
use crate::track_ui::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};
use rox_library::playlists::PlaylistTrack;
use rox_library::projection::{parse_query, track_matches, FilterSet, Term, TrackFields};

/// One row's height; the list is a uniform_list, so every row agrees.
const ROW_H: f32 = 30.;

/// The track columns, in render order. The number and name lead, the rating
/// and favourite controls trail, the tag columns sit between. Which show is
/// the config's call; this only fixes the order and the default set. Every
/// key is one the shared [`track_columns::cell`] draws.
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
    Column { key: "rating", label: "Rating", default_on: true },
    Column { key: "favourite", label: "Favourite", default_on: true },
];

/// The playlists panel's config: the shared chrome, which playlists are
/// expanded so a saved layout restores the open ones, the album heading
/// mode, and which per-track columns show.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub expanded: Vec<i64>,
    /// The album heading mode over each expanded playlist, the library's
    /// grouping brought to the tree. Off by default; a playlist stays a flat
    /// list unless you ask for the headings.
    pub headers: Headers,
    /// The shown column keys, in no particular order (render order is the
    /// registry's). Defaults to the registry's default-on set, so a fresh
    /// panel and a pre-columns layout both open with the same fields.
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

// Hand-written over derived so the columns default to the registry set and
// the headings default off, both for a new panel and for a saved layout from
// before these existed (the container's serde default fills a missing field
// from here).
impl Default for PlaylistsConfig {
    fn default() -> Self {
        PlaylistsConfig {
            chrome: PanelChrome::default(),
            expanded: Vec::new(),
            headers: Headers::Off,
            columns: track_columns::default_columns(COLUMNS),
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
        }
    }
}

/// A flattened tree row: a playlist header, or one of its tracks.
enum Row {
    Head {
        id: i64,
        name: String,
        count: u64,
        expanded: bool,
        /// The one default playlist behind the heart column: shown with a
        /// heart, shielded from rename and delete.
        favourite: bool,
    },
    /// The name line of an album heading inside an expanded playlist,
    /// indexing [`PlaylistsPanel::albums`]. Built only when album headings
    /// are on, one block per run of tracks that share an album.
    Album(u32),
    /// The heading's second line, the stats under the name. Same index.
    AlbumMeta(u32),
    Track(TrackRow),
}

/// One track row's data, what its cells draw. Carries every column's value
/// so the render only reads the shown ones; the favourite is looked up live
/// off the panel's set, not stored here.
struct TrackRow {
    playlist_id: i64,
    member_id: i64,
    track_id: i64,
    /// The track's 1-based spot in its playlist, its play order. Runs
    /// unbroken through the album headings, so it counts the playlist, not
    /// each album.
    pos: u32,
    title: String,
    artist: String,
    album: String,
    year: u16,
    genre: String,
    duration_ms: u32,
    rating: u8,
    plays: u32,
    path: String,
}

impl TrackRow {
    /// Pull a member row's fields into a display row at a play-order spot,
    /// with the total play count resolved from the catalog.
    fn new(playlist_id: i64, pos: u32, t: &PlaylistTrack, plays: u32) -> TrackRow {
        TrackRow {
            playlist_id,
            member_id: t.member_id,
            track_id: t.track_id,
            pos,
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            year: t.year,
            genre: t.genre.clone(),
            duration_ms: t.duration_ms,
            rating: t.rating,
            plays,
            path: t.path.clone(),
        }
    }
}

/// A member row's grouping inputs, borrowed for the album run aggregate.
fn group_track(t: &PlaylistTrack) -> GroupTrack<'_> {
    GroupTrack {
        album: &t.album,
        album_artist: &t.album_artist,
        artist: &t.artist,
        year: t.year,
        genre: &t.genre,
        codec: &t.codec,
        bitrate_kbps: t.bitrate_kbps,
        duration_ms: t.duration_ms,
        track_id: t.track_id,
    }
}

/// A dragged set of members, in view order, and the grabbed row's title for
/// the preview. Dragging a row inside a multi-selection carries the whole set;
/// outside it, just that row. Where they land is the drop target's call, so no
/// source playlist rides along.
#[derive(Clone)]
struct TrackDrag {
    members: Arc<[i64]>,
    title: SharedString,
}

/// The label that floats under the pointer while tracks are dragged. A
/// multi-row drag shows the grabbed title with a count of the rest.
struct TrackDragPreview {
    title: SharedString,
    extra: usize,
}

impl Render for TrackDragPreview {
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

pub struct PlaylistsPanel {
    state: AppState,
    config: PlaylistsConfig,
    /// The search box, shared by every searching view; shown per config.
    search: Entity<SearchBox>,
    /// A pending box reset from a source toggle or a shared-query change,
    /// applied on the next render where a window exists to set the input.
    resync_box: bool,
    /// The query and filter the tree is built for, snapshotted whenever the
    /// query changes; a searching tree surfaces matches from every list.
    applied_query: String,
    applied_filter: FilterSet,
    rows: Vec<Row>,
    /// The album runs the heading rows index, rebuilt with `rows` each
    /// refresh; empty when the headings are off.
    albums: Vec<track_columns::AlbumGroup>,
    /// The expanded playlist ids, mirrored into the config on every change.
    expanded: HashSet<i64>,
    /// The favourited track ids, what each track row's heart checks against.
    /// Reloaded on every refresh, since a favourite toggle emits the same
    /// event a playlist edit does.
    favourites: HashSet<i64>,
    /// The playing track's library id, for the row highlight.
    playing: Option<i64>,
    /// The selected members, by row id. Keyed on the member id, not the row
    /// index, so a rescan, an expand, or a reorder rebuilds the tree without
    /// dropping the highlight. Shift extends, cmd (ctrl elsewhere) toggles,
    /// Ctrl+A takes the lot, the library's click rules.
    selected: HashSet<i64>,
    /// Bumped whenever the selection or the row order changes, keying the
    /// drag-set cache so a grab inside a big selection shares one Arc across
    /// every visible selected row instead of rescanning the tree per row.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[i64]>)>,
    /// Where the next shift-click extends from: the last plain or toggle pick,
    /// held as a member id so it survives a rebuild too.
    anchor: Option<i64>,
    menu_row: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
}

impl PlaylistsPanel {
    pub fn new(
        state: AppState,
        config: PlaylistsConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let expanded: HashSet<i64> = config.expanded.iter().copied().collect();
        // Playlist edits and rescans both change what the tree shows. A rating
        // click only moved one cell through the shared projection and never
        // reorders the tree, so patch it in place instead of reloading the
        // expanded lists.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Rated) {
                    this.patch_ratings(cx);
                    return;
                }
                if matches!(
                    event,
                    LibraryEvent::PlaylistsChanged | LibraryEvent::Updated
                ) {
                    this.refresh(cx);
                }
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        // A landing cover repaints the heading tiles; nothing to recompute.
        let _thumbs_changed =
            cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: rebuild the tree and reset the
        // box to it on the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        let mut this = PlaylistsPanel {
            state,
            config,
            search,
            resync_box: false,
            applied_query: String::new(),
            applied_filter: FilterSet::default(),
            rows: Vec::new(),
            albums: Vec::new(),
            expanded,
            favourites: HashSet::new(),
            playing: None,
            selected: HashSet::new(),
            drag_gen: 0,
            drag_set: None,
            anchor: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _player_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
        };
        this.refresh(cx);
        this.sync_playing(cx);
        this
    }

    /// Rebuild the flattened tree from the catalog: a header per playlist, its
    /// tracks under it when expanded. While a query is active every list opens
    /// and only its matching tracks show, so a search surfaces hits from
    /// collapsed lists too; a list with no match drops out entirely.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_query(cx);
        let terms = parse_query(&self.applied_query);
        let searching = !self.applied_query.is_empty() || !self.applied_filter.is_empty();
        let library = self.state.library.read(cx);
        let favourites = library.favourite_ids();
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        for playlist in library.playlists() {
            let expanded = self.expanded.contains(&playlist.id);
            // A searching tree loads every list to filter it; otherwise only
            // the expanded ones, which is what's on screen.
            let show_tracks = expanded || searching;
            let all = if show_tracks {
                library.playlist_tracks(playlist.id)
            } else {
                Vec::new()
            };
            // The original play-order index of each track that shows; a search
            // keeps only matches, so positions stay the playlist's, not the
            // filtered run's.
            let visible: Vec<usize> = if searching {
                (0..all.len())
                    .filter(|&i| self.track_visible(&terms, &all[i]))
                    .collect()
            } else {
                (0..all.len()).collect()
            };
            if searching && visible.is_empty() {
                continue;
            }
            rows.push(Row::Head {
                id: playlist.id,
                name: playlist.name,
                count: playlist.tracks,
                expanded: show_tracks,
                favourite: playlist.favourite,
            });
            if !show_tracks {
                continue;
            }
            // Total play counts for the shown tracks, one projection pass, for
            // the plays column.
            let ids: Vec<i64> = visible.iter().map(|&i| all[i].track_id).collect();
            let plays = library.plays_for(&ids);
            let plays_of = |t: &PlaylistTrack| plays.get(&t.track_id).copied().unwrap_or(0);
            if self.config.headers == Headers::Off {
                for &i in &visible {
                    rows.push(Row::Track(TrackRow::new(
                        playlist.id,
                        (i + 1) as u32,
                        &all[i],
                        plays_of(&all[i]),
                    )));
                }
                continue;
            }
            // A heading block opens each run of shown tracks that share an
            // album, in play order - no re-sort, so a playlist's own order
            // stays put and a mixed list just breaks more often. Consecutive
            // empty albums merge into one Unknown run, the library's rule.
            // Compact draws the name line alone, Expanded adds the meta line.
            let mut k = 0;
            while k < visible.len() {
                let mut m = k + 1;
                let head = &all[visible[k]];
                while m < visible.len()
                    && all[visible[m]].album == head.album
                    && all[visible[m]].album_artist == head.album_artist
                {
                    m += 1;
                }
                let group: Vec<GroupTrack> =
                    visible[k..m].iter().map(|&i| group_track(&all[i])).collect();
                albums.push(track_columns::album_group(&group));
                let g = (albums.len() - 1) as u32;
                rows.push(Row::Album(g));
                if self.config.headers == Headers::Expanded {
                    rows.push(Row::AlbumMeta(g));
                }
                for &i in &visible[k..m] {
                    rows.push(Row::Track(TrackRow::new(
                        playlist.id,
                        (i + 1) as u32,
                        &all[i],
                        plays_of(&all[i]),
                    )));
                }
                k = m;
            }
        }
        self.rows = rows;
        // The row order drives drag order, so a rebuild invalidates the cached
        // drag set even when the selected members are unchanged.
        self.drag_gen += 1;
        self.albums = albums;
        self.favourites = favourites;
        // Keep only members that still exist; a removed track drops out of the
        // selection, a moved one stays lit at its new spot.
        let live: HashSet<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.member_id),
                _ => None,
            })
            .collect();
        self.selected.retain(|member| live.contains(member));
        if self.anchor.is_some_and(|a| !live.contains(&a)) {
            self.anchor = None;
        }
        self.menu_row = None;
        cx.notify();
    }

    /// Re-read ratings for the visible track rows in place after a star click,
    /// instead of reloading the expanded playlists. The rating moved through
    /// the shared projection already; a track can sit in more than one open
    /// list, so every row that holds its id gets the new value, then repaints.
    fn patch_ratings(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.track_id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let ratings = self.state.library.read(cx).ratings_for(&ids);
        for row in &mut self.rows {
            if let Row::Track(t) = row {
                if let Some(&r) = ratings.get(&t.track_id) {
                    t.rating = r;
                }
            }
        }
        cx.notify();
    }

    /// Snapshot the active query and filter, so `refresh` filters the tree
    /// without a `cx`. The shared query while following it, the box's own
    /// text otherwise.
    fn refresh_query(&mut self, cx: &Context<Self>) {
        self.applied_query = self.effective_query(cx);
        self.applied_filter = self.effective_filter(cx);
    }

    /// Whether a playlist track passes the active query and filter.
    fn track_visible(&self, terms: &[Term], t: &PlaylistTrack) -> bool {
        let fields = TrackFields {
            title: &t.title,
            artist: &t.artist,
            album_artist: &t.album_artist,
            album: &t.album,
            genre: &t.genre,
            year: t.year,
            path: &t.path,
        };
        track_matches(terms, &fields) && self.applied_filter.matches(&fields)
    }

    /// Follow the player: resolve the playing path to its track id, so every
    /// row of that track across playlists carries the highlight.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let playing = self
            .state
            .player
            .read(cx)
            .now_playing()
            .and_then(|now| self.state.library.read(cx).id_for(&now.path));
        if playing != self.playing {
            self.playing = playing;
            cx.notify();
        }
    }

    /// Map the shared box's events onto the panel: a changed query rebuilds
    /// the tree, and a focus or dismiss repaints the tab title row where the
    /// box lives.
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

    /// Show or hide the panel's own search box, rebuilding the tree. The
    /// config rides the layout dump, so the tab-panel repaint carries it out.
    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// Nudge the dock to persist the layout after a config change it never sees
    /// on its own - a column toggle, heading flip, or expand. The panel's own
    /// events don't reach the dock, but its host tab panel's do, so bounce a
    /// LayoutChanged through it and the workspace's debounced save picks the
    /// change up. A plain tab-panel repaint isn't enough; only LayoutChanged
    /// arms the save. Without this the change only reaches disk on a clean
    /// close, so a relaunch can lose it.
    fn request_layout_save(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    /// Expand or collapse a playlist, mirroring the set into the config so a
    /// layout dump keeps it.
    fn toggle(&mut self, id: i64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.config.expanded = self.expanded.iter().copied().collect();
        self.request_layout_save(cx);
        self.refresh(cx);
    }

    /// Start the playlist playing, from `start_track` when given (a double
    /// click on a row), from the top otherwise (the header's Play).
    fn play(&self, playlist_id: i64, start_track: Option<i64>, cx: &mut Context<Self>) {
        let (paths, start) = {
            let library = self.state.library.read(cx);
            let ids = library.playlist_ids(playlist_id);
            let start = start_track
                .and_then(|t| ids.iter().position(|&x| x == t))
                .unwrap_or(0);
            (library.paths_for(&ids).unwrap_or_default(), start)
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, start, cx));
    }

    /// Write a playlist to an M3U8 file the user picks, named after it by
    /// default. Only playable members land in it; a deleted track has no file
    /// to point at.
    fn export(&self, playlist_id: i64, name: String, cx: &mut Context<Self>) {
        let rows = self
            .state
            .library
            .read(cx)
            .playlist_export_rows(playlist_id);
        if rows.is_empty() {
            return;
        }
        let text = rox_library::m3u::to_m3u8(&rows);
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.m3u8");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            if let Ok(Ok(Some(path))) = rx.await {
                std::fs::write(path, text).ok();
            }
        })
        .detach();
    }

    /// Pick an M3U file and load it as a new playlist named after the file.
    /// Entries resolve to catalog tracks, relative paths against the file's
    /// folder; paths the library never scanned are skipped.
    fn import(&self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };
            let entries = rox_library::m3u::parse(&text);
            if entries.is_empty() {
                return;
            }
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Imported".into());
            let base = path
                .parent()
                .map(|dir| dir.to_path_buf())
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.state.library.update(cx, |library, cx| {
                    library.import_playlist(&name, &base, &entries, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    /// The member id at a row, if it is a track row.
    fn member_at(&self, ix: usize) -> Option<i64> {
        match self.rows.get(ix) {
            Some(Row::Track(t)) => Some(t.member_id),
            _ => None,
        }
    }

    /// The row index of a member, if it is still on screen.
    fn index_of(&self, member: i64) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Track(t) if t.member_id == member))
    }

    /// The selected members in view order, so a drag or remove keeps the order
    /// you see rather than a set's arbitrary one.
    fn selected_members(&self) -> Vec<i64> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) if self.selected.contains(&t.member_id) => Some(t.member_id),
                _ => None,
            })
            .collect()
    }

    /// The multi-selection drag set as a shared Arc, resolved through
    /// `selected_members` once per selection or row change and cached after.
    fn drag_members(&mut self) -> Arc<[i64]> {
        if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.drag_gen) {
            let members: Arc<[i64]> = self.selected_members().into();
            self.drag_set = Some((self.drag_gen, members));
        }
        self.drag_set
            .as_ref()
            .map(|(_, members)| members.clone())
            .unwrap_or_else(|| Arc::from([]))
    }

    /// Put a click on a track row: plain selects just it, shift extends from
    /// the anchor over the tracks between, cmd (ctrl elsewhere) toggles - the
    /// library's click rules. Publishes the selection either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(member) = self.member_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor_ix = self.anchor.and_then(|a| self.index_of(a)).unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            // Only track rows in the span, so a header caught between two
            // playlists is skipped rather than selected.
            let range: Vec<_> = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    Row::Track(t) => Some(t.member_id),
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
                self.anchor = Some(member);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(member) {
                self.selected.remove(&member);
            }
            self.anchor = Some(member);
        } else {
            self.selected = HashSet::from([member]);
            self.anchor = Some(member);
        }
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    /// Ctrl+A: take every track across every open playlist. Anchors at the
    /// first so a follow-up shift-click narrows from the top.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let members = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) => Some(t.member_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        if members.is_empty() {
            return;
        }
        self.anchor = members.first().copied();
        self.selected = members.into_iter().collect();
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    /// Resolve the selected members to track ids in view order and publish them
    /// on the shared selection for the panels that display it.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(t) if self.selected.contains(&t.member_id) => Some(t.track_id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }

    /// Drop the given members. The library edit rebuilds the tree, and the
    /// refresh prunes them out of the selection.
    fn remove_members(&mut self, members: Vec<i64>, cx: &mut Context<Self>) {
        if members.is_empty() {
            return;
        }
        self.state.library.update(cx, |library, cx| {
            library.remove_playlist_members(&members, cx);
        });
    }

    /// Delete or Backspace drops the selected members. Ctrl+A takes every
    /// visible track.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }
        if key == "delete" || key == "backspace" {
            let members = self.selected_members();
            self.remove_members(members, cx);
        }
    }

    /// A dragged set dropped onto a row: onto a header, or a track, it lands as
    /// one block before the target (or at the end of a header's playlist),
    /// pulling in members from other playlists on the way. Dropping onto one of
    /// the dragged rows does nothing.
    fn drop_on(&mut self, drag: &TrackDrag, target: usize, cx: &mut Context<Self>) {
        let (playlist_id, before) = match self.rows.get(target) {
            Some(Row::Head { id, .. }) => (*id, None),
            Some(Row::Track(t)) => (t.playlist_id, Some(t.member_id)),
            // A heading is presentation, not a slot; drop on the tracks
            // around it.
            Some(Row::Album(_) | Row::AlbumMeta(_)) | None => return,
        };
        if before.is_some_and(|b| drag.members.contains(&b)) {
            return;
        }
        let members = drag.members.clone();
        self.state.library.update(cx, |library, cx| {
            library.place_playlist_members(playlist_id, &members, before, cx);
        });
    }

    /// The visible slice of the tree.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        // The whole multi-selection drag set, resolved once per frame (and
        // cached across frames until the selection or rows move) so a grab
        // inside it hands every selected row one shared Arc, not a rescan each.
        let multi_drag = (self.selected.len() > 1).then(|| self.drag_members());
        range
            .filter_map(|ix| {
                Some(match self.rows.get(ix)? {
                    Row::Head {
                        name,
                        count,
                        expanded,
                        favourite,
                        ..
                    } => self.head_row(ix, name.clone(), *count, *expanded, *favourite, cx),
                    Row::Album(g) => {
                        let g = *g;
                        self.album_row(ix, g, cx)
                    }
                    Row::AlbumMeta(g) => {
                        let g = *g;
                        self.album_meta_row(ix, g, cx)
                    }
                    Row::Track(t) => {
                        let selected = self.selected.contains(&t.member_id);
                        self.track_row(ix, t, selected, multi_drag.as_ref(), cx)
                    }
                })
            })
            .collect()
    }

    fn head_row(
        &self,
        ix: usize,
        name: String,
        count: u64,
        expanded: bool,
        favourite: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let chevron = if expanded {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        };
        div()
            .id(("playlist-head", ix))
            .w_full()
            .h(px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            // A header is a drop target: tracks dropped on it move there.
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    if let Some(Row::Head { id, .. }) = this.rows.get(ix) {
                        this.toggle(*id, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    cx.notify();
                }),
            )
            .child(
                svg()
                    .path(chevron)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            // The favourites playlist wears a heart so it reads as the default
            // one, not just another list named Favourites.
            .when(favourite, |d| {
                d.child(
                    svg()
                        .path(icons::HEART_FILLED)
                        .size(px(13.))
                        .flex_none()
                        .text_color(palette::accent()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(count.to_string())),
            )
            // Export this playlist to an M3U8 file. Its own mouse-down stops
            // the press from reaching the row, so a click here never toggles
            // the header open.
            .child(
                div()
                    .id(("playlist-export", ix))
                    .flex_none()
                    .p(px(3.))
                    .rounded(tokens::RADIUS)
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(Row::Head { id, name, .. }) = this.rows.get(ix) {
                                let (id, name) = (*id, name.clone());
                                this.export(id, name, cx);
                            }
                        }),
                    )
                    .child(
                        svg()
                            .path(icons::UPLOAD)
                            .size(px(14.))
                            .flex_none()
                            .text_color(palette::text_muted()),
                    ),
            )
    }

    /// An album run's name line, through the shared heading surface: Expanded
    /// opens the cover tile, Compact packs the one line.
    fn album_row(&mut self, ix: usize, g: u32, cx: &mut Context<Self>) -> Stateful<Div> {
        let headers = self.config.headers;
        track_columns::album_name_row(ix, &mut self.albums[g as usize], headers, &self.state, cx)
    }

    /// The run's meta line, the Expanded block's second row.
    fn album_meta_row(&mut self, ix: usize, g: u32, cx: &mut Context<Self>) -> Stateful<Div> {
        track_columns::album_meta_row(ix, &mut self.albums[g as usize], &self.state, cx)
    }

    fn track_row(
        &self,
        ix: usize,
        t: &TrackRow,
        selected: bool,
        multi_drag: Option<&Arc<[i64]>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (playlist_id, member_id, track_id) = (t.playlist_id, t.member_id, t.track_id);
        let playing = self.playing == Some(track_id);
        let favourite = self.favourites.contains(&track_id);
        // Dragging a row inside a multi-selection carries the whole set in view
        // order, the shared Arc `list_rows` resolved once; outside it, just this
        // row.
        let members: Arc<[i64]> = match multi_drag {
            Some(set) if selected => set.clone(),
            _ => Arc::from([member_id]),
        };
        let drag = TrackDrag {
            members,
            title: SharedString::from(t.title.clone()),
        };
        let mut row = div()
            .id(("playlist-track", ix))
            // The hover group the rating and favourite cells reveal on, the
            // library table's route.
            .group(track_cells::ROW_GROUP)
            .w_full()
            .h(px(ROW_H))
            // Indented under its header, past the chevron column.
            .pl(px(28.))
            .pr(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(playing && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_drag(drag, |drag, _pos, _window, cx| {
                cx.new(|_| TrackDragPreview {
                    title: drag.title.clone(),
                    extra: drag.members.len().saturating_sub(1),
                })
            })
            .drag_over::<TrackDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _, cx| {
                this.drop_on(drag, ix, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // Take focus so Delete on the selection reaches the panel's
                    // key handler.
                    window.focus(&this.focus);
                    if event.click_count > 1 {
                        this.play(playlist_id, Some(track_id), cx);
                    } else if event.modifiers.shift || event.modifiers.secondary() {
                        // Shift and cmd/ctrl resolve on press.
                        this.select(ix, event.modifiers, cx);
                    } else if !this.selected.contains(&member_id) {
                        // A plain press on an unselected row picks it now, so a
                        // drag from here carries it. A press on an already-lit
                        // row keeps the set for a whole-group drag; the collapse
                        // to this one row waits for the click.
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A plain click that never became a drag collapses a
                // multi-selection down to the row clicked. Modified and double
                // clicks already resolved on press.
                let mods = event.modifiers();
                if event.click_count() == 1
                    && !mods.shift
                    && !mods.secondary()
                    && this.selected.len() > 1
                    && this.selected.contains(&member_id)
                {
                    this.select(ix, Modifiers::default(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.menu_row = Some(ix);
                    // A right click outside the set reselects just that row, so
                    // the menu acts on what is lit.
                    if !this.selected.contains(&member_id) {
                        this.select(ix, Modifiers::default(), cx);
                    }
                    cx.notify();
                }),
            );
        // The cells, in registry order, only the shown ones. The shared
        // surface draws every playlist column, so there is no panel fallback.
        let cover = track_columns::cover_thumb(
            &self.state,
            (!t.path.is_empty()).then(|| std::path::Path::new(&t.path)),
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
            track_id,
            favourite,
            playing,
            plays: t.plays,
            cover,
        };
        for col in COLUMNS {
            if self.column_shown(col.key) {
                if let Some(c) = track_columns::cell(col.key, &cell, &self.state) {
                    row = row.child(c);
                }
            }
        }
        row
    }

    /// The panel menu's New Playlist entry, shared by the dropdown and the
    /// empty state.
    fn new_playlist_item(&self, menu: PopupMenu) -> PopupMenu {
        let state = self.state.clone();
        menu.item(
            PopupMenuItem::new("New Playlist...")
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    crate::playlist_create::open(state.clone(), Vec::new(), cx);
                }),
        )
    }
}

impl ColumnHost for PlaylistsPanel {
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
        self.request_layout_save(cx);
        cx.notify();
    }
}

impl HeadingHost for PlaylistsPanel {
    fn headers(&self) -> Headers {
        self.config.headers
    }

    /// Set the album heading mode and rebuild the tree, since Off, Compact,
    /// and Expanded push different rows.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.config.headers == headers {
            return;
        }
        self.config.headers = headers;
        self.request_layout_save(cx);
        self.refresh(cx);
    }
}

impl QueryFilter for PlaylistsPanel {
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
        self.refresh(cx);
    }
    fn set_query_resync(&mut self, pending: bool) {
        self.resync_box = pending;
    }
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }
}

impl PanelSettings for PlaylistsPanel {
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

    /// The panel's own View page: the column checklist and the album heading
    /// mode, the tree's view knobs, the library's own-page route rather than
    /// the shared Appearance page.
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
                Some("Which track columns show beside the title"),
                None,
                track_columns::checklist(COLUMNS, self, cx),
            ))
            .child(panel::setting_row(
                "Headings",
                Some("Break each playlist's tracks into album runs; Expanded adds the cover and stats"),
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
    /// shared query or filter by the panel's own.
    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        Some(crate::query::shared_query::search_section(
            self.config.search,
            |this: &mut Self, on, cx| this.set_search(on, cx),
            self.config.query_source,
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            cx,
        ))
    }
}

impl EventEmitter<PanelEvent> for PlaylistsPanel {}

impl Focusable for PlaylistsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for PlaylistsPanel {
    fn panel_name(&self) -> &'static str {
        "playlists"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Playlists")
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
        let menu = self.new_playlist_item(menu);

        // Display section: the view knobs under their own label, ahead of
        // the Panel section, the library's shape. The same knobs the View
        // settings page holds, one flyout each.
        let menu = menu.separator().label("Display");
        let columns = track_columns::columns_submenu(COLUMNS, window, cx);
        let menu = menu.item(PopupMenuItem::submenu("Columns", columns));
        let headings = track_columns::headings_submenu(window, cx);
        let menu = menu.item(PopupMenuItem::submenu("Headings", headings));
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

        // Panel section: rename_item opens it with its own "Panel" label.
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let menu = panel::duplicate_item(menu, &cx.entity(), self.tab_panel.clone(), |this, window, cx| {
            let (state, config) = {
                let panel = this.read(cx);
                (panel.state.clone(), panel.config.clone())
            };
            PlaylistsPanel::new(state, config, window, cx)
        });
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }

    /// The tab bar's own Import button, beside the panel menu. Import is a
    /// panel-level action, unlike per-playlist export, so it lives here where
    /// it reads clearly instead of buried in the dropdown.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![Button::new("import-playlist")
            .icon(Icon::default().path(icons::DOWNLOAD))
            .tooltip("Import Playlist")
            .on_click(cx.listener(|this, _, window, cx| {
                this.import(window, cx)
            }))])
    }
}

impl Render for PlaylistsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl PlaylistsPanel {
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
        let searching = !self.applied_query.is_empty() || !self.applied_filter.is_empty();
        let content = if self.rows.is_empty() {
            // A search that hit nothing reads differently from an empty tree.
            let message = if searching {
                "No matches"
            } else {
                "No playlists yet, add tracks or use New Playlist"
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
                    uniform_list("playlist-rows", self.rows.len(), move |range, _, cx| {
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
            this.update(cx, |this, cx| this.row_menu(menu, window, cx))
        }))
    }

    /// The right-click menu for the row under the last press: track actions
    /// for a track, play/rename/delete for a header, the panel menu when the
    /// press missed the rows.
    fn row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(ix) = self.menu_row else {
            return self.dropdown_menu(menu, window, cx);
        };
        let weak = cx.entity().downgrade();
        match self.rows.get(ix) {
            Some(Row::Track(t)) => {
                let (playlist_id, member_id, track_id) = (t.playlist_id, t.member_id, t.track_id);
                let play_panel = weak.clone();
                let menu = panel::track_actions(
                    menu,
                    self.state.clone(),
                    vec![track_id],
                    "Play",
                    window,
                    cx,
                    move |_, cx| {
                        if let Some(this) = play_panel.upgrade() {
                            this.update(cx, |this, cx| this.play(playlist_id, Some(track_id), cx));
                        }
                    },
                );
                let remove_panel = weak.clone();
                // The right press already pulled the row into the selection, so
                // Remove drops the whole lit set: one row or many.
                let remove_label = if self.selected.contains(&member_id) && self.selected.len() > 1
                {
                    format!("Remove {} from Playlist", self.selected.len())
                } else {
                    "Remove from Playlist".to_string()
                };
                let menu = menu.item(
                    PopupMenuItem::new(remove_label)
                        .icon(Icon::default().path(icons::CLOSE))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = remove_panel.upgrade() {
                                this.update(cx, |this, cx| {
                                    let members = if this.selected.contains(&member_id) {
                                        this.selected_members()
                                    } else {
                                        vec![member_id]
                                    };
                                    this.remove_members(members, cx);
                                });
                            }
                        }),
                );
                self.dropdown_menu(menu.separator(), window, cx)
            }
            Some(Row::Head {
                id,
                name,
                favourite,
                ..
            }) => {
                let (id, name, favourite) = (*id, name.clone(), *favourite);
                let play_panel = weak.clone();
                let menu = menu.item(
                    PopupMenuItem::new("Play")
                        .icon(Icon::default().path(icons::PLAY))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = play_panel.upgrade() {
                                this.update(cx, |this, cx| this.play(id, None, cx));
                            }
                        }),
                );
                // The favourites playlist is the one default: no rename, no
                // delete, so the heart column and menu always have their home.
                let rename_state = self.state.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new("Rename...")
                            .icon(Icon::default().path(icons::PENCIL))
                            .on_click(move |_, _, cx| {
                                crate::playlist_create::open_rename(
                                    rename_state.clone(),
                                    id,
                                    name.clone(),
                                    cx,
                                );
                            }),
                    )
                });
                let delete_panel = weak.clone();
                let menu = menu.when(!favourite, |menu| {
                    menu.item(
                        PopupMenuItem::new("Delete Playlist")
                            .icon(Icon::default().path(icons::TRASH))
                            .on_click(move |_, _, cx| {
                                if let Some(this) = delete_panel.upgrade() {
                                    this.update(cx, |this, cx| {
                                        this.state.library.update(cx, |library, cx| {
                                            library.delete_playlist(id, cx);
                                        });
                                    });
                                }
                            }),
                    )
                });
                self.dropdown_menu(menu.separator(), window, cx)
            }
            // A right-click never lands on a heading (they set no menu row),
            // but keep the match total: fall back to the panel menu.
            Some(Row::Album(_) | Row::AlbumMeta(_)) | None => {
                self.dropdown_menu(menu, window, cx)
            }
        }
    }
}
