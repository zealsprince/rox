//! The folder tree panel: the library's folder hierarchy as an
//! expand/collapse tree, reconstructed from the projection's interned
//! folder strings, never a scan of the filesystem. The shared prefix
//! above the music (the mount point, the home dir) collapses away, so the
//! top nodes are the folders where the library actually starts. Expanding
//! a folder shows its subfolders and then its songs; a double click plays
//! from there, and the right-click menu has the track actions every
//! song surface shares plus the folder-scope filter, which narrows the
//! shared query to the folder's whole subtree with a single pick. The
//! active query narrows the tree too (the shared one by default, the
//! panel's own box or the app-wide selection per config), and folders left
//! with no matching songs drop out.

use std::collections::{HashMap, HashSet};
use std::path::MAIN_SEPARATOR;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, ScrollStrategy,
    ScrollWheelEvent, SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity,
    Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Side};
use rox_core::fmt::fmt_ms;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::folders::{build_roots, node_at, sum_counts, Node};
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

/// One row's height, the filter panel's, so the two read as one family.
const ROW_H: f32 = 26.;

/// How far each depth level steps in.
const INDENT: f32 = 14.;

/// The opacity a dimmed row (outside the active facet filter) draws at.
const DIM: f32 = 0.4;

/// Where the tree shows cover art in place of the row icon: nowhere, on
/// the folder rows (the album tile), on the song rows, or both.
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
    /// Whether folder rows show the album tile.
    fn on_folders(self) -> bool {
        matches!(self, CoverArt::Folders | CoverArt::Both)
    }

    /// Whether song rows show their cover.
    fn on_songs(self) -> bool {
        matches!(self, CoverArt::Songs | CoverArt::Both)
    }
}

/// What the tree does with the folders and songs the active query leaves
/// out (text terms and facet picks both): dim them in place so the branch
/// still reads whole, or drop them so only the matches show. Folders and
/// songs each have their own choice, so the tree can hide the folders with
/// no match while still dimming the stray songs inside the folders that do.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterEffect {
    #[default]
    Dim,
    Hide,
}

/// The folder tree panel's per-view config: what a saved layout restores.
/// The shared chrome plus the cover-art and filter knobs; the folder scope
/// is app state, transient like the rest of the filter, and the expand
/// state is per-session.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderTreeConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Where cover art shows in place of the row icon.
    pub cover: CoverArt,
    /// What happens to folders with no match under the active query.
    pub folders: FilterEffect,
    /// What happens to non-matching songs inside a folder that's shown.
    pub songs: FilterEffect,
    /// Reveal and scroll to the playing track whenever it changes.
    pub follow_playing: bool,
    /// Scroll back to the playing track after browsing stops.
    pub resume_playing: bool,
    /// Glide to the track instead of jumping.
    pub smooth_follow: bool,
    /// Whether the search box shows; the query only filters while it does.
    pub search: bool,
    /// Follow the shared query, or filter by this panel's own box.
    pub query_source: QuerySource,
    /// The panel's own query, kept while following the shared one.
    pub query: String,
    /// The folders left open when the layout was saved, so a relaunch
    /// reopens the tree where it was instead of folding back to the
    /// roots. Paths a rescan no longer knows just sit inert in the set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded: Vec<String>,
}

impl Default for FolderTreeConfig {
    fn default() -> Self {
        FolderTreeConfig {
            chrome: PanelChrome::default(),
            cover: CoverArt::default(),
            // Hide the folders that miss, so a search narrows the tree; keep
            // and dim the stray songs inside the folders that hit, so a
            // folder still shows its whole contents.
            folders: FilterEffect::Hide,
            songs: FilterEffect::Dim,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            search: false,
            query_source: QuerySource::default(),
            query: String::new(),
            expanded: Vec::new(),
        }
    }
}

/// What one visible row stands for: a folder of the tree, or one of a
/// folder's songs.
#[derive(Clone)]
enum RowKind {
    Folder {
        path: String,
        count: u32,
        has_children: bool,
        expanded: bool,
        /// Drawn faint: a folder whose subtree holds no filter match, in
        /// Dim mode.
        dimmed: bool,
    },
    Track {
        /// The projection row, for the duration and db id.
        row: u32,
        id: i64,
        /// The owning folder's path and this song's position in its list,
        /// what a play-from-here resolves against.
        folder: String,
        pos: usize,
        /// Drawn faint: a song outside the active facet filter, in Dim mode.
        dimmed: bool,
    },
}

/// One visible row of the flattened tree, what the uniform list renders.
#[derive(Clone)]
struct Row {
    label: SharedString,
    depth: usize,
    kind: RowKind,
}

pub struct FolderTreePanel {
    state: AppState,
    config: FolderTreeConfig,
    focus: FocusHandle,
    /// The panel's search box, shown per config; its query filters the tree
    /// through whichever source is active.
    search: Entity<SearchBox>,
    /// A pending box reset from a source toggle or a shared-query change,
    /// consumed in render where a window exists.
    resync_box: bool,
    /// The track ids pinned while following the app-wide selection.
    selection_ids: Vec<i64>,
    /// The top-level folders after collapsing the shared prefix, structure
    /// rebuilt on a library update, counts on every query change.
    roots: Vec<Node>,
    /// Each folder's own context songs by path, filename ordered; folders
    /// with none stay out. Rebuilt with the counts.
    folder_tracks: HashMap<String, Vec<u32>>,
    /// Shown songs outside the active facet filter, drawn faint in Dim mode.
    /// Empty in Hide mode (non-matches are dropped) and when no filter is
    /// active.
    dimmed_songs: HashSet<u32>,
    /// The flattened visible rows, rebuilt on expand and recount.
    visible: Vec<Row>,
    /// The expanded folders by path. Kept across rescans; top-level nodes
    /// seed in expanded once.
    expanded: HashSet<String>,
    seeded: bool,
    scroll: UniformListScrollHandle,
    /// The keyboard-and-click cursor, an index into `visible`: the lit
    /// row, where arrows move from and enter acts. None until a key or
    /// click sets one.
    cursor: Option<usize>,
    /// The selected songs by library id, the multi-select set the shared
    /// selection and a drag read from. Songs only; folders aren't selectable.
    selected: HashSet<i64>,
    /// The shift-range anchor, a `visible` index into the last plainly
    /// clicked song row.
    anchor: Option<usize>,
    /// The row under the last right press, what the context menu acts on;
    /// cleared when the press falls off the rows.
    menu_row: Option<usize>,
    /// The playing track's path and library id, the highlight's key, the
    /// history panel's follow.
    playing_key: Option<TrackKey>,
    playing: Option<i64>,
    /// Per-track paths resolved for drag payloads, so a hover frame never
    /// repeats the store lookup. Cleared on a library update.
    drag_keys: HashMap<i64, Option<TrackKey>>,
    /// Bumped whenever the selection or the visible order changes, keying the
    /// drag-set cache so a grab inside a big selection shares one Arc across
    /// every visible selected row instead of rebuilding the set per row.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[TrackKey]>)>,
    /// The idle clock behind resume: a browse gesture arms it, its wake
    /// scrolls back to the playing track once the panel goes untouched.
    resume_idle: ResumeIdle,
    /// The follow glide's target row and its per-frame clock, stepped in
    /// render like the library's; None when nothing is easing.
    glide_to: Option<usize>,
    glide_tick: Instant,
    /// The type-ahead phrase and when its last keystroke arrived, so a
    /// quick run of letters jumps to a row by prefix.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    /// The tab panel this panel is currently in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
    _player_changed: Subscription,
    _search_events: Subscription,
    _selection_changed: Subscription,
    /// Drops the phrase when focus leaves the panel, so tab goes back to
    /// walking panels instead of cycling a phrase from a past visit.
    _type_ahead_blur: Subscription,
}

impl FolderTreePanel {
    pub fn new(
        state: AppState,
        config: FolderTreeConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The folder set changes when the library rescans; rebuild the
        // structure. Counts and the scope highlight follow the shared
        // query, our own scope writes included. Recount is idempotent, so
        // the echo settles in one pass.
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
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Restored as selection-following, it opens on whatever is picked
        // now, rather than blank until the next pick.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        // A restored layout carries its open folders; seed from them and
        // skip the root seeding, so the tree comes back as it was left.
        let expanded: HashSet<String> = config.expanded.iter().cloned().collect();
        let seeded = !expanded.is_empty();
        let focus = cx.focus_handle().tab_stop(true);
        // The phrase outlives its badge, so it needs an end: leaving the
        // panel drops it, which is also what hands tab back to traversal.
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
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Rebuild the hierarchy from the projection's folder set, then count.
    /// The structure only follows the library, never the query, so typing
    /// a search never restructures the branches: it only hides the empty
    /// ones.
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

    /// Regroup the songs per folder and recount every subtree. The tree is
    /// the full library hierarchy; the active query (the text terms and
    /// every facet pick, the folder scope included) marks which songs
    /// match, and [`FilterEffect`] decides the rest: a folder with no match
    /// in its subtree hides or dims per `folders`, a non-matching song
    /// inside a shown folder hides or dims per `songs`. Then reflatten.
    fn recount(&mut self, cx: &mut Context<Self>) {
        {
            let song_hide = self.config.songs == FilterEffect::Hide;
            // Whichever source is active: the shared query, the panel's own
            // box, or the app-wide selection pinned as an id filter.
            let (text, facet) = (self.effective_query(cx), self.effective_filter(cx));
            let library = self.state.library.read(cx);
            self.folder_tracks.clear();
            self.dimmed_songs.clear();
            if let Some(projection) = library.projection() {
                let len = projection.len();
                // Two masks over the catalog: the text hits and the facet
                // picks. None on either means it constrains nothing, so a
                // song passes it. A song matches when it passes both.
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
                // The songs each folder lists: all of them in Dim, only the
                // matches in Hide. Non-matches that stay get marked faint.
                let mut listed: Vec<Vec<u32>> = vec![Vec::new(); nsym];
                for row in 0..len {
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
                    // Bare-filename tracks intern to the empty folder and
                    // never get a node; skip them.
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

    /// Reflatten the visible rows from the roots and the expand set:
    /// subfolders first, then the folder's own songs. Folders with no
    /// context songs anywhere below stay out, so a search leaves only the
    /// branches that still hold matches.
    fn flatten(&mut self, cx: &mut Context<Self>) {
        struct Walk<'a> {
            expanded: &'a HashSet<String>,
            folder_tracks: &'a HashMap<String, Vec<u32>>,
            dimmed_songs: &'a HashSet<u32>,
            /// Hide the folders a filter leaves with no match, or keep them
            /// faint.
            folder_hide: bool,
            labels: HashMap<u32, (SharedString, i64)>,
            out: Vec<Row>,
        }
        impl Walk<'_> {
            fn folder(&mut self, node: &Node, depth: usize) {
                // A genuinely empty branch is never a row.
                if node.total == 0 {
                    return;
                }
                // No match anywhere below: Hide drops the whole branch, Dim
                // keeps it faint.
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
                        // The badge reads the matches, so it lines up with
                        // what a search leaves lit.
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
                let Some(tracks) = tracks else { return };
                for (pos, &row) in tracks.iter().enumerate() {
                    let Some((label, id)) = self.labels.get(&row) else {
                        continue;
                    };
                    self.out.push(Row {
                        label: label.clone(),
                        depth: depth + 1,
                        kind: RowKind::Track {
                            row,
                            id: *id,
                            folder: node.path.clone(),
                            pos,
                            dimmed: self.dimmed_songs.contains(&row),
                        },
                    });
                }
            }
        }
        // The song rows in expanded folders, each with its db id and the
        // title we fall back to. Gathered under an immutable library borrow
        // before the path resolution below needs `&mut self`.
        let songs: Vec<(u32, i64, SharedString)> = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => self
                    .folder_tracks
                    .iter()
                    .filter(|(path, _)| self.expanded.contains(*path))
                    .flat_map(|(_, rows)| rows)
                    .map(|&row| {
                        (
                            row,
                            projection.db_id[row as usize],
                            SharedString::from(projection.title.get(row as usize).to_string()),
                        )
                    })
                    .collect(),
                None => Vec::new(),
            }
        };
        // The label is the file's own name, so the tree matches the folder
        // on disk; a row with no resolvable path or an all-extension name
        // falls back to its title. The path resolves through the shared
        // cache, so covers and drags reuse it.
        let labels: HashMap<u32, (SharedString, i64)> = songs
            .into_iter()
            .map(|(row, id, title)| {
                let label = self
                    .key_for(id, cx)
                    .as_ref()
                    .and_then(|key| key.path.file_name())
                    .map(|name| SharedString::from(name.to_string_lossy().into_owned()))
                    .filter(|name| !name.is_empty())
                    .unwrap_or(title);
                (row, (label, id))
            })
            .collect();
        // Order each expanded folder's songs by filename, so the tree reads
        // top to bottom like the folder on disk and a track's `pos` (what a
        // play-from-here counts against) matches what's shown. Collapsed
        // folders keep their scan order; only the counts read them.
        let expanded_paths: Vec<String> = self
            .folder_tracks
            .keys()
            .filter(|path| self.expanded.contains(*path))
            .cloned()
            .collect();
        // Lower each label once up front rather than per comparison: the sort
        // touched to_lowercase O(n log n) times per expanded folder on every
        // keystroke, allocating a fresh String each call.
        let sort_keys: HashMap<u32, String> = labels
            .iter()
            .map(|(&row, (label, _))| (row, label.to_lowercase()))
            .collect();
        for path in expanded_paths {
            if let Some(rows) = self.folder_tracks.get_mut(&path) {
                rows.sort_by(|a, b| {
                    let name = |row: &u32| sort_keys.get(row).map(String::as_str).unwrap_or("");
                    natural_cmp(name(a), name(b))
                });
            }
        }
        let mut walk = Walk {
            expanded: &self.expanded,
            folder_tracks: &self.folder_tracks,
            dimmed_songs: &self.dimmed_songs,
            folder_hide: self.config.folders == FilterEffect::Hide,
            labels,
            out: Vec::new(),
        };
        for root in &self.roots {
            walk.folder(root, 0);
        }
        self.visible = walk.out;
        // The visible order drives drag order, so a reflow invalidates the
        // cached drag set even when the selection ids are unchanged.
        self.drag_gen += 1;
        // The row set moved under the indices; drop the ones now off the end.
        // The selection keys on ids, so it comes through untouched.
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

    /// Follow the player: on a track change, resolve the playing track to
    /// its id, the history panel's move. The highlight matches track rows
    /// by that id.
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
        // Reveal and chase the new track when the follow is on; the move
        // notifies on its own.
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Open every branch from a root down to `path`, so the folder's row
    /// shows even if it or an ancestor was collapsed. Takes the same prefix
    /// descent as [`node_at`], banking each node on the way down.
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

    /// Open the branches down to the playing track's folder, reflatten, and
    /// hand back its row index. None when nothing is playing or a filter with
    /// Hide keeps the track off the tree, since there's no row to scroll to
    /// then. The shared step behind the menu jump and the automatic follow.
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
        self.flatten(cx);
        self.visible
            .iter()
            .position(|row| matches!(row.kind, RowKind::Track { id: rid, .. } if rid == id))
    }

    /// The menu's jump: reveal the playing track and put the cursor on it,
    /// which selects it, publishes, and scrolls it into view in one move.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.reveal_playing(cx) {
            self.set_cursor(ix, cx);
        }
    }

    /// Reveal the playing track and scroll it into view: a glide when smooth
    /// is on, a jump otherwise. Scroll only, no cursor move, since the
    /// deliberate jump owns the selection. Runs on a track change while
    /// follow is on and on the idle resume.
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

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the tree scrolls back to the playing track once the user steps away.
    /// A no-op unless the resume is on, so an off panel spends nothing per
    /// gesture.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// What the idle wake does: scroll back to the playing track, so long as
    /// the resume is still on. The clock only fires once the tree has gone
    /// untouched a full window, so no extra idle check is needed here.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// Map the box's events onto the panel: a changed query recounts the
    /// tree, and a focus or dismiss repaints the tab title row that holds
    /// the box.
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

    /// Show or hide the panel's own search box, recounting the tree. The
    /// config is stored in the layout dump, so the tab-panel repaint applies
    /// it.
    fn set_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.search = on;
        self.rebuild_query_view(cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// The menu's follow toggle: flip the follow and catch up right away when
    /// turning it on, the same move as the settings switch.
    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.config.follow_playing = !self.config.follow_playing;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Fold one folder row open or shut.
    fn toggle_expand(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(Row {
            kind: RowKind::Folder { path, .. },
            ..
        }) = self.visible.get(ix)
        else {
            return;
        };
        let path = path.clone();
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.flatten(cx);
    }

    /// Fold a folder and its whole subtree open or shut in one move, the
    /// alt-click and branch-menu answer to deep trees: open when the
    /// folder itself is shut, shut everything below otherwise.
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

    /// Fold every branch shut, leaving only the root rows. The follow glide
    /// stops too: its target index just moved under it.
    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if self.expanded.is_empty() {
            return;
        }
        self.expanded.clear();
        self.glide_to = None;
        self.flatten(cx);
    }

    /// Scope the shared folder filter to one folder's subtree, or clear it
    /// if that folder is the scope already. One pick covers the branch,
    /// since the filter matches folders by prefix, so this stays cheap at
    /// any depth.
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
        // The scope highlight reads the shared filter live; while the panel
        // follows its own query the shared-query echo returns early, so
        // repaint here.
        cx.notify();
    }

    /// Drop the folder scope, the panel menu's clear.
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

    /// A folder's whole subtree as projection rows, in the tree's order:
    /// each folder's subfolders first, then its own songs. What Play
    /// Folder queues and the folder context menu acts on.
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

    /// A representative projection row for a folder's cover: its own first
    /// song, or the first song in its subtree, top-down. None when the
    /// subtree holds no context songs.
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

    /// The cached file path for a track id, resolved once through the store
    /// and shared by the drag payloads and the cover thumbnails.
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

    /// The cover thumbnail for a projection row's file, resolved through the
    /// path cache and the shared thumbnail service. None when the file has
    /// no path yet; a pending or missing cover comes back as a placeholder
    /// tile.
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

    /// Queue a set of projection rows on the shared player with the cursor
    /// at `start`, capped like every other play surface.
    fn play_rows(&mut self, rows: &[u32], start: usize, cx: &mut Context<Self>) {
        // Keep the clicked row inside the capped window, the history
        // panel's centering.
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

    /// Queue an explicit set of library ids from the front, the multi-select
    /// menu's play. Order is the caller's (view order for a selection).
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

    /// Play a folder's subtree from the top; the double click's and the
    /// context menu's move.
    fn play_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.subtree_rows(path);
        self.play_rows(&rows, 0, cx);
    }

    /// Play a folder's own songs starting at one of them.
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
    /// dragged row is part of a multi-selection, otherwise just this row.
    /// Keys resolve through the shared cache, the library table's route
    /// into the play-drag story.
    fn song_drag(&mut self, ix: usize, title: &SharedString, cx: &App) -> Option<PlayDrag> {
        let id = self.song_id_at(ix)?;
        // A grab inside a multi-selection takes the whole set in visible order,
        // built once per selection or reflow and shared behind an Arc so it's a
        // refcount bump per row, not a rebuild. Outside it, just this song.
        let keys: Arc<[TrackKey]> = if self.selected.len() > 1 && self.selected.contains(&id) {
            if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.drag_gen) {
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
                self.drag_set = Some((self.drag_gen, set));
            }
            self.drag_set.as_ref().map(|(_, set)| set.clone())?
        } else {
            self.key_for(id, cx).into_iter().collect()
        };
        if keys.is_empty() {
            return None;
        }
        Some(PlayDrag {
            keys,
            title: title.clone(),
        })
    }

    /// Browse from the keyboard while the panel is focused: up and down
    /// move the cursor, left and right fold folders, enter folds a folder
    /// or plays a song, and plain typing jumps to a row by prefix. The
    /// filter panel's keys plus the tree's fold pair.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        // Cmd/Ctrl+A selects every shown song, before the modifier guard
        // below rejects the rest of the chorded keys.
        if keystroke.modifiers.secondary() && keystroke.key == "a" {
            self.select_all(cx);
            return;
        }
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // Arrow and type-ahead navigation is browsing too, so it restarts
        // the idle clock the same as a scroll or a click.
        self.touch_resume(cx);
        match keystroke.key.as_str() {
            // Escape drops a phrase, which is what hands tab back to
            // panel traversal.
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
                let Some(Row {
                    kind: RowKind::Folder { path, expanded, .. },
                    ..
                }) = self.visible.get(ix)
                else {
                    return;
                };
                // Shift folds the whole branch, the arrows' spelling of the
                // shift-click.
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
                        kind: RowKind::Folder { .. },
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
                // Consumed as type-ahead text: stop it here so it doesn't
                // also match the workspace's space-bound TogglePlayback
                // binding, which this panel otherwise inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead phrase and jump to its next match
    /// among the visible rows. A grown phrase re-tests the cursor's own
    /// row first so refining a match stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // The badge shows the phrase now and leaves when the window
        // lapses; a miss below still updated it, so repaint either way.
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

    /// Drop the phrase, handing tab back to Root's panel traversal. True
    /// when there was one, for the escape ladder.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Step to the phrase's neighbouring match, Tab's cycle, dispatched
    /// off the cycle-scoped tab bindings. Deliberately leaves the window
    /// stamp alone: the badge and the letter grouping belong to typing,
    /// so a run of tabs steps silently rather than reviving them.
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

    /// Step the cursor; the first press with no cursor starts at the edge
    /// it heads toward.
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

    /// Select song rows on click, the library table's rules. A plain click
    /// takes just this song; shift extends the range from the anchor over
    /// the song rows between (folders and gaps skipped); cmd or ctrl toggles
    /// this one. The shared selection follows so the panels that read it turn
    /// to the set.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(id) = self.song_id_at(ix) else {
            return;
        };
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            let range: Vec<_> = (lo..=hi).filter_map(|i| self.song_id_at(i)).collect();
            // Ctrl+Shift stacks the range onto the selection so you can
            // skip a run and grab a second block; plain shift replaces.
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

    /// Select every song currently shown, the Ctrl+A move; the anchor goes
    /// on the first so a follow-up shift-click narrows from the top.
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

    /// The library id of a song row, or None for a folder row.
    fn song_id_at(&self, ix: usize) -> Option<i64> {
        match self.visible.get(ix) {
            Some(Row {
                kind: RowKind::Track { id, .. },
                ..
            }) => Some(*id),
            _ => None,
        }
    }

    /// Push the selected songs onto the shared selection, in view order.
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

    /// Put the cursor on a row and scroll it into view.
    fn set_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.visible.len() {
            return;
        }
        self.cursor = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        cx.notify();
    }

    /// The visible slice of the tree's rows. Folder rows fold on click and
    /// play on double click, with the subtree count on the right and a
    /// funnel marking the scoped one; song rows select on click, play on
    /// double click, and drag onto anything that takes a play drag.
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
                RowKind::Folder { dimmed, .. } | RowKind::Track { dimmed, .. } => *dimmed,
            };
            let row_song_id = match &row.kind {
                RowKind::Track { id, .. } => Some(*id),
                _ => None,
            };
            // A selected song or the cursor row gets the accent wash.
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
                // Outside the active filter, in Dim mode: faint but still
                // there to browse, click, and play.
                .when(dimmed, |d| d.opacity(DIM))
                .when(lit, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
                .hover(|d| d.bg(palette::bg_control_hover()))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.menu_row = Some(ix);
                        this.cursor = Some(ix);
                        // A right click on a song outside the selection
                        // reselects just it, so the menu acts on what's lit.
                        if let Some(id) = row_song_id {
                            if !this.selected.contains(&id) {
                                this.select(ix, Modifiers::default(), cx);
                            }
                        }
                        cx.notify();
                    }),
                );
            let built = match &row.kind {
                RowKind::Folder {
                    path,
                    count,
                    has_children,
                    expanded,
                    dimmed: _,
                } => {
                    let scoped = scope.iter().any(|p| p == path);
                    let path = path.clone();
                    // The album tile in place of the folder icon: the
                    // folder's first song stands in for its art.
                    let cover = self
                        .config
                        .cover
                        .on_folders()
                        .then(|| self.folder_cover_row(&path))
                        .flatten()
                        .and_then(|row| self.cover_for(row, cx));
                    base.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus);
                            this.type_ahead.clear();
                            this.cursor = Some(ix);
                            if event.click_count > 1 {
                                this.play_folder(&path.clone(), cx);
                            } else if event.modifiers.alt || event.modifiers.shift {
                                // Shift or Alt folds the whole branch, the
                                // file manager's deep toggle. Both spellings
                                // because Linux WMs commonly grab Alt+click
                                // for window drags before the app sees it.
                                this.toggle_expand_deep(&path.clone(), cx);
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
                        Some(thumb) => track_columns::cover_cell(&Some(thumb))
                            .flex_none()
                            .into_any_element(),
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
                                // Shift and cmd/ctrl resolve on press.
                                this.select(ix, event.modifiers, cx);
                            } else if !this.selected.contains(&id) {
                                // A plain press on an unselected row picks it
                                // now, so a drag from here includes it. A press
                                // on a lit row keeps the set for a whole-set
                                // drag; the collapse waits for the click.
                                this.select(ix, event.modifiers, cx);
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        // A plain click that never became a drag collapses a
                        // multi-selection down to the clicked row.
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
                    // The chevron column stays empty so songs align with
                    // their folder's children.
                    .child(div().flex_none().w(px(16.)))
                    .child(match cover {
                        Some(thumb) => track_columns::cover_cell(&Some(thumb))
                            .flex_none()
                            .into_any_element(),
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
                        // Catch up right away instead of waiting for the next
                        // track change.
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
        // Hide drops branches, Dim keeps them faint, so the row set changes:
        // recount rather than a plain repaint.
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("folder-tree-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The search box shares the title bar row, the playlists panel's spot.
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

    /// The panel body hands its right-click to the rows, so the track and
    /// folder menus are the only ones a click over the list opens.
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
        // The live expand set rides along in the config, sorted so the
        // saved layout doesn't churn with the set's iteration order.
        let mut config = self.config.clone();
        config.expanded = self.expanded.iter().cloned().collect();
        config.expanded.sort_unstable();
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
        // Checks on the right so the follow toggle keeps its icon; the
        // default left side would swap it out for the checkmark.
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
                .disabled(self.expanded.is_empty())
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
        // The cover-art knob as a flyout, so the toggle is on the menu the
        // same way it's on the settings page. Live checks through
        // follow_panel + check_row, not plain .checked(), so the tick moves
        // while the flyout stays open.
        let menu = menu.separator().label(rox_i18n::t!("panel-menu-display"));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            // The same four labels the settings row uses, so the two
            // spellings of one knob can't drift apart.
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
        // The Dim/Hide knobs, the same flyout shape, so the behavior toggles
        // are on the menu too: one for folders, one for songs.
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
        // A pending box reset (a source toggle or a shared-query change)
        // is applied here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        // The follow glide eases toward the playing row, stepped here in
        // render one frame at a time until it arrives, the library's idiom.
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
            // Scopes the workspace's space-bound playback binding out while
            // a type-ahead phrase is mid-flight, the same way the search
            // box's own context does: bindings win over key listeners, so
            // without this a space continuing a phrase would also toggle
            // playback before on_panel_key ever saw the keystroke.
            // While a phrase is up the panel carries its contexts, which
            // scope the workspace's space binding out (only while the
            // phrase is still taking keystrokes) and Root's tab traversal
            // out (for as long as there's a phrase to cycle).
            .when_some(
                panel::type_ahead_context(&self.type_ahead, self.type_ahead_at),
                |d, context| d.key_context(context),
            )
            // A press anywhere in the panel ends the phrase: the cursor
            // has moved by hand, so the cycle it was stepping is stale,
            // and tab belongs back with panel traversal. Capture phase,
            // so rows and tiles that stop the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            // Tab cycles the live phrase's matches, off the bindings the
            // TypeAhead context above scopes in; with no phrase up, tab
            // stays Root's focus traversal.
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_panel_key(event, cx)),
            )
            // Any scroll or press over the tree counts as browsing; the stamp
            // only restarts the idle clock, leaving the gesture to the row
            // handlers underneath, so nothing acts twice.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.touch_resume(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.touch_resume(cx)),
            );
        if self.visible.is_empty() {
            // A search that hit nothing reads differently from an empty tree.
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
            // A press anywhere in the body takes keyboard focus, so
            // type-ahead works without first clicking a row. It runs in
            // the capture phase, before any row's bubble handler records
            // itself, so a right press off the rows leaves no target and the
            // menu below falls back to the panel's own.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, _| {
                window.focus(&this.focus);
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }));
        // The row context menu: the track actions every song surface
        // shares (a folder row standing for its whole subtree), plus the
        // folder-scope filter, then the panel menu appended so a click
        // over the list never dead-ends at Play.
        let weak = cx.entity().downgrade();
        root.child(content.context_menu(move |menu, window, cx| {
            let Some(this) = weak.upgrade() else {
                return menu;
            };
            enum Target {
                Folder { path: String, scoped: bool },
                Track { id: i64, folder: String, pos: usize },
            }
            let target = {
                let panel = this.read(cx);
                panel.menu_row.and_then(|ix| {
                    panel.visible.get(ix).map(|row| match &row.kind {
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
                    // The branch fold, the menu's spelling of the
                    // alt-click: one entry per state, so the label says
                    // what the click will do.
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
                    // A right click inside a multi-selection acts on the whole
                    // set (the right-press already reselected a lone row), so
                    // the menu queues exactly what's lit.
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
