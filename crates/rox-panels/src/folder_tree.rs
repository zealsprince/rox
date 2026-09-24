//! The folder tree panel: the library's folder hierarchy as an
//! expand/collapse tree, rebuilt from the projection's interned folder
//! strings, never a filesystem scan. The shared prefix above the music
//! collapses away, so the top nodes are where the library starts. CUE images
//! stay one physical-file row with their subsongs nested underneath. The
//! active query narrows the tree, and the right-click menu adds a
//! folder-scope filter that narrows the shared query to a whole subtree.

use std::collections::{HashMap, HashSet};
use std::path::{MAIN_SEPARATOR, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, ScrollStrategy, ScrollWheelEvent, SharedString, Stateful,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, svg,
    uniform_list,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Side};
use rox_core::QUEUE_CAP;
use rox_core::fmt::fmt_ms;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::folders::{Node, build_roots, node_at, sum_counts};
use rox_library::projection::FilterField;
use rox_library::sort::natural_cmp;
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ResumeIdle};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::track_ui::track_columns;
use crate::track_ui::track_drag::{PlayDrag, PlayDragPreview};

/// Matches the filter panel's row height.
const ROW_H: f32 = 26.;

const INDENT: f32 = 14.;

const DIM: f32 = 0.4;

/// Where the tree shows cover art in place of the row icon.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverArt {
    #[default]
    None,
    Folders,
    Songs,
    Both,
}

impl CoverArt {
    fn on_folders(self) -> bool {
        matches!(self, CoverArt::Folders | CoverArt::Both)
    }

    fn on_songs(self) -> bool {
        matches!(self, CoverArt::Songs | CoverArt::Both)
    }
}

/// What the tree does with rows the active query leaves out: dim them in
/// place or drop them. Folders and songs each pick their own.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterEffect {
    #[default]
    Dim,
    Hide,
}

/// The folder tree panel's per-view config. The folder scope is app state
/// and isn't saved here.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderTreeConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub cover: CoverArt,
    /// What happens to folders with no match under the active query.
    pub folders: FilterEffect,
    /// What happens to non-matching songs inside a folder that's shown.
    pub songs: FilterEffect,
    pub follow_playing: bool,
    /// Scroll back to the playing track after browsing stops.
    pub resume_playing: bool,
    pub smooth_follow: bool,
    /// Whether the search box shows; the query only filters while it does.
    pub search: bool,
    pub query_source: QuerySource,
    /// The panel's own query, kept while following the shared one.
    pub query: String,
    /// Folders left open when the layout was saved. Paths a rescan no longer
    /// knows sit inert.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded: Vec<String>,
    /// CUE images left open, by full path so equal filenames stay distinct.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_cues: Vec<PathBuf>,
}

impl Default for FolderTreeConfig {
    fn default() -> Self {
        FolderTreeConfig {
            chrome: PanelChrome::default(),
            cover: CoverArt::default(),
            // Hide folders that miss so a search narrows the tree; dim stray songs so
            // a folder still shows its whole contents.
            folders: FilterEffect::Hide,
            songs: FilterEffect::Dim,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
            expanded: Vec::new(),
            expanded_cues: Vec::new(),
        }
    }
}

/// A directory, a CUE image, or a playable logical track.
#[derive(Clone)]
enum RowKind {
    /// A physical image claimed by a CUE sheet; `ids` are its logical tracks.
    Cue {
        /// Full backing-file path, also the expansion-state key.
        path: PathBuf,
        expanded: bool,
        /// Drawn faint only when every currently listed child is dimmed.
        dimmed: bool,
        ids: Vec<i64>,
    },
    Folder {
        path: String,
        count: u32,
        has_children: bool,
        expanded: bool,
        dimmed: bool,
    },
    Track {
        row: u32,
        id: i64,
        /// The owning folder and this song's index in its `folder_tracks` list.
        folder: String,
        pos: usize,
        dimmed: bool,
    },
}

#[derive(Clone)]
struct Row {
    label: SharedString,
    depth: usize,
    kind: RowKind,
}

/// A projection row with its track key resolved once per flatten.
struct SongRow {
    row: u32,
    id: i64,
    title: SharedString,
    key: Option<TrackKey>,
}

/// One file entry in a folder: a plain file with one song, or a CUE image
/// with every subsong sharing its full path.
struct SongGroup {
    label: SharedString,
    cue: Option<PathBuf>,
    songs: Vec<SongRow>,
}

/// Group rows into physical files, CUE tracks by full path whatever the
/// scan order. Groups sort naturally by filename, subsongs by `TrackKey.sub`.
fn group_songs(songs: Vec<SongRow>) -> Vec<SongGroup> {
    let mut groups: Vec<SongGroup> = Vec::new();
    let mut images: HashMap<PathBuf, usize> = HashMap::new();
    for song in songs {
        let label = song
            .key
            .as_ref()
            .and_then(|key| key.path.file_name())
            .map(|name| SharedString::from(name.to_string_lossy().into_owned()))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| song.title.clone());
        let cue = song
            .key
            .as_ref()
            .filter(|key| key.sub > 0)
            .map(|key| key.path.clone());
        if let Some(path) = &cue {
            if let Some(&ix) = images.get(path) {
                groups[ix].songs.push(song);
                continue;
            }
            images.insert(path.clone(), groups.len());
        }
        groups.push(SongGroup {
            label,
            cue,
            songs: vec![song],
        });
    }
    for group in &mut groups {
        group
            .songs
            .sort_by_key(|song| song.key.as_ref().map_or(0, |key| key.sub));
    }
    // Cache the folded filename once; stable ties retain the ordinary-file order.
    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|group| (group.label.to_lowercase(), group))
        .collect();
    groups.sort_by(|(a, _), (b, _)| natural_cmp(a, b));
    groups.into_iter().map(|(_, group)| group).collect()
}

/// Open the CUE image that owns `key`, if `key` is a subsong.
fn reveal_cue(expanded: &mut HashSet<PathBuf>, key: &TrackKey) {
    if key.sub > 0 {
        expanded.insert(key.path.clone());
    }
}

/// Append one folder's file entries to the visible rows. `pos` counts
/// children of collapsed images too: playback indexes `folder_tracks`, not
/// visible rows, so folding must never shift where the next file starts.
fn append_songs(
    out: &mut Vec<Row>,
    groups: &[SongGroup],
    folder: &str,
    depth: usize,
    expanded_cues: &HashSet<PathBuf>,
    dimmed: &HashSet<u32>,
) {
    let mut pos = 0;
    for group in groups {
        if let Some(path) = &group.cue {
            let expanded = expanded_cues.contains(path);
            out.push(Row {
                label: group.label.clone(),
                depth,
                kind: RowKind::Cue {
                    path: path.clone(),
                    expanded,
                    dimmed: group.songs.iter().all(|song| dimmed.contains(&song.row)),
                    ids: group.songs.iter().map(|song| song.id).collect(),
                },
            });
            if !expanded {
                pos += group.songs.len();
                continue;
            }
        }
        for song in &group.songs {
            let label = if group.cue.is_some() {
                format!(
                    "{:02}. {}",
                    song.key.as_ref().map_or(0, |key| key.sub),
                    song.title
                )
                .into()
            } else {
                group.label.clone()
            };
            out.push(Row {
                label,
                depth: depth + usize::from(group.cue.is_some()),
                kind: RowKind::Track {
                    row: song.row,
                    id: song.id,
                    folder: folder.to_owned(),
                    pos,
                    dimmed: dimmed.contains(&song.row),
                },
            });
            pos += 1;
        }
    }
}

pub struct FolderTreePanel {
    state: AppState,
    config: FolderTreeConfig,
    focus: FocusHandle,
    search: Entity<SearchBox>,
    /// A pending box reset, consumed in render where a window exists.
    resync_box: bool,
    selection_ids: Vec<i64>,
    /// Top-level folders after the shared prefix collapses.
    roots: Vec<Node>,
    /// Each folder's own listed songs by path, in play order.
    folder_tracks: HashMap<String, Vec<u32>>,
    /// Listed songs outside the active filter, drawn faint in Dim mode.
    dimmed_songs: HashSet<u32>,
    visible: Vec<Row>,
    /// Kept across rescans; top-level nodes seed in expanded once.
    expanded: HashSet<String>,
    expanded_cues: HashSet<PathBuf>,
    seeded: bool,
    scroll: UniformListScrollHandle,
    /// The keyboard-and-click cursor, an index into `visible`.
    cursor: Option<usize>,
    /// Selected songs by library id. Folders aren't selectable.
    selected: HashSet<i64>,
    /// The shift-range anchor, a `visible` index.
    anchor: Option<usize>,
    /// The row under the last right press, what the context menu acts on.
    menu_row: Option<usize>,
    /// The playing track's key and library id; the highlight matches on the id.
    playing_key: Option<TrackKey>,
    playing: Option<i64>,
    /// Track keys by id, so a hover frame never repeats the store lookup.
    drag_keys: HashMap<i64, Option<TrackKey>>,
    /// Bumped when the selection or visible order changes. Keys the drag-set
    /// cache, so a big selection shares one Arc across its rows.
    drag_gen: u64,
    drag_set: Option<DragSet>,
    /// The hovered folder row and its drag payload. Only that row carries a
    /// drag, so the subtree walk runs on hover, not per folder row per frame.
    hover_folder: Option<String>,
    folder_drag: Option<FolderDrag>,
    resume_idle: ResumeIdle,
    glide_to: Option<usize>,
    glide_tick: Instant,
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
    _player_changed: Subscription,
    _search_events: Subscription,
    _selection_changed: Subscription,
    /// Drops the phrase on blur, so tab goes back to walking panels.
    _type_ahead_blur: Subscription,
}

/// A cached multi-selection drag: generation, keys to play, catalog ids.
type DragSet = (u64, Arc<[TrackKey]>, Arc<[i64]>);

/// The hovered folder's cached drag payload. `drag` is None for an empty
/// subtree, so a dead branch isn't re-walked every frame either.
struct FolderDrag {
    path: String,
    generation: u64,
    drag: Option<PlayDrag>,
}

impl FolderTreePanel {
    pub fn new(
        state: AppState,
        config: FolderTreeConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A rescan rebuilds the structure. Counts follow the shared query, our own
        // scope writes included; recount is idempotent, so the echo settles.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.rebuild(cx);
                }
            },
        );
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.on_shared_query_changed(cx),
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Seed from the current pick so a restored selection-follower isn't blank.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        // A restored layout's open folders skip the root seeding.
        let expanded: HashSet<String> = config.expanded.iter().cloned().collect();
        let seeded = !expanded.is_empty();
        let expanded_cues = config.expanded_cues.iter().cloned().collect();
        let focus = cx.focus_handle().tab_stop(true);
        let panel = cx.weak_entity();
        let _type_ahead_blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
            panel
                .update(cx, |this: &mut FolderTreePanel, cx| {
                    this.clear_type_ahead(cx);
                })
                .ok();
        });
        let mut this = FolderTreePanel {
            state,
            config,
            focus,
            search,
            resync_box: false,
            selection_ids,
            roots: Vec::new(),
            folder_tracks: HashMap::new(),
            dimmed_songs: HashSet::new(),
            visible: Vec::new(),
            expanded,
            expanded_cues,
            seeded,
            scroll: UniformListScrollHandle::new(),
            cursor: None,
            selected: HashSet::new(),
            anchor: None,
            menu_row: None,
            playing_key: None,
            playing: None,
            drag_keys: HashMap::new(),
            drag_gen: 0,
            drag_set: None,
            hover_folder: None,
            folder_drag: None,
            resume_idle: ResumeIdle::default(),
            glide_to: None,
            glide_tick: Instant::now(),
            type_ahead: String::new(),
            type_ahead_at: None,
            tab_panel: None,
            _library_changed,
            _query_changed,
            _player_changed,
            _search_events,
            _selection_changed,
            _type_ahead_blur,
        };
        this.rebuild(cx);
        // A duplicate opens mid-track; pick the playing one up now.
        this.sync_playing(cx);
        this
    }

    /// Rebuild the hierarchy from the projection's folder set, then count. The
    /// structure never follows the query, so typing only hides empty branches.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.roots = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => build_roots(&projection.folders.strings),
                None => Vec::new(),
            }
        };
        if !self.seeded && !self.roots.is_empty() {
            self.expanded = self.roots.iter().map(|r| r.path.clone()).collect();
            self.seeded = true;
        }
        self.drag_keys.clear();
        self.recount(cx);
    }

    /// Regroup the songs per folder, recount every subtree against the active
    /// query, and reflatten. [`FilterEffect`] decides what happens to misses.
    fn recount(&mut self, cx: &mut Context<Self>) {
        {
            let song_hide = self.config.songs == FilterEffect::Hide;
            let (text, facet) = (self.effective_query(cx), self.effective_filter(cx));
            let library = self.state.library.read(cx);
            self.folder_tracks.clear();
            self.dimmed_songs.clear();
            if let Some(projection) = library.projection() {
                let len = projection.len();
                // None on either mask constrains nothing; a song matches when it passes both.
                let text_hits: Option<Vec<bool>> = (!text.is_empty()).then(|| {
                    let mut hits = vec![false; len];
                    for row in projection.search(&text) {
                        hits[row as usize] = true;
                    }
                    hits
                });
                let facet_mask = projection.filter_mask(&facet);
                let matches = |row: usize| {
                    text_hits.as_ref().is_none_or(|h| h[row])
                        && facet_mask.as_ref().is_none_or(|m| m[row])
                };
                let nsym = projection.folders.strings.len();
                let mut total_sym = vec![0u32; nsym];
                let mut matched_sym = vec![0u32; nsym];
                // All songs listed in Dim, only matches in Hide; kept misses go faint.
                let mut listed: Vec<Vec<u32>> = vec![Vec::new(); nsym];
                for row in 0..len {
                    // Skips tombstoned rows and radio stations alike.
                    if !projection.is_browsable(row as u32) {
                        continue;
                    }
                    let sym = projection.folder[row] as usize;
                    total_sym[sym] += 1;
                    let hit = matches(row);
                    if hit {
                        matched_sym[sym] += 1;
                    }
                    if hit || !song_hide {
                        listed[sym].push(row as u32);
                        if !hit {
                            self.dimmed_songs.insert(row as u32);
                        }
                    }
                }
                let mut counts: HashMap<&str, (u32, u32)> = HashMap::with_capacity(nsym);
                for (sym, list) in listed.into_iter().enumerate() {
                    // Bare-filename tracks intern to the empty folder and never get a node.
                    if total_sym[sym] == 0 || projection.folders.strings[sym].is_empty() {
                        continue;
                    }
                    let path = &projection.folders.strings[sym];
                    counts.insert(path, (total_sym[sym], matched_sym[sym]));
                    if !list.is_empty() {
                        self.folder_tracks.insert(path.clone(), list);
                    }
                }
                for root in &mut self.roots {
                    sum_counts(root, &counts);
                }
            }
        }
        self.flatten(cx);
    }

    /// Reflatten the visible rows: subfolders first, then files, CUE subsongs
    /// nested under their image. Empty branches stay out.
    fn flatten(&mut self, cx: &mut Context<Self>) {
        struct Walk<'a> {
            expanded: &'a HashSet<String>,
            folder_tracks: &'a HashMap<String, Vec<u32>>,
            dimmed_songs: &'a HashSet<u32>,
            folder_hide: bool,
            /// Built before the walk so it never resolves store keys.
            groups: HashMap<String, Vec<SongGroup>>,
            expanded_cues: &'a HashSet<PathBuf>,
            out: Vec<Row>,
        }
        impl Walk<'_> {
            fn folder(&mut self, node: &Node, depth: usize) {
                if node.total == 0 {
                    return;
                }
                let unmatched = node.matched == 0;
                if unmatched && self.folder_hide {
                    return;
                }
                let open = self.expanded.contains(&node.path);
                let tracks = self.folder_tracks.get(&node.path);
                self.out.push(Row {
                    label: node.label.clone().into(),
                    depth,
                    kind: RowKind::Folder {
                        path: node.path.clone(),
                        // The badge counts matches, so it lines up with what a search leaves lit.
                        count: node.matched,
                        has_children: !node.children.is_empty() || tracks.is_some(),
                        expanded: open,
                        dimmed: unmatched,
                    },
                });
                if !open {
                    return;
                }
                for child in &node.children {
                    self.folder(child, depth + 1);
                }
                if let Some(groups) = self.groups.get(&node.path) {
                    append_songs(
                        &mut self.out,
                        groups,
                        &node.path,
                        depth + 1,
                        self.expanded_cues,
                        self.dimmed_songs,
                    );
                }
            }
        }
        // Collected first: `key_for` below needs `&mut self`.
        let songs: Vec<(String, u32, i64, SharedString)> = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => self
                    .folder_tracks
                    .iter()
                    .filter(|(path, _)| self.expanded.contains(*path))
                    .flat_map(|(folder, rows)| rows.iter().map(move |&row| (folder.clone(), row)))
                    .map(|(folder, row)| {
                        (
                            folder,
                            row,
                            projection.db_id[row as usize],
                            SharedString::from(projection.title.get(row as usize).to_string()),
                        )
                    })
                    .collect(),
                None => Vec::new(),
            }
        };
        let mut by_folder: HashMap<String, Vec<SongRow>> = HashMap::new();
        for (folder, row, id, title) in songs {
            let key = self.key_for(id, cx);
            by_folder.entry(folder).or_default().push(SongRow {
                row,
                id,
                title,
                key,
            });
        }
        let groups: HashMap<_, _> = by_folder
            .into_iter()
            .map(|(folder, songs)| (folder, group_songs(songs)))
            .collect();
        // Play order follows the grouped order, so `pos` lines up, collapsed
        // images' children included.
        for (folder, groups) in &groups {
            self.folder_tracks.insert(
                folder.clone(),
                groups
                    .iter()
                    .flat_map(|group| group.songs.iter().map(|song| song.row))
                    .collect(),
            );
        }
        let mut walk = Walk {
            expanded: &self.expanded,
            folder_tracks: &self.folder_tracks,
            dimmed_songs: &self.dimmed_songs,
            folder_hide: self.config.folders == FilterEffect::Hide,
            groups,
            expanded_cues: &self.expanded_cues,
            out: Vec::new(),
        };
        for root in &self.roots {
            walk.folder(root, 0);
        }
        self.visible = walk.out;
        // Visible order drives drag order, so a reflow drops the cached drag set.
        self.drag_gen += 1;
        // Drop indices now off the end; the selection keys on ids and survives.
        if self.cursor.is_some_and(|ix| ix >= self.visible.len()) {
            self.cursor = None;
        }
        if self.menu_row.is_some_and(|ix| ix >= self.visible.len()) {
            self.menu_row = None;
        }
        if self.anchor.is_some_and(|ix| ix >= self.visible.len()) {
            self.anchor = None;
        }
        cx.notify();
    }

    /// On a track change, resolve the playing track to its id for the highlight.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let key = self.state.player.read(cx).now_playing().map(|now| now.key);
        if key == self.playing_key {
            return;
        }
        self.playing_key = key;
        self.playing = self
            .playing_key
            .as_ref()
            .and_then(|key| self.state.library.read(cx).id_for_key(key));
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Open every branch from a root down to `path`, the same prefix descent
    /// as [`node_at`].
    fn expand_to(&mut self, path: &str) {
        let mut chain = Vec::new();
        let mut nodes = self.roots.as_slice();
        while let Some(node) = nodes.iter().find(|node| {
            node.path == path
                || (path.starts_with(node.path.as_str())
                    && path[node.path.len()..].starts_with(MAIN_SEPARATOR))
        }) {
            chain.push(node.path.clone());
            if node.path == path {
                break;
            }
            nodes = node.children.as_slice();
        }
        self.expanded.extend(chain);
    }

    /// Open the branches to the playing track, reflatten, and return its row.
    /// None when nothing plays or Hide keeps it off the tree.
    fn reveal_playing(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        let id = self.playing?;
        let folder = {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            self.folder_tracks
                .iter()
                .find(|(_, rows)| rows.iter().any(|&row| projection.db_id[row as usize] == id))
                .map(|(folder, _)| folder.clone())
        }?;
        self.expand_to(&folder);
        if let Some(key) = self.key_for(id, cx) {
            reveal_cue(&mut self.expanded_cues, &key);
        }
        self.flatten(cx);
        self.visible
            .iter()
            .position(|row| matches!(row.kind, RowKind::Track { id: rid, .. } if rid == id))
    }

    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.reveal_playing(cx) {
            self.set_cursor(ix, cx);
        }
    }

    /// Scroll to the playing track, gliding when smooth is on. Never moves the
    /// cursor; that's the menu jump's job.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.reveal_playing(cx) else {
            return;
        };
        if self.config.smooth_follow {
            self.glide_to = Some(ix);
        } else {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Restart the idle clock that scrolls back to the playing track.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.follow_playing(cx);
        }
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
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.config.follow_playing = !self.config.follow_playing;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    fn toggle_expand(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.visible.get(ix).map(|row| &row.kind) {
            Some(RowKind::Folder { path, .. }) => {
                if !self.expanded.remove(path) {
                    self.expanded.insert(path.clone());
                }
            }
            Some(RowKind::Cue { path, .. }) => {
                if !self.expanded_cues.remove(path) {
                    self.expanded_cues.insert(path.clone());
                }
            }
            _ => return,
        }
        self.flatten(cx);
    }

    /// Fold a folder and its whole subtree: open when the folder is shut, shut
    /// everything below otherwise.
    fn toggle_expand_deep(&mut self, path: &str, cx: &mut Context<Self>) {
        fn collect(node: &Node, out: &mut Vec<String>) {
            out.push(node.path.clone());
            for child in &node.children {
                collect(child, out);
            }
        }
        let Some(node) = node_at(&self.roots, path) else {
            return;
        };
        let mut paths = Vec::new();
        collect(node, &mut paths);
        if self.expanded.contains(path) {
            for path in &paths {
                self.expanded.remove(path);
            }
        } else {
            self.expanded.extend(paths);
        }
        self.glide_to = None;
        self.flatten(cx);
    }

    /// Fold everything shut. The glide stops too, since its target index moved.
    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if self.expanded.is_empty() && self.expanded_cues.is_empty() {
            return;
        }
        self.expanded.clear();
        self.expanded_cues.clear();
        self.glide_to = None;
        self.flatten(cx);
    }

    /// Scope the shared folder filter to one folder's subtree, or clear it if
    /// already scoped. The filter matches by prefix, so one pick covers the branch.
    fn toggle_scope(&mut self, path: String, cx: &mut Context<Self>) {
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            let scoped = filter.values(FilterField::Folder) == [path.clone()];
            filter.clear(FilterField::Folder);
            if !scoped {
                filter.toggle(FilterField::Folder, &path);
            }
            query.set_filter(filter, cx);
        });
        // While following its own query the panel ignores the shared-query echo,
        // so repaint here for the scope highlight.
        cx.notify();
    }

    fn clear_scope(&mut self, cx: &mut Context<Self>) {
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            if filter.values(FilterField::Folder).is_empty() {
                return;
            }
            filter.clear(FilterField::Folder);
            query.set_filter(filter, cx);
        });
        cx.notify();
    }

    /// A folder's whole subtree as projection rows in tree order: subfolders
    /// first, then its own songs.
    fn subtree_rows(&self, path: &str) -> Vec<u32> {
        fn collect(node: &Node, folder_tracks: &HashMap<String, Vec<u32>>, out: &mut Vec<u32>) {
            for child in &node.children {
                collect(child, folder_tracks, out);
            }
            if let Some(rows) = folder_tracks.get(&node.path) {
                out.extend_from_slice(rows);
            }
        }
        let mut out = Vec::new();
        if let Some(node) = node_at(&self.roots, path) {
            collect(node, &self.folder_tracks, &mut out);
        }
        out
    }

    /// The row whose cover stands for a folder: its first song, or the first
    /// in its subtree.
    fn folder_cover_row(&self, path: &str) -> Option<u32> {
        fn first(node: &Node, folder_tracks: &HashMap<String, Vec<u32>>) -> Option<u32> {
            if let Some(&row) = folder_tracks.get(&node.path).and_then(|rows| rows.first()) {
                return Some(row);
            }
            node.children
                .iter()
                .find_map(|child| first(child, folder_tracks))
        }
        node_at(&self.roots, path).and_then(|node| first(node, &self.folder_tracks))
    }

    /// A track id's key, resolved once and shared by drags and covers.
    fn key_for(&mut self, id: i64, cx: &App) -> Option<TrackKey> {
        match self.drag_keys.get(&id) {
            Some(key) => key.clone(),
            None => {
                let key = self
                    .state
                    .library
                    .read(cx)
                    .keys_for(&[id])
                    .ok()
                    .and_then(|mut keys| keys.pop());
                self.drag_keys.insert(id, key.clone());
                key
            }
        }
    }

    /// A row's cover thumbnail. None when the file has no path yet.
    fn cover_for(&mut self, row: u32, cx: &mut Context<Self>) -> Option<crate::thumbs::Thumb> {
        let id = self
            .state
            .library
            .read(cx)
            .projection()
            .map(|p| p.db_id[row as usize])?;
        let key = self.key_for(id, cx)?;
        track_columns::cover_thumb(&self.state, Some(key.path.as_path()), true, cx)
    }

    /// Queue projection rows with the cursor at `start`, capped at [`QUEUE_CAP`].
    fn play_rows(&mut self, rows: &[u32], start: usize, cx: &mut Context<Self>) {
        // Keep the clicked row inside the capped window.
        let lo = start
            .saturating_sub(QUEUE_CAP / 2)
            .min(rows.len().saturating_sub(QUEUE_CAP));
        let hi = (lo + QUEUE_CAP).min(rows.len());
        let rows = &rows[lo..hi];
        let keys = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = rows.iter().map(|&r| projection.db_id[r as usize]).collect();
            let Ok(keys) = library.keys_for(&ids) else {
                return;
            };
            keys
        };
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(keys, start - lo, cx));
    }

    /// Queue library ids from the front, in the caller's order.
    fn play_ids(&mut self, ids: &[i64], cx: &mut Context<Self>) {
        let capped = &ids[..ids.len().min(QUEUE_CAP)];
        let Ok(keys) = self.state.library.read(cx).keys_for(capped) else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(keys, 0, cx));
    }

    fn play_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.subtree_rows(path);
        self.play_rows(&rows, 0, cx);
    }

    fn play_track(&mut self, folder: &str, pos: usize, cx: &mut Context<Self>) {
        let Some(rows) = self.folder_tracks.get(folder).cloned() else {
            return;
        };
        if pos >= rows.len() {
            return;
        }
        self.play_rows(&rows, pos, cx);
    }

    /// A song row's drag payload: the whole selection in view order when the
    /// row is part of it, otherwise just this row.
    fn song_drag(&mut self, ix: usize, title: &SharedString, cx: &App) -> Option<PlayDrag> {
        let id = self.song_id_at(ix)?;
        // Built once per selection or reflow and shared behind an Arc, so each row
        // costs a refcount bump.
        let (keys, ids): (Arc<[TrackKey]>, Arc<[i64]>) = if self.selected.len() > 1
            && self.selected.contains(&id)
        {
            if self.drag_set.as_ref().map(|(generation, ..)| *generation) != Some(self.drag_gen) {
                let ids: Vec<i64> = self
                    .visible
                    .iter()
                    .filter_map(|row| match &row.kind {
                        RowKind::Track { id, .. } if self.selected.contains(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                let set: Arc<[TrackKey]> =
                    ids.iter().filter_map(|&id| self.key_for(id, cx)).collect();
                self.drag_set = Some((self.drag_gen, set, ids.into()));
            }
            self.drag_set
                .as_ref()
                .map(|(_, set, ids)| (set.clone(), ids.clone()))?
        } else {
            (self.key_for(id, cx).into_iter().collect(), vec![id].into())
        };
        if keys.is_empty() {
            return None;
        }
        Some(PlayDrag {
            keys,
            ids,
            title: title.clone(),
        })
    }

    /// A folder row's drag payload: its subtree in tree order, capped like a
    /// folder play.
    ///
    /// gpui takes a drag value eagerly at render, once per row per frame, and a
    /// root folder's subtree can run to tens of thousands of rows. So only the
    /// hovered row builds one, cached until the hover moves or the tree reflows.
    fn folder_drag(&mut self, path: &str, title: &SharedString, cx: &App) -> Option<PlayDrag> {
        if self.hover_folder.as_deref() != Some(path) {
            return None;
        }

        let fresh = self
            .folder_drag
            .as_ref()
            .is_some_and(|cached| cached.path == path && cached.generation == self.drag_gen);
        if !fresh {
            let rows = self.subtree_rows(path);
            let drag = self.build_folder_drag(&rows, title, cx);
            self.folder_drag = Some(FolderDrag {
                path: path.to_string(),
                generation: self.drag_gen,
                drag,
            });
        }

        self.folder_drag
            .as_ref()
            .and_then(|cached| cached.drag.clone())
    }

    /// None when no key resolves, so a dead folder offers no drag.
    fn build_folder_drag(&self, rows: &[u32], title: &SharedString, cx: &App) -> Option<PlayDrag> {
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        let ids: Vec<i64> = rows
            .iter()
            .take(QUEUE_CAP)
            .map(|&row| projection.db_id[row as usize])
            .collect();
        let keys: Arc<[TrackKey]> = library.keys_for(&ids).ok()?.into();

        if keys.is_empty() {
            return None;
        }

        Some(PlayDrag {
            keys,
            ids: ids.into(),
            title: title.clone(),
        })
    }

    /// Keyboard browsing: arrows move and fold, enter folds or plays, and
    /// plain typing jumps by prefix.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        // Before the modifier guard, which rejects the rest of the chorded keys.
        if keystroke.modifiers.secondary() && keystroke.key == "a" {
            self.select_all(cx);
            return;
        }
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        self.touch_resume(cx);
        match keystroke.key.as_str() {
            // Escape drops a phrase, handing tab back to panel traversal.
            "escape" => {
                self.clear_type_ahead(cx);
            }
            "up" => self.move_cursor(-1, cx),
            "down" => self.move_cursor(1, cx),
            "home" => self.set_cursor(0, cx),
            "end" => {
                let last = self.visible.len().saturating_sub(1);
                self.set_cursor(last, cx);
            }
            "left" => {
                let Some(ix) = self.cursor else { return };
                if let Some(Row {
                    kind: RowKind::Cue { expanded, .. },
                    ..
                }) = self.visible.get(ix)
                {
                    if *expanded {
                        self.toggle_expand(ix, cx);
                    }
                    return;
                }
                let Some(Row {
                    kind: RowKind::Folder { path, expanded, .. },
                    ..
                }) = self.visible.get(ix)
                else {
                    return;
                };
                if keystroke.modifiers.shift {
                    if *expanded {
                        let path = path.clone();
                        self.toggle_expand_deep(&path, cx);
                    }
                } else if *expanded {
                    self.toggle_expand(ix, cx);
                }
            }
            "right" => {
                let Some(ix) = self.cursor else { return };
                if let Some(Row {
                    kind: RowKind::Cue { expanded, .. },
                    ..
                }) = self.visible.get(ix)
                {
                    if !*expanded {
                        self.toggle_expand(ix, cx);
                    }
                    return;
                }
                let Some(Row {
                    kind: RowKind::Folder { path, expanded, .. },
                    ..
                }) = self.visible.get(ix)
                else {
                    return;
                };
                if keystroke.modifiers.shift {
                    if !*expanded {
                        let path = path.clone();
                        self.toggle_expand_deep(&path, cx);
                    }
                } else if !*expanded {
                    self.toggle_expand(ix, cx);
                }
            }
            "enter" => {
                let Some(ix) = self.cursor else { return };
                match self.visible.get(ix) {
                    Some(Row {
                        kind: RowKind::Folder { .. } | RowKind::Cue { .. },
                        ..
                    }) => self.toggle_expand(ix, cx),
                    Some(Row {
                        kind: RowKind::Track { folder, pos, .. },
                        ..
                    }) => {
                        let (folder, pos) = (folder.clone(), *pos);
                        self.play_track(&folder, pos, cx);
                    }
                    None => {}
                }
            }
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                if text == " " && !panel::type_ahead_live(self.type_ahead_at) {
                    return;
                }
                // Stop it here, or it also fires the workspace's unscoped space-bound
                // TogglePlayback.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead phrase and jump to its next match. A
    /// grown phrase re-tests the cursor's row so refining a match stays put.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // A miss still updated the badge, so repaint either way.
        panel::type_ahead_fade(cx);
        cx.notify();
        let needle = self.type_ahead.to_lowercase();
        let start = match self.cursor {
            Some(ix) if grown => ix,
            Some(ix) => ix + 1,
            None => 0,
        };
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let hit = (0..len)
            .map(|off| (start + off) % len)
            .find(|&ix| panel::type_ahead_hit(&self.visible[ix].label.to_lowercase(), &needle));
        if let Some(ix) = hit {
            self.set_cursor(ix, cx);
        }
    }

    /// Drop the phrase. True when there was one, for the escape ladder.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Step to the phrase's next match, Tab's cycle. Leaves the window stamp
    /// alone, so a run of tabs doesn't revive the badge.
    fn type_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.type_ahead.is_empty() {
            return;
        }
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        cx.notify();
        let needle = self.type_ahead.to_lowercase();
        let hit = panel::type_ahead_scan(len, self.cursor, back)
            .find(|&ix| panel::type_ahead_hit(&self.visible[ix].label.to_lowercase(), &needle));
        if let Some(ix) = hit {
            self.set_cursor(ix, cx);
        }
    }

    /// Step the cursor. With none, the first press starts at the edge it heads for.
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let ix = match self.cursor {
            None if delta >= 0 => 0,
            None => len - 1,
            Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.set_cursor(ix, cx);
    }

    /// Select song rows on click, the library table's rules: plain takes one,
    /// shift extends from the anchor over song rows, cmd or ctrl toggles.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(id) = self.song_id_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            let range: Vec<_> = (lo..=hi).filter_map(|i| self.song_id_at(i)).collect();
            // Ctrl+Shift stacks the range onto the selection; plain shift replaces.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.into_iter().collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(ix);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(id) {
                self.selected.remove(&id);
            }
            self.anchor = Some(ix);
        } else {
            self.selected = HashSet::from([id]);
            self.anchor = Some(ix);
        }
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    /// Select every shown song, anchored on the first.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = self
            .visible
            .iter()
            .filter_map(|row| match &row.kind {
                RowKind::Track { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        self.anchor = self
            .visible
            .iter()
            .position(|row| matches!(row.kind, RowKind::Track { .. }));
        self.drag_gen += 1;
        self.publish_selection(cx);
        cx.notify();
    }

    fn song_id_at(&self, ix: usize) -> Option<i64> {
        match self.visible.get(ix) {
            Some(Row {
                kind: RowKind::Track { id, .. },
                ..
            }) => Some(*id),
            _ => None,
        }
    }

    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .visible
            .iter()
            .filter_map(|row| match &row.kind {
                RowKind::Track { id, .. } if self.selected.contains(id) => Some(*id),
                _ => None,
            })
            .collect();
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    fn set_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.visible.len() {
            return;
        }
        self.cursor = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    fn list_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let scope = self
            .state
            .query
            .read(cx)
            .filter()
            .values(FilterField::Folder)
            .to_vec();
        let cursor = self.cursor;
        let mut out = Vec::with_capacity(range.len());
        for ix in range {
            // Cloned out so the drag cache below can borrow self mutably.
            let Some(row) = self.visible.get(ix).cloned() else {
                continue;
            };
            let dimmed = match &row.kind {
                RowKind::Folder { dimmed, .. }
                | RowKind::Cue { dimmed, .. }
                | RowKind::Track { dimmed, .. } => *dimmed,
            };
            let row_song_id = match &row.kind {
                RowKind::Track { id, .. } => Some(*id),
                _ => None,
            };
            let lit =
                cursor == Some(ix) || row_song_id.is_some_and(|id| self.selected.contains(&id));
            let base = div()
                .id(("folder-tree-row", ix))
                .w_full()
                .h(palette::scaled_px(ROW_H))
                .pl(px(INDENT) * row.depth as f32 + tokens::SPACE_XS)
                .pr(tokens::SPACE_SM)
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .cursor_pointer()
                .when(dimmed, |d| d.opacity(DIM))
                .when(lit, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
                .hover(|d| d.bg(palette::bg_control_hover()))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.menu_row = Some(ix);
                        this.cursor = Some(ix);
                        // Reselect so the menu acts on what's lit.
                        if let Some(id) = row_song_id
                            && !this.selected.contains(&id)
                        {
                            this.select(ix, Modifiers::default(), cx);
                        }
                        cx.notify();
                    }),
                );
            let built = match &row.kind {
                RowKind::Cue { expanded, ids, .. } => {
                    let image_ids = ids.clone();
                    base.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.type_ahead.clear();
                            this.cursor = Some(ix);
                            // The second press of a double click plays instead of undoing the fold.
                            if event.click_count > 1 {
                                this.play_ids(&image_ids, cx);
                            } else {
                                this.toggle_expand(ix, cx);
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(if *expanded {
                                        icons::CHEVRON_DOWN
                                    } else {
                                        icons::CHEVRON_RIGHT
                                    })
                                    .size(px(12.)),
                            ),
                    )
                    .child(
                        svg()
                            .path(icons::MUSIC)
                            .size(px(12.))
                            .flex_none()
                            .text_color(palette::text_muted()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(row.label.clone()))
                    .child(
                        div().text_xs().text_color(palette::text_muted()).child(
                            SharedString::from(rox_i18n::format::format_int(ids.len() as i64)),
                        ),
                    )
                }
                RowKind::Folder {
                    path,
                    count,
                    has_children,
                    expanded,
                    dimmed: _,
                } => {
                    let scoped = scope.iter().any(|p| p == path);
                    let path = path.clone();
                    let cover = self
                        .config
                        .cover
                        .on_folders()
                        .then(|| self.folder_cover_row(&path))
                        .flatten()
                        .and_then(|row| self.cover_for(row, cx));
                    let drag = self.folder_drag(&path, &row.label, cx);
                    let (fold_path, hover_path) = (path.clone(), path.clone());
                    base.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.type_ahead.clear();
                            this.cursor = Some(ix);
                            if event.click_count > 1 {
                                this.play_folder(&path.clone(), cx);
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        // Fold on click, not press, so a drag off a folder row carries the folder
                        // instead of opening it; gpui drops the click once a drag starts. A double
                        // click's second click lands here too and has to pass through.
                        if event.click_count() > 1 {
                            return;
                        }
                        let mods = event.modifiers();
                        if mods.alt || mods.shift {
                            // Shift or Alt folds the whole branch. Both, because Linux WMs often
                            // grab Alt+click for window drags.
                            this.toggle_expand_deep(&fold_path, cx);
                        } else {
                            this.toggle_expand(ix, cx);
                        }
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        // Stays armed after the pointer leaves: gpui calls hover off as a button
                        // goes down, and dropping the payload there kills the starting drag.
                        if !hovered || this.hover_folder.as_deref() == Some(hover_path.as_str()) {
                            return;
                        }
                        this.hover_folder = Some(hover_path.clone());
                        this.folder_drag = None;
                        cx.notify();
                    }))
                    .when_some(drag, |d, drag| {
                        d.on_drag(drag, |drag, _pos, _window, cx| {
                            cx.new(|_| PlayDragPreview {
                                title: drag.title.clone(),
                                extra: drag.len().saturating_sub(1),
                            })
                        })
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(palette::text_muted())
                            .when(*has_children, |d| {
                                d.child(
                                    svg()
                                        .path(if *expanded {
                                            icons::CHEVRON_DOWN
                                        } else {
                                            icons::CHEVRON_RIGHT
                                        })
                                        .size(px(12.)),
                                )
                            }),
                    )
                    .child(match cover {
                        Some(thumb) => {
                            track_columns::cover_cell(&Some(thumb), track_columns::ROW_HEIGHT_STOCK)
                                .flex_none()
                                .into_any_element()
                        }
                        None => svg()
                            .path(icons::FOLDER)
                            .size(px(12.))
                            .flex_none()
                            .text_color(palette::text_muted())
                            .into_any_element(),
                    })
                    .child(div().flex_1().min_w_0().truncate().child(row.label.clone()))
                    .when(scoped, |d| {
                        d.child(
                            svg()
                                .path(icons::FUNNEL)
                                .size(px(10.))
                                .flex_none()
                                .text_color(palette::accent()),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(rox_i18n::format::format_int(
                                *count as i64,
                            ))),
                    )
                }
                RowKind::Track {
                    row: prow,
                    id,
                    folder,
                    pos,
                    dimmed: _,
                } => {
                    let playing = self.playing == Some(*id);
                    let duration = self
                        .state
                        .library
                        .read(cx)
                        .projection()
                        .map(|p| fmt_ms(p.duration_ms[*prow as usize]))
                        .unwrap_or_default();
                    let drag = self.song_drag(ix, &row.label, cx);
                    let cover = self
                        .config
                        .cover
                        .on_songs()
                        .then(|| self.cover_for(*prow, cx))
                        .flatten();
                    let (folder, pos, id) = (folder.clone(), *pos, *id);
                    base.when(playing && !lit, |d| {
                        d.bg(palette::alpha(palette::highlight(), 0x12))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.type_ahead.clear();
                            this.cursor = Some(ix);
                            if event.click_count > 1 {
                                this.play_track(&folder.clone(), pos, cx);
                            } else if event.modifiers.shift || event.modifiers.secondary() {
                                this.select(ix, event.modifiers, cx);
                            } else if !this.selected.contains(&id) {
                                // Pick an unselected row on press so a drag includes
                                // it. A lit row keeps the set for a whole-set drag;
                                // the collapse waits for the click.
                                this.select(ix, event.modifiers, cx);
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        // A plain click that never became a drag collapses the
                        // selection to this row.
                        let mods = event.modifiers();
                        if event.click_count() == 1
                            && !mods.shift
                            && !mods.secondary()
                            && this.selected.len() > 1
                            && this.selected.contains(&id)
                        {
                            this.select(ix, Modifiers::default(), cx);
                        }
                    }))
                    .when_some(drag, |d, drag| {
                        d.on_drag(drag, |drag, _pos, _window, cx| {
                            cx.new(|_| PlayDragPreview {
                                title: drag.title.clone(),
                                extra: drag.len().saturating_sub(1),
                            })
                        })
                    })
                    // Empty chevron column so songs align with sibling folders.
                    .child(div().flex_none().w(px(16.)))
                    .child(match cover {
                        Some(thumb) => {
                            track_columns::cover_cell(&Some(thumb), track_columns::ROW_HEIGHT_STOCK)
                                .flex_none()
                                .into_any_element()
                        }
                        None => svg()
                            .path(icons::MUSIC)
                            .size(px(12.))
                            .flex_none()
                            .text_color(if playing {
                                palette::highlight()
                            } else {
                                palette::text_muted()
                            })
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(playing, |d| d.text_color(palette::highlight()))
                            .child(row.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(duration)),
                    )
                }
            };
            out.push(built);
        }
        out
    }
}

impl PanelSettings for FolderTreePanel {
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
        &[("Content", icons::FOLDER)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .child(panel::setting_row(
                rox_i18n::t!("folder-tree-cover-art"),
                Some(rox_i18n::t!("folder-tree-cover-art.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("shader-pick-none"), CoverArt::None),
                        (rox_i18n::t!("folder-tree-cover-folders"), CoverArt::Folders),
                        (rox_i18n::t!("folder-tree-cover-songs"), CoverArt::Songs),
                        (rox_i18n::t!("choice-both"), CoverArt::Both),
                    ],
                    self.config.cover,
                    |this: &mut Self, cover, cx| this.set_cover(cover, cx),
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
        Some(
            div()
                .flex()
                .flex_col()
                .gap(crate::settings::ui::SECTION_GAP)
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    rox_i18n::t!("folder-tree-follow-description"),
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.config.resume_playing,
                    rox_i18n::t!("folder-tree-resume-description"),
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    rox_i18n::t!("folder-tree-smooth-description"),
                    |this: &mut Self, on, cx| {
                        this.config.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .child(crate::query::shared_query::search_section(
                    self.config.search,
                    |this: &mut Self, on, cx| this.set_search(on, cx),
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(crate::settings::ui::section(
                    rox_i18n::t!("content-filter"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(panel::setting_row(
                            rox_i18n::t!("folder-tree-nonmatch-folders"),
                            Some(rox_i18n::t!("folder-tree-nonmatch-folders.description")),
                            panel::choices_shared(
                                &[
                                    (rox_i18n::t!("choice-dim"), FilterEffect::Dim),
                                    (rox_i18n::t!("choice-hide"), FilterEffect::Hide),
                                ],
                                self.config.folders,
                                |this: &mut Self, effect, cx| this.set_folders(effect, cx),
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("folder-tree-nonmatch-songs"),
                            Some(rox_i18n::t!("folder-tree-nonmatch-songs.description")),
                            panel::choices_shared(
                                &[
                                    (rox_i18n::t!("choice-dim"), FilterEffect::Dim),
                                    (rox_i18n::t!("choice-hide"), FilterEffect::Hide),
                                ],
                                self.config.songs,
                                |this: &mut Self, effect, cx| this.set_songs(effect, cx),
                                cx,
                            ),
                        )),
                ))
                .into_any_element(),
        )
    }
}

impl FolderTreePanel {
    fn set_cover(&mut self, cover: CoverArt, cx: &mut Context<Self>) {
        self.config.cover = cover;
        cx.notify();
    }

    fn set_folders(&mut self, effect: FilterEffect, cx: &mut Context<Self>) {
        if self.config.folders == effect {
            return;
        }
        self.config.folders = effect;
        self.recount(cx);
    }

    fn set_songs(&mut self, effect: FilterEffect, cx: &mut Context<Self>) {
        if self.config.songs == effect {
            return;
        }
        self.config.songs = effect;
        self.recount(cx);
    }
}

impl QueryFilter for FolderTreePanel {
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
        self.recount(cx);
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

impl EventEmitter<PanelEvent> for FolderTreePanel {}

impl Focusable for FolderTreePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for FolderTreePanel {
    fn panel_name(&self) -> &'static str {
        "folder tree"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("folder-tree-title"),
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

    /// Hands the body's right-click to the rows' menus.
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
        // Sorted so the saved layout doesn't churn with set iteration order.
        let mut config = self.config.clone();
        config.expanded = self.expanded.iter().cloned().collect();
        config.expanded.sort_unstable();
        config.expanded_cues = self.expanded_cues.iter().cloned().collect();
        config.expanded_cues.sort_unstable();
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
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
        let scoped = !self
            .state
            .query
            .read(cx)
            .filter()
            .values(FilterField::Folder)
            .is_empty();
        let weak = cx.entity().downgrade();
        // Checks on the right, so the follow toggle keeps its icon.
        let menu = menu.check_side(Side::Right).item(
            PopupMenuItem::new(rox_i18n::t!("panel-jump-to-playing"))
                .icon(Icon::default().path(icons::DISC))
                .disabled(self.playing.is_none())
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.jump_to_playing(cx));
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("tracking-follow"))
                .icon(Icon::default().path(icons::LOCATE))
                .checked(self.config.follow_playing)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.toggle_follow_playing(cx));
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("folder-tree-collapse-all"))
                .icon(Icon::default().path(icons::MINIMIZE))
                .disabled(self.expanded.is_empty() && self.expanded_cues.is_empty())
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.collapse_all(cx));
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("folder-tree-clear-scope"))
                .icon(Icon::default().path(icons::FUNNEL))
                .disabled(!scoped)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.clear_scope(cx));
                }),
        );
        // Live checks through follow_panel + check_row, not .checked(), so the
        // tick moves while the flyout stays open.
        let menu = menu.separator().label(rox_i18n::t!("panel-menu-display"));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, cover) in [
                (rox_i18n::t!("shader-pick-none"), CoverArt::None),
                (rox_i18n::t!("folder-tree-cover-folders"), CoverArt::Folders),
                (rox_i18n::t!("folder-tree-cover-songs"), CoverArt::Songs),
                (rox_i18n::t!("choice-both"), CoverArt::Both),
            ] {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.cover == cover,
                    move |this, cx| this.set_cover(cover, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("folder-tree-cover-art"),
            submenu,
        ));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, effect) in [
                (rox_i18n::t!("choice-dim"), FilterEffect::Dim),
                (rox_i18n::t!("choice-hide"), FilterEffect::Hide),
            ] {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.folders == effect,
                    move |this, cx| this.set_folders(effect, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("folder-tree-nonmatch-folders"),
            submenu,
        ));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, effect) in [
                (rox_i18n::t!("choice-dim"), FilterEffect::Dim),
                (rox_i18n::t!("choice-hide"), FilterEffect::Hide),
            ] {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.songs == effect,
                    move |this, cx| this.set_songs(effect, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("folder-tree-nonmatch-songs"),
            submenu,
        ));
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
                FolderTreePanel::new(state, config, window, cx)
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

impl Render for FolderTreePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl FolderTreePanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(ix) = self.glide_to {
            let count = self.visible.len();
            match panel::glide_target(&self.scroll, ix, count) {
                Some(target) if !panel::glide_step(&self.scroll, target, dt) => {
                    self.glide_to = None
                }
                // Not laid out yet, or still moving: keep going.
                _ => window.request_animation_frame(),
            }
        }
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // Bindings beat key listeners, so a workspace-bound key never reaches
            // on_panel_key unless a context scopes it out. PanelNav takes back left and
            // right for folding; the type-ahead contexts take back space and tab while a
            // phrase is up.
            .key_context(panel::panel_nav_context(
                &self.type_ahead,
                self.type_ahead_at,
            ))
            // Any press ends the phrase, since the cursor moved by hand. Capture
            // phase, so rows that stop the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_panel_key(event, cx)),
            )
            // Any scroll or press counts as browsing for the idle clock.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.touch_resume(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.touch_resume(cx)),
            );
        if self.visible.is_empty() {
            let searching =
                !self.effective_query(cx).is_empty() || !self.effective_filter(cx).is_empty();
            let message = if searching {
                rox_i18n::t!("picker-no-matches")
            } else {
                rox_i18n::t!("folder-tree-empty")
            };
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(tokens::SPACE_MD)
                    .text_center()
                    .text_color(palette::text_faint())
                    .child(message),
            );
        }
        let count = self.visible.len();
        let this = cx.entity().downgrade();
        let content = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .child(
                uniform_list("folder-tree-rows", count, move |range, _, cx| {
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
            .children(crate::panel::type_ahead_overlay(
                &self.type_ahead,
                self.type_ahead_at,
            ))
            // Any press takes focus so type-ahead works without clicking a row.
            // Capture phase runs before the rows record a menu target, so a right press
            // off the rows falls back to the panel menu.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, _| {
                window.focus(&this.focus);
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        // Row menu: the shared track actions plus the folder scope, then the panel
        // menu so a click over the list never dead-ends.
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            enum Target {
                Cue { ids: Vec<i64> },
                Folder { path: String, scoped: bool },
                Track { id: i64, folder: String, pos: usize },
            }
            let target = {
                let panel = this.read(cx);
                panel.menu_row.and_then(|ix| {
                    panel.visible.get(ix).map(|row| match &row.kind {
                        RowKind::Cue { ids, .. } => Target::Cue { ids: ids.clone() },
                        RowKind::Folder { path, .. } => Target::Folder {
                            scoped: panel
                                .state
                                .query
                                .read(cx)
                                .filter()
                                .values(FilterField::Folder)
                                == [path.clone()],
                            path: path.clone(),
                        },
                        RowKind::Track {
                            id, folder, pos, ..
                        } => Target::Track {
                            id: *id,
                            folder: folder.clone(),
                            pos: *pos,
                        },
                    })
                })
            };
            let Some(target) = target else {
                return this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
            };
            let state = this.read(cx).state.clone();
            let menu = match target {
                Target::Cue { ids } => {
                    let label = rox_i18n::t!("folder-tree-play-songs", count = ids.len() as u64)
                        .to_string();
                    let play_ids = ids.clone();
                    let play_panel = weak.clone();
                    panel::track_actions(menu, state, ids, label, window, cx, move |_, cx| {
                        if let Some(this) = play_panel.upgrade() {
                            this.update(cx, |this, cx| this.play_ids(&play_ids, cx));
                        }
                    })
                }
                Target::Folder { path, scoped } => {
                    let ids: Vec<i64> = {
                        let panel = this.read(cx);
                        let rows = panel.subtree_rows(&path);
                        panel
                            .state
                            .library
                            .read(cx)
                            .projection()
                            .map(|p| rows.iter().map(|&r| p.db_id[r as usize]).collect())
                            .unwrap_or_default()
                    };
                    let play_path = path.clone();
                    let play_panel = weak.clone();
                    let menu = panel::track_actions(
                        menu,
                        state,
                        ids,
                        rox_i18n::t!("folder-tree-play-folder"),
                        window,
                        cx,
                        move |_, cx| {
                            let Some(this) = play_panel.upgrade() else {
                                return;
                            };
                            this.update(cx, |this, cx| this.play_folder(&play_path.clone(), cx));
                        },
                    );
                    let scope_panel = weak.clone();
                    let scope_path = path.clone();
                    let menu = menu.separator().item(
                        PopupMenuItem::new(if scoped {
                            rox_i18n::t!("folder-tree-clear-scope")
                        } else {
                            rox_i18n::t!("folder-tree-scope-to-folder")
                        })
                        .icon(Icon::default().path(icons::FUNNEL))
                        .on_click(move |_, _, cx| {
                            let Some(this) = scope_panel.upgrade() else {
                                return;
                            };
                            this.update(cx, |this, cx| this.toggle_scope(scope_path.clone(), cx));
                        }),
                    );
                    let open = this.read(cx).expanded.contains(&path);
                    let deep_panel = weak.clone();
                    menu.item(
                        PopupMenuItem::new(if open {
                            rox_i18n::t!("folder-tree-collapse-branch")
                        } else {
                            rox_i18n::t!("folder-tree-expand-branch")
                        })
                        .icon(Icon::default().path(if open {
                            icons::CHEVRON_RIGHT
                        } else {
                            icons::CHEVRON_DOWN
                        }))
                        .on_click(move |_, _, cx| {
                            let Some(this) = deep_panel.upgrade() else {
                                return;
                            };
                            this.update(cx, |this, cx| this.toggle_expand_deep(&path.clone(), cx));
                        }),
                    )
                }
                Target::Track { id, folder, pos } => {
                    // Inside a multi-selection the menu acts on the whole set.
                    let selection: Vec<i64> = {
                        let panel = this.read(cx);
                        if panel.selected.len() > 1 && panel.selected.contains(&id) {
                            panel
                                .visible
                                .iter()
                                .filter_map(|row| match &row.kind {
                                    RowKind::Track { id, .. } if panel.selected.contains(id) => {
                                        Some(*id)
                                    }
                                    _ => None,
                                })
                                .collect()
                        } else {
                            vec![id]
                        }
                    };
                    let label =
                        rox_i18n::t!("folder-tree-play-songs", count = selection.len() as u64)
                            .to_string();
                    let play_panel = weak.clone();
                    let play_ids = selection.clone();
                    panel::track_actions(menu, state, selection, label, window, cx, move |_, cx| {
                        let Some(this) = play_panel.upgrade() else {
                            return;
                        };
                        this.update(cx, |this, cx| {
                            if play_ids.len() > 1 {
                                this.play_ids(&play_ids, cx);
                            } else {
                                this.play_track(&folder.clone(), pos, cx);
                            }
                        });
                    })
                }
            };
            this.update(cx, |this, cx| {
                this.dropdown_menu(menu.separator(), window, cx)
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(row: u32, path: &str, sub: u16, title: &str) -> SongRow {
        SongRow {
            row,
            id: i64::from(row),
            title: title.to_owned().into(),
            key: Some(TrackKey {
                source: rox_library::cue::local(),
                path: path.into(),
                sub,
            }),
        }
    }

    fn rows(groups: &[SongGroup], expanded: &[&str], dimmed: &[u32]) -> Vec<Row> {
        let mut out = Vec::new();
        append_songs(
            &mut out,
            groups,
            "Mixes",
            1,
            &expanded.iter().map(PathBuf::from).collect(),
            &dimmed.iter().copied().collect(),
        );
        out
    }

    fn labels(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.label.as_ref()).collect()
    }

    #[test]
    fn ordinary_file_keeps_filename_and_identity() {
        let groups = group_songs(vec![song(7, "ordinary-song.mp3", 0, "Title")]);
        let out = rows(&groups, &[], &[]);
        assert_eq!(labels(&out), ["ordinary-song.mp3"]);
        assert!(matches!(
            out[0].kind,
            RowKind::Track {
                row: 7,
                id: 7,
                pos: 0,
                ..
            }
        ));
        assert_eq!(out[0].depth, 1);
    }

    #[test]
    fn cue_children_use_subsong_numbers_and_titles() {
        let groups = group_songs(vec![
            song(8, "mix.mp3", 4, "Four"),
            song(3, "mix.mp3", 1, "One"),
            song(6, "mix.mp3", 2, "Two"),
        ]);
        let collapsed = rows(&groups, &[], &[]);
        assert_eq!(labels(&collapsed), ["mix.mp3"]);
        assert!(
            matches!(&collapsed[0].kind, RowKind::Cue { ids, expanded: false, .. } if ids == &[3, 6, 8])
        );
        let out = rows(&groups, &["mix.mp3"], &[]);
        assert_eq!(labels(&out), ["mix.mp3", "01. One", "02. Two", "04. Four"]);
        assert_eq!(out[3].depth, 2);
        assert!(matches!(
            out[3].kind,
            RowKind::Track {
                row: 8,
                id: 8,
                pos: 2,
                ..
            }
        ));
    }

    #[test]
    fn mixed_images_group_independent_of_input_order_and_sort_naturally() {
        let groups = group_songs(vec![
            song(0, "mix10.flac", 4, "Ten four"),
            song(1, "mix2.flac", 2, "Two two"),
            song(2, "mix3.mp3", 0, "Ordinary"),
            song(3, "mix10.flac", 1, "Ten one"),
            song(4, "mix2.flac", 1, "Two one"),
        ]);
        assert_eq!(groups.len(), 3);
        let out = rows(&groups, &["mix2.flac", "mix10.flac"], &[]);
        assert_eq!(
            labels(&out),
            [
                "mix2.flac",
                "01. Two one",
                "02. Two two",
                "mix3.mp3",
                "mix10.flac",
                "01. Ten one",
                "04. Ten four"
            ]
        );
        let collapsed = rows(&groups, &[], &[]);
        // Hidden children still occupy the existing folder playback list.
        assert!(matches!(
            collapsed[1].kind,
            RowKind::Track { id: 2, pos: 2, .. }
        ));
    }

    #[test]
    fn grouping_uses_full_path_and_keeps_plain_tracks_separate() {
        let groups = group_songs(vec![
            song(0, "a/mix.flac", 1, "A"),
            song(1, "b/mix.flac", 1, "B"),
            song(2, "a/mix.flac", 0, "Plain"),
        ]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.iter().filter(|group| group.cue.is_some()).count(), 2);
    }

    #[test]
    fn filter_keeps_parent_of_matching_child_and_dims_only_misses() {
        // Recount applies Hide before flattening; one surviving child is
        // still a CUE image, even when its siblings have been filtered out.
        let hidden = group_songs(vec![song(4, "mix.flac", 4, "Match")]);
        assert_eq!(
            labels(&rows(&hidden, &["mix.flac"], &[])),
            ["mix.flac", "04. Match"]
        );
        let groups = group_songs(vec![
            song(1, "mix.flac", 1, "Miss"),
            song(4, "mix.flac", 4, "Match"),
        ]);
        let out = rows(&groups, &["mix.flac"], &[1]);
        assert!(matches!(out[0].kind, RowKind::Cue { dimmed: false, .. }));
        assert!(matches!(out[1].kind, RowKind::Track { dimmed: true, .. }));
        assert!(matches!(out[2].kind, RowKind::Track { dimmed: false, .. }));
        assert!(matches!(
            rows(&groups, &[], &[1, 4])[0].kind,
            RowKind::Cue { dimmed: true, .. }
        ));
    }

    #[test]
    fn reveal_opens_the_playing_image_before_locating_its_child() {
        let groups = group_songs(vec![song(4, "mix.flac", 4, "Playing")]);
        let mut expanded = HashSet::new();
        let mut out = Vec::new();
        append_songs(&mut out, &groups, "Mixes", 1, &expanded, &HashSet::new());
        assert_eq!(out.len(), 1);
        reveal_cue(
            &mut expanded,
            &TrackKey {
                source: rox_library::cue::local(),
                path: "mix.flac".into(),
                sub: 4,
            },
        );
        out.clear();
        append_songs(&mut out, &groups, "Mixes", 1, &expanded, &HashSet::new());
        assert_eq!(
            out.iter()
                .position(|row| matches!(row.kind, RowKind::Track { id: 4, .. })),
            Some(1)
        );
        reveal_cue(
            &mut expanded,
            &TrackKey {
                source: rox_library::cue::local(),
                path: "plain.mp3".into(),
                sub: 0,
            },
        );
        assert!(!expanded.contains(&PathBuf::from("plain.mp3")));
    }

    #[test]
    fn cue_expansion_config_is_backward_compatible_and_round_trips() {
        let old: FolderTreeConfig = serde_json::from_str(r#"{"expanded":["Mixes"]}"#).unwrap();
        assert!(old.expanded_cues.is_empty());
        let config = FolderTreeConfig {
            expanded_cues: vec!["Mixes/mix.flac".into()],
            ..old
        };
        let saved = serde_json::to_string(&config).unwrap();
        let restored: FolderTreeConfig = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.expanded, ["Mixes"]);
        assert_eq!(restored.expanded_cues, [PathBuf::from("Mixes/mix.flac")]);
    }
}
