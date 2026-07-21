//! The play queue panel (ADR 16): the explicit up-next queue, what Play Next
//! and Add to Queue put ahead of the playing track. The album or library you
//! started from plays on as the context and is not listed here, so the queue
//! stays what you hand-picked; a now-playing strip heads the numbered rows
//! so the panel says where the queue picks up from. Rows play now on double
//! click, drop from the right-click menu, and drag to reorder. Its own
//! panel, never a mode of the library.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, SharedString, Stateful,
    Subscription, UniformListScrollHandle, WeakEntity, Window,
};
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
use crate::settings::Settings;
use crate::track_cells;
use crate::track_columns::{self, Column, ColumnHost, GroupTrack, HeadingHost};

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
    ids: Vec<u64>,
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
    /// The display rows over `tracks`: the entries flat, or broken by album
    /// headings.
    rows: Vec<QRow>,
    /// The album runs the heading rows index, rebuilt with `rows`.
    albums: Vec<track_columns::AlbumGroup>,
    /// The favourited track ids, what each row's heart checks against.
    favourites: HashSet<i64>,
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
}

impl QueuePanel {
    pub fn new(state: AppState, config: QueueConfig, cx: &mut Context<Self>) -> Self {
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
        let mut this = QueuePanel {
            state,
            config,
            persist: Persist::Layout,
            tracks: Vec::new(),
            rows: Vec::new(),
            albums: Vec::new(),
            favourites: HashSet::new(),
            playing: None,
            rev: None,
            playing_path: None,
            selected: HashSet::new(),
            anchor: None,
            menu_row: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _library_changed,
            _thumbs_changed,
        };
        this.sync(cx);
        this
    }

    /// The widget's queue window: the modal and the popped-out queue, which
    /// have no dock layout behind them. Reads its view from settings and
    /// writes edits back there, so its columns and headings survive a close
    /// and a relaunch the way a docked panel's ride the layout dump.
    pub fn windowed(state: AppState, cx: &mut Context<Self>) -> Self {
        let config = Settings::load()
            .queue_view
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let mut this = Self::new(state, config, cx);
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
        // Total play counts for the queue's tracks and the playing one, one
        // projection pass, for the plays column.
        let plays = {
            let mut ids: Vec<i64> = queued.iter().filter_map(|e| library.id_for(&e.path)).collect();
            if let Some(id) = self.playing_path.as_ref().and_then(|p| library.id_for(p)) {
                ids.push(id);
            }
            library.plays_for(&ids)
        };
        self.playing = self.playing_path.as_ref().map(|path| {
            let track_id = library.id_for(path);
            let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
            match library.meta_for(path) {
                Some(m) => Playing {
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
            }
        });
        self.favourites = library.favourite_ids();
        self.tracks = queued
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let track_id = library.id_for(&entry.path);
                let pos = (i + 1) as u32;
                let count = track_id.and_then(|id| plays.get(&id).copied()).unwrap_or(0);
                match library.meta_for(&entry.path) {
                    Some(m) => TrackRow {
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
        self.rebuild_rows();
        cx.notify();
    }

    /// Lay the display rows over `tracks`: flat, or broken into album runs
    /// with a heading over each. A headings or column flip that leaves the
    /// queue alone calls this, not `sync`.
    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut albums = Vec::new();
        if self.config.headers == Headers::Off {
            rows.extend((0..self.tracks.len() as u32).map(QRow::Track));
            self.rows = rows;
            self.albums = albums;
            return;
        }
        let mut i = 0;
        while i < self.tracks.len() {
            let mut j = i + 1;
            while j < self.tracks.len() && self.tracks[j].album == self.tracks[i].album {
                j += 1;
            }
            let group: Vec<GroupTrack> = self.tracks[i..j].iter().map(group_track).collect();
            albums.push(track_columns::album_group(&group));
            let g = (albums.len() - 1) as u32;
            rows.push(QRow::Album(g));
            if self.config.headers == Headers::Expanded {
                rows.push(QRow::AlbumMeta(g));
            }
            rows.extend((i..j).map(|ti| QRow::Track(ti as u32)));
            i = j;
        }
        self.rows = rows;
        self.albums = albums;
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
            self.selected = self.rows[lo..=hi]
                .iter()
                .filter_map(|row| match row {
                    QRow::Track(ti) => self.tracks.get(*ti as usize).map(|t| t.entry_id),
                    _ => None,
                })
                .collect();
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
        self.publish_selection(cx);
        cx.notify();
    }

    /// Ctrl+A: take every queue entry. Anchors at the first so a follow-up
    /// shift-click narrows from the top.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.tracks.is_empty() {
            return;
        }
        self.selected = self.tracks.iter().map(|t| t.entry_id).collect();
        self.anchor = self.tracks.first().map(|t| t.entry_id);
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
        let player = self.state.player.read(cx);
        for &id in ids {
            player.remove_from_queue(id);
        }
        let landing = self.tracks.iter().position(|t| ids.contains(&t.entry_id));
        self.tracks.retain(|t| !ids.contains(&t.entry_id));
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
    }

    /// The visible slice of the list: album headings and queue entries, drawn
    /// through the shared column surface.
    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
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
                        self.track_row(ix, ti, cx)
                    }
                })
            })
            .collect()
    }

    /// One queue entry row: reorder-drag, multi-select, and remove keyed on
    /// the entry id, its cells the shown columns. A queued file the library
    /// does not know shows no rating or favourite.
    fn track_row(&self, ix: usize, ti: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = &self.tracks[ti];
        let entry = t.entry_id;
        let has_track = t.track_id.is_some();
        let favourite = t
            .track_id
            .map(|id| self.favourites.contains(&id))
            .unwrap_or(false);
        let selected = self.selected.contains(&entry);
        // Dragging a row inside a multi-selection carries the whole set in
        // queue order; outside it, just this entry.
        let ids = if self.selected.len() > 1 && selected {
            self.selected_ids()
        } else {
            vec![entry]
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
            .on_drop(cx.listener(move |this, drag: &QueueDrag, _, cx| {
                this.reorder(&drag.ids, ix, cx);
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

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
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
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| QueuePanel::new(state, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for QueuePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl QueuePanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
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
        let content = if self.tracks.is_empty() {
            div().flex_1().min_h_0().flex().flex_col().child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child("Queue is empty"),
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
