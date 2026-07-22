//! The folder tree panel: the library's folder hierarchy as an
//! expand/collapse tree, reconstructed from the projection's interned
//! folder strings - never a walk of the filesystem. The shared prefix
//! above the music (the mount point, the home dir) collapses away, so the
//! top nodes are the folders where the library actually starts. Expanding
//! a folder shows its subfolders and then its songs; a double click plays
//! from there, and the right-click menu carries the track actions every
//! song surface shares plus the folder-scope filter, which narrows the
//! shared query to the folder's whole subtree with a single pick. The
//! shared text query and filter narrow the tree too: folders left with no
//! matching songs drop out.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{PathBuf, MAIN_SEPARATOR};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, ScrollStrategy, ScrollWheelEvent,
    SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::projection::FilterField;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ResumeIdle};
use crate::panel_settings;
use crate::panels::library::{fmt_ms, LibraryEvent, QUEUE_CAP};
use crate::query::shared_query::SharedQueryEvent;
use crate::track_ui::track_columns;
use crate::track_ui::track_drag::{PlayDrag, PlayDragPreview};

/// One row's height, the filter panel's, so the two read as one family.
const ROW_H: f32 = 26.;

/// How far each depth level steps in.
const INDENT: f32 = 14.;

/// The opacity a dimmed row (outside the active facet filter) draws at.
const DIM: f32 = 0.4;

/// How long a type-ahead phrase keeps growing before the next keystroke
/// starts a fresh jump; the filter panel's cadence.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

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
    /// Whether folder rows wear the album tile.
    fn on_folders(self) -> bool {
        matches!(self, CoverArt::Folders | CoverArt::Both)
    }

    /// Whether song rows wear their cover.
    fn on_songs(self) -> bool {
        matches!(self, CoverArt::Songs | CoverArt::Both)
    }
}

/// What the tree does with the folders and songs the active query leaves
/// out (text terms and facet picks both): dim them in place so the branch
/// still reads whole, or drop them so only the matches show. Folders and
/// songs carry their own choice, so the tree can hide the folders with no
/// match while still dimming the stray songs inside the folders that do.
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
    /// What happens to non-matching songs inside a folder that is shown.
    pub songs: FilterEffect,
    /// Reveal and scroll to the playing track whenever it changes.
    pub follow_playing: bool,
    /// Scroll back to the playing track after browsing stops.
    pub resume_playing: bool,
    /// Glide to the track instead of jumping.
    pub smooth_follow: bool,
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
        }
    }
}

/// One folder in the reconstructed hierarchy. The path is the exact
/// interned folder string, which is what the subtree filter pick matches
/// by prefix; the count is the context tracks in this subtree.
struct Node {
    label: SharedString,
    path: String,
    /// Every song in this subtree, whatever the query - the tree is the
    /// full hierarchy, so this never changes with a search.
    total: u32,
    /// Of those, how many pass the active query (text and facet). Equal to
    /// `total` when nothing is active; a subtree with zero here is what a
    /// filter dims or, in Hide mode, drops.
    matched: u32,
    children: Vec<Node>,
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
    /// The expanded folders by path. Survives rescans; top-level nodes
    /// seed in expanded once.
    expanded: HashSet<String>,
    seeded: bool,
    scroll: UniformListScrollHandle,
    /// The keyboard-and-click cursor, an index into `visible`: the lit
    /// row, where arrows move from and enter acts. None until a key or
    /// click lands one.
    cursor: Option<usize>,
    /// The selected songs by library id, the multi-select set the shared
    /// selection and a drag read from. Songs only; folders aren't selectable.
    selected: HashSet<i64>,
    /// The shift-range anchor, a `visible` index into the last plainly
    /// clicked song row.
    anchor: Option<usize>,
    /// The row under the last right press, what the context menu acts on;
    /// cleared when the press lands off the rows.
    menu_row: Option<usize>,
    /// The playing track's path and library id, the highlight's key, the
    /// history panel's follow.
    playing_path: Option<PathBuf>,
    playing: Option<i64>,
    /// Per-track paths resolved for drag payloads, so a hover frame never
    /// repeats the store lookup. Cleared on a library update.
    drag_paths: HashMap<i64, Option<PathBuf>>,
    /// Bumped whenever the selection or the visible order changes, keying the
    /// drag-set cache so a grab inside a big selection shares one Arc across
    /// every visible selected row instead of rebuilding the set per row.
    drag_gen: u64,
    drag_set: Option<(u64, Arc<[PathBuf]>)>,
    /// The idle clock behind resume: a browse gesture arms it, its wake
    /// scrolls back to the playing track once the panel sits untouched.
    resume_idle: ResumeIdle,
    /// The follow glide's target row and its per-frame clock, stepped in
    /// render like the library's; None when nothing is easing.
    glide_to: Option<usize>,
    glide_tick: Instant,
    /// The type-ahead phrase and when its last keystroke landed, so a
    /// quick run of letters jumps to a row by prefix.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
    _player_changed: Subscription,
}

impl FolderTreePanel {
    pub fn new(
        state: AppState,
        config: FolderTreeConfig,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The folder set changes when the library rescans; rebuild the
        // structure. Counts and the scope highlight follow the shared
        // query, our own scope writes included - recount is idempotent, so
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
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.recount(cx),
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let mut this = FolderTreePanel {
            state,
            config,
            focus: cx.focus_handle(),
            roots: Vec::new(),
            folder_tracks: HashMap::new(),
            dimmed_songs: HashSet::new(),
            visible: Vec::new(),
            expanded: HashSet::new(),
            seeded: false,
            scroll: UniformListScrollHandle::new(),
            cursor: None,
            selected: HashSet::new(),
            anchor: None,
            menu_row: None,
            playing_path: None,
            playing: None,
            drag_paths: HashMap::new(),
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
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Rebuild the hierarchy from the projection's folder set, then count.
    /// The structure only follows the library, never the query, so typing
    /// a search never restructures the branches - it only hides the empty
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
        self.drag_paths.clear();
        self.recount(cx);
    }

    /// Regroup the songs per folder and recount every subtree. The tree is
    /// the full library hierarchy; the active query - the text terms and
    /// every facet pick, the folder scope included - marks which songs
    /// match, and [`FilterEffect`] decides the rest: a folder with no match
    /// in its subtree hides or dims per `folders`, a non-matching song
    /// inside a shown folder hides or dims per `songs`. Then reflatten.
    fn recount(&mut self, cx: &mut Context<Self>) {
        {
            let song_hide = self.config.songs == FilterEffect::Hide;
            let (text, facet) = {
                let query = self.state.query.read(cx);
                (query.text().to_string(), query.filter().clone())
            };
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
                    label: node.label.clone(),
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
        // The label is the file's own name, so the tree mirrors the folder
        // on disk; a row with no resolvable path or an all-extension name
        // falls back to its title. The path resolves through the shared
        // cache, so covers and drags reuse it.
        let labels: HashMap<u32, (SharedString, i64)> = songs
            .into_iter()
            .map(|(row, id, title)| {
                let label = self
                    .path_for(id, cx)
                    .as_deref()
                    .and_then(|path| path.file_name())
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
        // The selection rides on ids, so it survives untouched.
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

    /// Follow the player: on a track change, resolve the playing path to
    /// its id, the history panel's move. The highlight matches track rows
    /// by that id.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        self.playing = self
            .playing_path
            .as_ref()
            .and_then(|path| self.state.library.read(cx).id_for(path));
        // Reveal and chase the new track when the follow is on; the move
        // notifies on its own.
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Open every branch from a root down to `path`, so the folder's row
    /// shows even if it or an ancestor sat collapsed. Walks the same prefix
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
    /// Hide keeps the track off the tree - there is no row to reach then. The
    /// shared step behind the menu jump and the automatic follow.
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

    /// The menu's jump: reveal the playing track and land the cursor on it,
    /// which selects it, publishes, and scrolls it into view in one move.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.reveal_playing(cx) {
            self.set_cursor(ix, cx);
        }
    }

    /// Reveal the playing track and scroll it into view: a glide when smooth
    /// is on, a jump otherwise. Scroll only, no cursor move - the deliberate
    /// jump owns the selection. Runs on a track change while follow is on and
    /// on the idle resume.
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

    /// The idle wake's landing: scroll back to the playing track, so long as
    /// the resume is still on. The clock only fires once the tree has sat
    /// untouched a full window, so no extra idle check is needed here.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.follow_playing(cx);
        }
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

    /// Scope the shared folder filter to one folder's subtree, or clear it
    /// if that folder is the scope already. One pick covers the branch -
    /// the filter matches folders by prefix - so this stays cheap at any
    /// depth.
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
    }

    /// A folder's whole subtree as projection rows, in the tree's order:
    /// each folder's subfolders first, then its own songs. What Play
    /// Folder queues and the folder context menu acts on.
    fn subtree_rows(&self, path: &str) -> Vec<u32> {
        fn collect(
            node: &Node,
            folder_tracks: &HashMap<String, Vec<u32>>,
            out: &mut Vec<u32>,
        ) {
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
    fn path_for(&mut self, id: i64, cx: &App) -> Option<PathBuf> {
        match self.drag_paths.get(&id) {
            Some(path) => path.clone(),
            None => {
                let path = self
                    .state
                    .library
                    .read(cx)
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                self.drag_paths.insert(id, path.clone());
                path
            }
        }
    }

    /// The cover thumbnail for a projection row's file, resolved through the
    /// path cache and the shared thumbnail service. None when the file has
    /// no path yet; a pending or missing cover rides through as a placeholder
    /// tile.
    fn cover_for(&mut self, row: u32, cx: &mut Context<Self>) -> Option<crate::thumbs::Thumb> {
        let id = self
            .state
            .library
            .read(cx)
            .projection()
            .map(|p| p.db_id[row as usize])?;
        let path = self.path_for(id, cx)?;
        track_columns::cover_thumb(&self.state, Some(path.as_path()), true, cx)
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
        let paths = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = rows.iter().map(|&r| projection.db_id[r as usize]).collect();
            let Ok(paths) = library.paths_for(&ids) else {
                return;
            };
            paths
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, start - lo, cx));
    }

    /// Queue an explicit set of library ids from the front, the multi-select
    /// menu's play. Order is the caller's (view order for a selection).
    fn play_ids(&mut self, ids: &[i64], cx: &mut Context<Self>) {
        let capped = &ids[..ids.len().min(QUEUE_CAP)];
        let Ok(paths) = self.state.library.read(cx).paths_for(capped) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_at(paths, 0, cx));
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
    /// Paths resolve through the shared cache, the library table's route
    /// into the play-drag story.
    fn song_drag(&mut self, ix: usize, title: &SharedString, cx: &App) -> Option<PlayDrag> {
        let id = self.song_id_at(ix)?;
        // A grab inside a multi-selection carries the whole set in visible order,
        // built once per selection or reflow and shared behind an Arc so it's a
        // refcount bump per row, not a rebuild. Outside it, just this song.
        let paths: Arc<[PathBuf]> = if self.selected.len() > 1 && self.selected.contains(&id) {
            if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.drag_gen) {
                let ids: Vec<i64> = self
                    .visible
                    .iter()
                    .filter_map(|row| match &row.kind {
                        RowKind::Track { id, .. } if self.selected.contains(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                let set: Arc<[PathBuf]> =
                    ids.iter().filter_map(|&id| self.path_for(id, cx)).collect();
                self.drag_set = Some((self.drag_gen, set));
            }
            self.drag_set.as_ref().map(|(_, set)| set.clone())?
        } else {
            self.path_for(id, cx).into_iter().collect()
        };
        if paths.is_empty() {
            return None;
        }
        Some(PlayDrag {
            paths,
            title: title.clone(),
        })
    }

    /// Browse from the keyboard while the panel is focused: up and down
    /// move the cursor, left and right fold folders, enter folds a folder
    /// or plays a song, and plain typing jumps to a row by prefix - the
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
            "up" => self.move_cursor(-1, cx),
            "down" => self.move_cursor(1, cx),
            "home" => self.set_cursor(0, cx),
            "end" => {
                let last = self.visible.len().saturating_sub(1);
                self.set_cursor(last, cx);
            }
            "left" => {
                if let Some(ix) = self.cursor {
                    if matches!(
                        self.visible.get(ix),
                        Some(Row {
                            kind: RowKind::Folder { expanded: true, .. },
                            ..
                        })
                    ) {
                        self.toggle_expand(ix, cx);
                    }
                }
            }
            "right" => {
                if let Some(ix) = self.cursor {
                    if matches!(
                        self.visible.get(ix),
                        Some(Row {
                            kind: RowKind::Folder {
                                expanded: false,
                                ..
                            },
                            ..
                        })
                    ) {
                        self.toggle_expand(ix, cx);
                    }
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
                if self.type_ahead.is_empty() && text == " " {
                    return;
                }
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead phrase and jump to its next match
    /// among the visible rows. A grown phrase re-tests the cursor's own
    /// row first so refining a match stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let now = Instant::now();
        let grown = self
            .type_ahead_at
            .is_some_and(|at| now.duration_since(at) < TYPE_AHEAD);
        if grown {
            self.type_ahead.push_str(&text);
        } else {
            self.type_ahead = text;
        }
        self.type_ahead_at = Some(now);
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
            .find(|&ix| self.visible[ix].label.to_lowercase().starts_with(&needle));
        if let Some(ix) = hit {
            self.set_cursor(ix, cx);
        }
    }

    /// Step the cursor; the first press with no cursor lands on the edge
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

    /// Select every song currently shown, the Ctrl+A move; the anchor lands
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
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
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
            // A selected song or the cursor row wears the accent wash.
            let lit = cursor == Some(ix)
                || row_song_id.is_some_and(|id| self.selected.contains(&id));
            let base = div()
                .id(("folder-tree-row", ix))
                .w_full()
                .h(px(ROW_H))
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
                        Some(thumb) => {
                            track_columns::cover_cell(&Some(thumb)).flex_none().into_any_element()
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
                            .child(SharedString::from(count.to_string())),
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
                                // now, so a drag from here carries it. A press
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
                        Some(thumb) => {
                            track_columns::cover_cell(&Some(thumb)).flex_none().into_any_element()
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
                "Cover Art",
                Some("Show album art in place of the row icon, on folders or songs"),
                panel::choices(
                    &[
                        ("None", CoverArt::None),
                        ("Folders", CoverArt::Folders),
                        ("Songs", CoverArt::Songs),
                        ("Both", CoverArt::Both),
                    ],
                    self.config.cover,
                    |this: &mut Self, cover, cx| this.set_cover(cover, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(crate::settings_ui::SECTION_GAP)
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    "Reveal and scroll to the playing track whenever it changes",
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
                    "Scroll back to the playing track after you stop browsing",
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    "Glide to the track instead of jumping",
                    |this: &mut Self, on, cx| {
                        this.config.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .child(crate::settings_ui::section(
                    "Filter",
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(panel::setting_row(
                            "Non-matching Folders",
                            Some("Hide the folders with no match, or keep them dim"),
                            panel::choices(
                                &[("Dim", FilterEffect::Dim), ("Hide", FilterEffect::Hide)],
                                self.config.folders,
                                |this: &mut Self, effect, cx| this.set_folders(effect, cx),
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            "Non-matching Songs",
                            Some("Inside a folder that matches, dim the stray songs or hide them"),
                            panel::choices(
                                &[("Dim", FilterEffect::Dim), ("Hide", FilterEffect::Hide)],
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
        panel::title_text(self.config.chrome.title.as_deref(), "Tree")
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

    /// The panel body hands its right-click to the rows, so the track and
    /// folder menus are the only ones a click over the list opens.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump carries the panel's config; the builder registered
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
            PopupMenuItem::new("Jump to Playing")
                .icon(Icon::default().path(icons::DISC))
                .disabled(self.playing.is_none())
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.jump_to_playing(cx));
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Follow Playing")
                .icon(Icon::default().path(icons::LOCATE))
                .checked(self.config.follow_playing)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.toggle_follow_playing(cx));
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Clear Folder Scope")
                .icon(Icon::default().path(icons::FUNNEL))
                .disabled(!scoped)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.clear_scope(cx));
                }),
        );
        // The cover-art knob as a flyout, so the toggle rides the menu the
        // same way it rides the settings page. Live checks through
        // follow_panel + check_row, not plain .checked(), so the tick moves
        // while the flyout stays open.
        let menu = menu.separator().label("Display");
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, cover) in [
                ("None", CoverArt::None),
                ("Folders", CoverArt::Folders),
                ("Songs", CoverArt::Songs),
                ("Both", CoverArt::Both),
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
        let menu = menu.item(PopupMenuItem::submenu("Cover Art", submenu));
        // The Dim/Hide knobs, the same flyout shape, so the behavior toggles
        // ride the menu too - one for folders, one for songs.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, effect) in [("Dim", FilterEffect::Dim), ("Hide", FilterEffect::Hide)] {
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
        let menu = menu.item(PopupMenuItem::submenu("Non-matching Folders", submenu));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (label, effect) in [("Dim", FilterEffect::Dim), ("Hide", FilterEffect::Hide)] {
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
        let menu = menu.item(PopupMenuItem::submenu("Non-matching Songs", submenu));
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let menu = panel::duplicate_item(menu, &cx.entity(), self.tab_panel.clone(), |this, window, cx| {
            let (state, config) = {
                let panel = this.read(cx);
                (panel.state.clone(), panel.config.clone())
            };
            FolderTreePanel::new(state, config, window, cx)
        });
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
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
        // The follow glide eases toward the playing row, stepped here in
        // render one frame at a time until it lands, the library's idiom.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(ix) = self.glide_to {
            let count = self.visible.len();
            match panel::glide_target(&self.scroll, ix, count) {
                Some(target) if !panel::glide_step(&self.scroll, target, dt) => self.glide_to = None,
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.on_panel_key(event, cx)
            }))
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
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child("No folders in the library yet"),
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
            // A press anywhere in the body takes keyboard focus, so
            // type-ahead works without first landing on a row. It lands in
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
        // shares - a folder row standing for its whole subtree - plus the
        // folder-scope filter, then the panel menu riding along so a click
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
                        "Play Folder",
                        window,
                        cx,
                        move |_, cx| {
                            let Some(this) = play_panel.upgrade() else { return };
                            this.update(cx, |this, cx| this.play_folder(&play_path.clone(), cx));
                        },
                    );
                    let scope_panel = weak.clone();
                    menu.separator().item(
                        PopupMenuItem::new(if scoped {
                            "Clear Folder Scope"
                        } else {
                            "Scope Filter to Folder"
                        })
                        .icon(Icon::default().path(icons::FUNNEL))
                        .on_click(move |_, _, cx| {
                            let Some(this) = scope_panel.upgrade() else { return };
                            this.update(cx, |this, cx| this.toggle_scope(path.clone(), cx));
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
                    let label = if selection.len() > 1 {
                        format!("Play {} Songs", selection.len())
                    } else {
                        "Play".to_string()
                    };
                    let play_panel = weak.clone();
                    let play_ids = selection.clone();
                    panel::track_actions(menu, state, selection, label, window, cx, move |_, cx| {
                        let Some(this) = play_panel.upgrade() else { return };
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

/// Reconstruct the folder hierarchy from the projection's folder strings.
/// Every path threads into a trie; the shared prefix above the first
/// branch or the first folder holding tracks collapses away, so the tree
/// starts where the library does instead of at the filesystem root. Node
/// paths slice the original strings, so a pick matches the interned
/// values exactly. Children sort case-insensitively; two top nodes that
/// collapse to the same name fall back to their full paths to stay apart.
fn build_roots(folders: &[String]) -> Vec<Node> {
    #[derive(Default)]
    struct Trie {
        /// Children keyed by path component, ordered for the walk.
        children: BTreeMap<String, Trie>,
        /// The full path down to this node, sliced from an inserted string.
        path: String,
        /// Whether this exact path is an interned folder - a directory
        /// holding tracks itself, not just the ancestor of one.
        has_tracks: bool,
    }

    // Bare filenames intern to the empty string; drop those.
    let mut root = Trie::default();
    for path in folders.iter().filter(|s| !s.is_empty()) {
        let mut node = &mut root;
        let mut start = 0;
        for (ix, _) in path
            .match_indices(MAIN_SEPARATOR)
            .chain([(path.len(), "")])
        {
            if ix > start {
                let component = path[start..ix].to_string();
                node = node.children.entry(component).or_default();
                if node.path.is_empty() {
                    node.path = path[..ix].to_string();
                }
            }
            start = ix + MAIN_SEPARATOR.len_utf8();
        }
        node.has_tracks = true;
    }

    fn node_from(trie: Trie) -> Node {
        let label = trie
            .path
            .rsplit(MAIN_SEPARATOR)
            .next()
            .unwrap_or(trie.path.as_str())
            .to_string();
        let mut children: Vec<Node> = trie.children.into_values().map(node_from).collect();
        children.sort_by(|a, b| {
            a.label
                .to_lowercase()
                .cmp(&b.label.to_lowercase())
                .then_with(|| a.label.cmp(&b.label))
        });
        Node {
            label: label.into(),
            path: trie.path,
            total: 0,
            matched: 0,
            children,
        }
    }

    let mut tops: Vec<Node> = root
        .children
        .into_values()
        .map(|mut trie| {
            // Collapse the chain of lone, trackless ancestors: /mnt/Zeal
            // holds nothing and branches nowhere, so the top node is Music.
            while !trie.has_tracks && trie.children.len() == 1 {
                trie = trie.children.into_values().next().unwrap();
            }
            node_from(trie)
        })
        .collect();
    tops.sort_by_key(|t| t.label.to_lowercase());
    let lower: Vec<String> = tops.iter().map(|t| t.label.to_lowercase()).collect();
    for ix in 0..tops.len() {
        let clash = lower
            .iter()
            .enumerate()
            .any(|(other, label)| other != ix && *label == lower[ix]);
        if clash {
            tops[ix].label = tops[ix].path.clone().into();
        }
    }
    tops
}

/// Compare two names the way a file manager lists them: runs of digits
/// compare as numbers, so "2" sorts before "10" and padded "02" reads the
/// same as "2", and the rest byte by byte. Inputs come lowercased.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    /// Drop leading zeros but keep one digit, so "007" compares as "7".
    fn magnitude(digits: &[u8]) -> &[u8] {
        let mut k = 0;
        while k + 1 < digits.len() && digits[k] == b'0' {
            k += 1;
        }
        &digits[k..]
    }

    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (da, db) = (magnitude(&a[si..i]), magnitude(&b[sj..j]));
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            // Same value; the shorter run (fewer leading zeros) reads first.
            let ord = (i - si).cmp(&(j - sj));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            let ord = a[i].cmp(&b[j]);
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

/// The node at an exact path, descending only into the branch whose path
/// prefixes the target so the walk stays O(depth), not O(nodes).
fn node_at<'a>(nodes: &'a [Node], path: &str) -> Option<&'a Node> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if path.starts_with(node.path.as_str())
            && path[node.path.len()..].starts_with(MAIN_SEPARATOR)
        {
            return node_at(&node.children, path);
        }
    }
    None
}

/// Fold the per-folder counts up the tree: each node's total and matched
/// count are its own folder's plus every descendant's.
fn sum_counts(node: &mut Node, by_path: &HashMap<&str, (u32, u32)>) -> (u32, u32) {
    let (mut total, mut matched) = by_path.get(node.path.as_str()).copied().unwrap_or((0, 0));
    for child in &mut node.children {
        let (t, m) = sum_counts(child, by_path);
        total += t;
        matched += m;
    }
    node.total = total;
    node.matched = matched;
    (total, matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    /// The tree starts where the library does: the lone, trackless chain
    /// above the first real folder collapses into the top node, and the
    /// nesting below reconstructs from the paths alone.
    #[test]
    fn collapses_shared_prefix_and_nests() {
        let roots = build_roots(&folders(&[
            "/mnt/Zeal/Music",
            "/mnt/Zeal/Music/Air - Moon Safari",
            "/mnt/Zeal/Music/Apocalyptica - Cult/CD1",
            "/mnt/Zeal/Music/Apocalyptica - Cult/CD2",
        ]));
        assert_eq!(roots.len(), 1);
        let top = &roots[0];
        assert_eq!(top.label.as_ref(), "Music");
        assert_eq!(top.path, "/mnt/Zeal/Music");
        let labels: Vec<&str> = top.children.iter().map(|c| c.label.as_ref()).collect();
        assert_eq!(labels, ["Air - Moon Safari", "Apocalyptica - Cult"]);
        // The multi-disc album nests its discs; the disc folders carry the
        // exact interned paths so a pick matches them.
        let cult = &top.children[1];
        assert_eq!(cult.children.len(), 2);
        assert_eq!(cult.children[0].path, "/mnt/Zeal/Music/Apocalyptica - Cult/CD1");
    }

    /// A folder with tracks stops the collapse even with a single child,
    /// so the top node never skips past real music.
    #[test]
    fn tracks_stop_the_collapse() {
        let roots = build_roots(&folders(["/a/b", "/a/b/c"].as_ref()));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, "/a/b");
        assert_eq!(roots[0].children.len(), 1);
    }

    /// Two libraries that collapse to the same folder name keep their full
    /// paths as labels so the top row stays unambiguous.
    #[test]
    fn clashing_top_labels_fall_back_to_paths() {
        let roots = build_roots(&folders(&[
            "/home/a/Music/X",
            "/home/a/Music/Y",
            "/mnt/media/Music/P",
            "/mnt/media/Music/Q",
            "/srv/Vinyl/Z1",
            "/srv/Vinyl/Z2",
        ]));
        let labels: Vec<&str> = roots.iter().map(|r| r.label.as_ref()).collect();
        assert_eq!(labels, ["/home/a/Music", "/mnt/media/Music", "Vinyl"]);
    }

    /// Filenames sort the file-manager way: digit runs compare as numbers,
    /// so padded and unpadded track numbers both land 1, 2, ... 10, 11 and
    /// never 1, 10, 11, 2.
    #[test]
    fn natural_sort_orders_track_numbers() {
        let mut names = vec![
            "10 moonbeam.mp3",
            "2 never ever.mp3",
            "1 lost.mp3",
            "12 emerald.mp3",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            ["1 lost.mp3", "2 never ever.mp3", "10 moonbeam.mp3", "12 emerald.mp3"]
        );
        // Zero-padding reads as the same value, so "02" and "2" tie on
        // magnitude and only the padding breaks it.
        assert_eq!(natural_cmp("02 x.mp3", "2 x.mp3"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("03 a.mp3", "10 a.mp3"), std::cmp::Ordering::Less);
    }

    /// Counts fold bottom-up: a parent's count is its own tracks plus
    /// every descendant's, folders outside the context at zero. The matched
    /// count folds the same way, so a branch with no facet match reads zero
    /// there while its total stays whole.
    #[test]
    fn counts_aggregate_subtrees() {
        let mut roots = build_roots(&folders(&["/m/Air", "/m/Air/Moon Safari", "/m/Empty"]));
        // (total, matched) per folder: the nested album has songs but none
        // match the active facet, so its matched count is zero.
        let by_path: HashMap<&str, (u32, u32)> =
            [("/m/Air", (2, 2)), ("/m/Air/Moon Safari", (10, 0))]
                .into_iter()
                .collect();
        for root in &mut roots {
            sum_counts(root, &by_path);
        }
        // The collapse stopped at the branch, so the top is /m itself.
        assert_eq!(roots[0].path, "/m");
        assert_eq!(roots[0].total, 12);
        assert_eq!(roots[0].matched, 2);
        let air = &roots[0].children[0];
        assert_eq!(air.total, 12);
        assert_eq!(air.matched, 2);
        assert_eq!(air.children[0].total, 10);
        // The nested album folds no matches, so Dim mode draws it faint.
        assert_eq!(air.children[0].matched, 0);
        assert_eq!(roots[0].children[1].total, 0);
    }
}
