//! The Milkdrop preset browser: the preset library as a folder tree or a
//! thumbnail grid, with a filter box, a favorites switch, and a star on every
//! row. The Milkdrop panel's Presets page draws it inline, and the preset
//! picker window draws it as its whole body.
//!
//! The browser owns the browsing state and nothing else. The host supplies
//! the presets and the current one, and decides what a [`BrowserEvent::Pick`]
//! means. Stars are the one write the browser makes itself, straight to the
//! app-wide favorites list. [`PresetHost`] is the picker window's side, a
//! trait so the window can serve a panel it can't name.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Asset as _, Context, Div, Entity, EventEmitter, ImageAssetLoader, ImageCache,
    ImageCacheError, MouseButton, MouseDownEvent, ObjectFit, Pixels, RenderImage, Resource,
    ScrollStrategy, SharedString, Stateful, Subscription, UniformListScrollHandle, WeakEntity,
    Window, canvas, div, hash, image_cache, img, prelude::*, px, uniform_list,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, Sizable as _};

use rox_core::settings as core_settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::folders::{Node, build_roots, sum_counts};
use rox_panel_kit::ui as settings_ui;

/// The tests' row cap. The browser passes no bound, since its list only draws
/// the rows on screen.
#[cfg(test)]
const PRESET_ROWS: usize = 200;

/// Matches the folder tree panel's indent.
const PRESET_INDENT: f32 = 14.;

/// The folder tree panel's row height, folder or preset.
const ROW_H: Pixels = px(26.);

/// The narrowest a 16:9 grid cell gets before the row loses a column. Cells
/// share the measured width out between them.
const CELL_MIN_W: Pixels = px(136.);
const CELL_GAP: Pixels = px(8.);
/// The cell's own inset, and the name line under the thumbnail.
const CELL_PAD: Pixels = px(4.);
const CELL_LABEL_H: Pixels = px(18.);

#[derive(Clone, Copy, Debug, PartialEq)]
struct CellSize {
    columns: usize,
    w: Pixels,
    h: Pixels,
    thumb_h: Pixels,
}

impl CellSize {
    /// As many columns as fit at the minimum, widened to fill the box.
    fn fit(width: Pixels) -> CellSize {
        let gap = f32::from(CELL_GAP);
        let width = f32::from(width).max(f32::from(CELL_MIN_W));
        let columns = ((width + gap) / (f32::from(CELL_MIN_W) + gap))
            .floor()
            .max(1.0) as usize;
        let w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
        let thumb_w = w - f32::from(CELL_PAD) * 2.0;
        let thumb_h = (thumb_w * 9.0 / 16.0).round();
        let h = thumb_h + f32::from(CELL_PAD) * 3.0 + f32::from(CELL_LABEL_H);
        CellSize {
            columns,
            w: px(w.floor()),
            h: px(h),
            thumb_h: px(thumb_h),
        }
    }
}

/// Thumbnails are only asked for in the grid, so the list costs no rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Tree,
    Grid,
}

/// Polls only while thumbnails are pending.
const THUMB_POLL: Duration = Duration::from_millis(300);

/// Decoded thumbnails the grid keeps on the GPU. A window's own image cache
/// keeps everything it loaded, and scrolling a hundred-thousand-preset pack
/// through it runs the app out of texture memory.
const THUMB_CACHE: usize = 240;

/// The grid's image cache, evicting the least recently drawn.
struct ThumbCache {
    me: WeakEntity<ThumbCache>,
    loaded: HashMap<u64, Arc<RenderImage>>,
    used: HashMap<u64, u64>,
    tick: u64,
    /// Decoding or failed. Neither is asked for again, so a broken file
    /// doesn't decode on every frame.
    pending: HashSet<u64>,
}

impl ThumbCache {
    fn new(cx: &mut Context<Self>) -> Self {
        ThumbCache {
            me: cx.entity().downgrade(),
            loaded: HashMap::new(),
            used: HashMap::new(),
            tick: 0,
            pending: HashSet::new(),
        }
    }

    fn evict(&mut self, cx: &mut App) {
        while self.loaded.len() > THUMB_CACHE {
            let Some(oldest) = self
                .used
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.used.remove(&oldest);
            if let Some(image) = self.loaded.remove(&oldest) {
                cx.drop_image(image, None);
            }
        }
    }
}

impl ImageCache for ThumbCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let key = hash(resource);
        self.tick += 1;
        if let Some(image) = self.loaded.get(&key) {
            self.used.insert(key, self.tick);
            return Some(Ok(image.clone()));
        }
        if self.pending.contains(&key) {
            return None;
        }
        self.pending.insert(key);
        let view = window.current_view();
        let me = self.me.clone();
        let load = ImageAssetLoader::load(resource.clone(), cx);
        cx.spawn(async move |cx| {
            let result = load.await;
            me.update(cx, |cache, cx| {
                if let Ok(image) = result {
                    cache.pending.remove(&key);
                    cache.tick += 1;
                    cache.used.insert(key, cache.tick);
                    cache.loaded.insert(key, image);
                    cache.evict(cx);
                }
                // Notify the view that drew the slot; the cache has no
                // element of its own to repaint.
                App::notify(cx, view);
            })
            .ok();
        })
        .detach();
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Thumb {
    Ready(PathBuf),
    Pending,
    /// Shown as a mark with the reason on hover, never asked for again.
    Failed(String),
}

/// Implemented over the Milkdrop engine elsewhere, so this crate never names
/// a renderer.
pub trait Thumbnails: 'static {
    /// Cheap: a set lookup.
    fn thumb(&self, preset: &Path) -> Thumb;
    /// Which presets are on screen, in order. Replaces the last ask.
    fn want(&self, presets: Vec<PathBuf>);
    /// Moves when a thumbnail lands.
    fn generation(&self) -> u64;
    /// The preset ran on a real engine, so forget a failure held against it.
    fn loaded(&self, preset: &Path);
}

/// The file stem, the one place every pack writes a preset's name.
pub fn preset_label(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A folder's path under whichever root holds it, since the absolute path is
/// mostly the part every option shares. Falls back to the absolute path when
/// a root was edited out from under a saved pick.
pub fn folder_label(folder: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .filter_map(|root| folder.strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().into_owned())
        .filter(|relative| !relative.is_empty())
        // Overlapping roots can both match; the shortest strips the most.
        .min_by_key(String::len)
        .unwrap_or_else(|| folder.to_string_lossy().into_owned())
}

/// Forward slashes on every platform, so the value matches on whichever
/// machine the workspace lands on. None for a path under no root.
pub fn relative_to_roots(path: &Path, roots: &[PathBuf]) -> Option<String> {
    roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .map(|relative| {
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|relative| !relative.is_empty())
        .min_by_key(String::len)
}

/// The scanned folder a saved relative path names: the same path under
/// any root, or failing that a folder of the same name at any depth.
pub fn find_folder_by_relative(
    folders: &[PathBuf],
    roots: &[PathBuf],
    relative: &str,
) -> Option<PathBuf> {
    if let Some(exact) = folders
        .iter()
        .find(|folder| relative_to_roots(folder, roots).as_deref() == Some(relative))
    {
        return Some(exact.clone());
    }
    let name = relative.rsplit('/').next()?;
    folders
        .iter()
        .find(|folder| {
            folder
                .file_name()
                .is_some_and(|file| file.to_string_lossy() == name)
        })
        .cloned()
}

/// What the picker window needs from what it's picking for. The backdrop and
/// a Milkdrop panel both answer this, and the window serves both without
/// naming either.
pub trait PresetHost: 'static {
    /// The window header's name for the host.
    fn title(&self, cx: &App) -> SharedString;
    /// Asked on open and when the favorites or folder lists move, not per frame.
    fn presets(&self, cx: &mut App) -> Vec<PathBuf>;
    fn current(&self, cx: &App) -> Option<PathBuf>;
    /// The host writes whatever it keeps of the choice.
    fn pick(&self, path: PathBuf, cx: &mut App);
    fn random(&self, cx: &mut App);
    /// `wake` repaints for a change like the preset switching on its own;
    /// `gone` closes the window when the host goes away. The backdrop wakes
    /// every window itself and hooks nothing.
    fn watch(
        &self,
        wake: Rc<dyn Fn(&mut App)>,
        gone: Rc<dyn Fn(&mut App)>,
        cx: &mut App,
    ) -> Vec<Subscription> {
        let _ = (wake, gone, cx);
        Vec::new()
    }
}

pub struct PresetEntry {
    pub path: PathBuf,
    pub label: SharedString,
    /// Folded once, so a keystroke is substring searches over borrowed
    /// strings with no allocation.
    folded: String,
}

/// The preset list as a tree, built on a rescan rather than per render,
/// since the page rebuilds on every keystroke. The hierarchy is
/// [`rox_library::folders`], the trie the folder tree panel uses.
#[derive(Default)]
pub struct PresetTree {
    pub entries: Vec<PresetEntry>,
    /// Mutated in place by [`sum_counts`] on every query; the folder rows
    /// read those counts.
    pub roots: Vec<Node>,
    by_folder: HashMap<String, Vec<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PresetRow {
    Folder {
        path: String,
        label: SharedString,
        /// The whole subtree count when nothing's typed.
        matched: u32,
        depth: usize,
        open: bool,
        has_children: bool,
    },
    Preset {
        /// Index into [`PresetTree::entries`].
        entry: usize,
        depth: usize,
    },
}

/// `matched` counts the whole library, `shown` the matches that got a row.
#[derive(Debug, PartialEq)]
pub struct PresetRows {
    pub rows: Vec<PresetRow>,
    pub matched: usize,
    pub shown: usize,
}

impl PresetTree {
    pub fn build(presets: &[PathBuf]) -> PresetTree {
        let mut entries = Vec::with_capacity(presets.len());
        let mut by_folder: HashMap<String, Vec<usize>> = HashMap::new();
        let mut folders: Vec<String> = Vec::new();
        for path in presets {
            let label = preset_label(path);
            let folder = path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default();
            let index = entries.len();
            let list = by_folder.entry(folder.clone()).or_insert_with(|| {
                folders.push(folder.clone());
                Vec::new()
            });
            list.push(index);
            entries.push(PresetEntry {
                path: path.clone(),
                folded: label.to_lowercase(),
                label: SharedString::from(label),
            });
        }
        PresetTree {
            roots: build_roots(&folders),
            entries,
            by_folder,
        }
    }

    /// `query` arrives trimmed and case-folded, and empty matches everything.
    /// `only` narrows to the favorites. A query or the favorites switch opens
    /// every folder left standing, whatever the expand set says: searching a
    /// collapsed tree and getting folder names back is the complaint.
    pub fn rows(
        &mut self,
        query: &str,
        expanded: &HashSet<String>,
        only: Option<&HashSet<PathBuf>>,
        cap: usize,
    ) -> PresetRows {
        let searching = !query.is_empty() || only.is_some();
        let (matched_total, hits) = self.count(query, only);
        let roots = &self.roots;

        struct Walk<'a> {
            hits: &'a HashMap<String, Vec<usize>>,
            expanded: &'a HashSet<String>,
            searching: bool,
            cap: usize,
            shown: usize,
            rows: Vec<PresetRow>,
        }
        impl Walk<'_> {
            fn folder(&mut self, node: &Node, depth: usize) {
                // Drop a branch with no matches, or once the cap is full.
                if node.matched == 0 || self.shown >= self.cap {
                    return;
                }
                let own = self.hits.get(node.path.as_str());
                let open = self.searching || self.expanded.contains(&node.path);
                self.rows.push(PresetRow::Folder {
                    path: node.path.clone(),
                    label: SharedString::from(node.label.clone()),
                    matched: node.matched,
                    depth,
                    open,
                    has_children: !node.children.is_empty()
                        || own.is_some_and(|list| !list.is_empty()),
                });
                if !open {
                    return;
                }
                for child in &node.children {
                    self.folder(child, depth + 1);
                }
                for &entry in own.into_iter().flatten() {
                    if self.shown >= self.cap {
                        return;
                    }
                    self.shown += 1;
                    self.rows.push(PresetRow::Preset {
                        entry,
                        depth: depth + 1,
                    });
                }
            }
        }

        let mut walk = Walk {
            hits: &hits,
            expanded,
            searching,
            cap,
            shown: 0,
            rows: Vec::new(),
        };
        for root in roots.iter() {
            walk.folder(root, 0);
        }
        PresetRows {
            rows: walk.rows,
            matched: matched_total,
            shown: walk.shown,
        }
    }

    /// Every preset a query keeps, flat, for the grid. Returns the capped hits
    /// and the whole count.
    pub fn flat(
        &self,
        query: &str,
        only: Option<&HashSet<PathBuf>>,
        cap: usize,
    ) -> (Vec<usize>, usize) {
        let mut hits = Vec::new();
        let mut matched = 0usize;
        for (index, entry) in self.entries.iter().enumerate() {
            let keep = (query.is_empty() || entry.folded.contains(query))
                && only.is_none_or(|only| only.contains(&entry.path));
            if !keep {
                continue;
            }
            matched += 1;
            if hits.len() < cap {
                hits.push(index);
            }
        }
        (hits, matched)
    }

    /// Test every preset and fold the counts up the folders. After this, each
    /// node's `matched` is current.
    fn count(
        &mut self,
        query: &str,
        only: Option<&HashSet<PathBuf>>,
    ) -> (usize, HashMap<String, Vec<usize>>) {
        let PresetTree {
            entries,
            roots,
            by_folder,
        } = self;
        let mut matched_total = 0usize;
        let mut counts: HashMap<&str, (u32, u32)> = HashMap::with_capacity(by_folder.len());
        // Per-folder so a walk slices a folder's list without re-testing.
        let mut hits: HashMap<String, Vec<usize>> = HashMap::with_capacity(by_folder.len());
        for (folder, list) in by_folder.iter() {
            let matching: Vec<usize> = list
                .iter()
                .copied()
                .filter(|&index| query.is_empty() || entries[index].folded.contains(query))
                .filter(|&index| only.is_none_or(|only| only.contains(&entries[index].path)))
                .collect();
            matched_total += matching.len();
            counts.insert(folder.as_str(), (list.len() as u32, matching.len() as u32));
            hits.insert(folder.clone(), matching);
        }
        for root in roots.iter_mut() {
            sum_counts(root, &counts);
        }
        (matched_total, hits)
    }

    fn node(&self, path: &str) -> Option<&Node> {
        fn find<'a>(node: &'a Node, path: &str) -> Option<&'a Node> {
            if node.path == path {
                return Some(node);
            }
            let under = path
                .strip_prefix(node.path.as_str())
                .is_some_and(|rest| rest.starts_with(['/', '\\']));
            if !under {
                return None;
            }
            node.children.iter().find_map(|child| find(child, path))
        }
        self.roots.iter().find_map(|root| find(root, path))
    }

    /// A folder's child folders and its own presets. None is the top, whose
    /// folders are the roots.
    fn contents(&self, folder: Option<&str>) -> (Vec<&Node>, Vec<usize>) {
        match folder {
            None => (self.roots.iter().collect(), Vec::new()),
            Some(path) => match self.node(path) {
                Some(node) => (
                    node.children.iter().collect(),
                    self.by_folder.get(path).cloned().unwrap_or_default(),
                ),
                None => (Vec::new(), Vec::new()),
            },
        }
    }

    /// Every node on the way down to `folder`, for opening the tree onto one
    /// preset.
    fn ancestors(&self, folder: &str) -> Vec<String> {
        fn walk(node: &Node, folder: &str, out: &mut Vec<String>) {
            let under = folder == node.path
                || folder
                    .strip_prefix(node.path.as_str())
                    .is_some_and(|rest| rest.starts_with(['/', '\\']));
            if !under {
                return;
            }
            out.push(node.path.clone());
            for child in &node.children {
                walk(child, folder, out);
            }
        }
        let mut out = Vec::new();
        for root in &self.roots {
            walk(root, folder, &mut out);
        }
        out
    }
}

pub enum BrowserEvent {
    /// A row was clicked: put this preset up.
    Pick(PathBuf),
    /// For a host that brings the switches back on the next open.
    Switched { favorites_only: bool, nested: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cell {
    Folder {
        path: String,
        label: SharedString,
        count: u32,
    },
    Preset(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuTarget {
    Preset(PathBuf),
    Folder(String),
}

/// A render that changed none of this keeps the last walk instead of redoing
/// a hundred thousand rows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WalkKey {
    query: String,
    expanded_gen: u64,
    favorites_only: bool,
    lists_gen: u64,
    nested: bool,
    view: View,
    folder: Option<String>,
    presets_gen: u64,
}

/// The list is a `uniform_list` with the folder tree panel's mechanics. In
/// the grid each item is a row of cells at one height, which lets a hundred
/// thousand presets scroll like a list.
pub struct PresetBrowser {
    /// For a host that lays the browser out on its own page. None fills the
    /// host's box.
    height: Option<Pixels>,
    view: View,
    /// In the list, the tree against one flat run. In the grid, stepping
    /// through folders against every preset at once.
    nested: bool,
    /// The nested grid's folder by node path. None is the top.
    folder: Option<String>,
    /// From the last frame's layout. None until one has run, when the
    /// window's width stands in.
    width: Option<Pixels>,
    /// The grid cell for that width. The arrows step by its columns.
    cell: CellSize,
    /// Kept so a hand-in that changed nothing doesn't rebuild the tree.
    presets: Vec<PathBuf>,
    presets_gen: u64,
    tree: PresetTree,
    expanded: HashSet<String>,
    expanded_gen: u64,
    /// Whether the top folders were opened once, so the tree lands on the
    /// pack's categories. After that the set is the user's.
    seeded: bool,
    favorites_only: bool,
    current: Option<PathBuf>,
    /// Bring the current preset into view on the next render. Never set by the
    /// preset merely changing: the rotation moves every half minute, and a
    /// browser that jumped each time would be impossible to browse.
    reveal: bool,
    /// The first current preset the host hands in is where the browser opens.
    shown_once: bool,
    filter: Entity<InputState>,
    _filter_events: Subscription,
    walk: Option<WalkKey>,
    favorites: HashSet<PathBuf>,
    matched: usize,
    listed: Vec<PresetRow>,
    /// The grid's cells, `cell.columns` per item.
    cells: Vec<Cell>,
    /// None until a key or a reveal lands on one; the mouse never sets it.
    cursor: Option<usize>,
    /// None off the rows, where the menu has nothing to act on.
    menu_target: Option<MenuTarget>,
    /// None draws the cells without thumbnails.
    thumbs: Option<Arc<dyn Thumbnails>>,
    /// So the poll only repaints when something landed.
    thumbs_gen: u64,
    cache: Entity<ThumbCache>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<BrowserEvent> for PresetBrowser {}

impl PresetBrowser {
    pub fn new(height: Option<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rox_i18n::t!("milkdrop-filter-presets"))
        });
        let _filter_events = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        PresetBrowser {
            height,
            view: View::Grid,
            nested: true,
            folder: None,
            width: None,
            cell: CellSize::fit(CELL_MIN_W),
            presets: Vec::new(),
            presets_gen: 0,
            tree: PresetTree::default(),
            expanded: HashSet::new(),
            expanded_gen: 0,
            seeded: false,
            favorites_only: false,
            current: None,
            reveal: false,
            shown_once: false,
            filter,
            _filter_events,
            walk: None,
            favorites: HashSet::new(),
            matched: 0,
            listed: Vec::new(),
            cells: Vec::new(),
            cursor: None,
            menu_target: None,
            thumbs: None,
            thumbs_gen: 0,
            cache: cx.new(ThumbCache::new),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Starts the poll that repaints the cells as thumbnails land. It costs
    /// one atomic read per tick when nothing's pending.
    pub fn set_thumbs(&mut self, thumbs: Arc<dyn Thumbnails>, cx: &mut Context<Self>) {
        self.thumbs_gen = thumbs.generation();
        self.thumbs = Some(thumbs);
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(THUMB_POLL).await;
                let alive = view.update(cx, |this, cx| {
                    if let Some(thumbs) = this.thumbs.as_ref() {
                        let generation = thumbs.generation();
                        if generation != this.thumbs_gen {
                            this.thumbs_gen = generation;
                            cx.notify();
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// A list that matches the last one is a no-op, so a host can hand it in
    /// on every render.
    pub fn set_presets(&mut self, presets: &[PathBuf], cx: &mut Context<Self>) {
        if self.presets.as_slice() == presets {
            return;
        }
        // Only the first list reveals the current preset. A later one is no
        // reason to leave where the user is.
        let first = self.presets.is_empty();
        self.presets = presets.to_vec();
        self.presets_gen += 1;
        self.tree = PresetTree::build(presets);
        if !self.seeded && !self.tree.roots.is_empty() {
            self.seeded = true;
            self.expanded
                .extend(self.tree.roots.iter().map(|root| root.path.clone()));
            self.expanded_gen += 1;
        }
        if self
            .folder
            .as_deref()
            .is_some_and(|path| self.tree.node(path).is_none())
        {
            self.folder = None;
        }
        if first {
            self.reveal = true;
        }
        cx.notify();
    }

    /// The highlight follows it. The view only goes to it the first time and
    /// on Go to Active.
    pub fn set_current(&mut self, current: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.current == current {
            return;
        }
        self.current = current;
        if !self.shown_once && self.current.is_some() {
            self.shown_once = true;
            self.reveal = true;
        }
        cx.notify();
    }

    pub fn favorites_only(&self) -> bool {
        self.favorites_only
    }

    pub fn set_favorites_only(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.favorites_only != on {
            self.favorites_only = on;
            self.switched(cx);
            cx.notify();
        }
    }

    pub fn view(&self) -> View {
        self.view
    }

    /// Leaving the grid empties the thumbnail queue, so nothing renders for a
    /// list nobody's looking at.
    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view == view {
            return;
        }
        self.view = view;
        if view == View::Tree
            && let Some(thumbs) = self.thumbs.as_ref()
        {
            thumbs.want(Vec::new());
        }
        self.reveal = true;
        cx.notify();
    }

    pub fn nested(&self) -> bool {
        self.nested
    }

    pub fn set_nested(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.nested != on {
            self.nested = on;
            self.reveal = true;
            self.switched(cx);
            cx.notify();
        }
    }

    fn switched(&self, cx: &mut Context<Self>) {
        cx.emit(BrowserEvent::Switched {
            favorites_only: self.favorites_only,
            nested: self.nested,
        });
    }

    pub fn total(&self) -> usize {
        self.tree.entries.len()
    }

    fn enter_folder(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        if self.folder != folder {
            self.folder = folder;
            self.cursor = None;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn toggle_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.expanded_gen += 1;
        cx.notify();
    }

    /// Writes the app-wide list; every host follows its generation.
    fn toggle_favorite(&mut self, path: &Path, cx: &mut Context<Self>) {
        let on = !core_settings::is_milkdrop_favorite(path);
        core_settings::set_milkdrop_favorite(path, on);
        cx.notify();
    }

    fn len(&self) -> usize {
        match self.view {
            View::Tree => self.listed.len(),
            View::Grid => self.cells.len(),
        }
    }

    /// The list item holding a cursor position: a row of cells in the grid.
    fn item_of(&self, index: usize) -> usize {
        match self.view {
            View::Tree => index,
            View::Grid => index / self.cell.columns.max(1),
        }
    }

    fn preset_at(&self, index: usize) -> Option<&PathBuf> {
        let entry = match self.view {
            View::Tree => match self.listed.get(index)? {
                PresetRow::Preset { entry, .. } => *entry,
                PresetRow::Folder { .. } => return None,
            },
            View::Grid => match self.cells.get(index)? {
                Cell::Preset(entry) => *entry,
                Cell::Folder { .. } => return None,
            },
        };
        Some(&self.tree.entries[entry].path)
    }

    /// In the grid a step is a row of cells. A preset the cursor lands on
    /// goes up straight away, so the arrows flip through presets.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let delta = match self.view {
            View::Tree => delta,
            View::Grid => delta * self.cell.columns.max(1) as isize,
        };
        let from = match self.cursor {
            Some(at) => at as isize,
            // The first press starts from the preset that's up.
            None => self.current_index().map(|at| at as isize).unwrap_or(-1),
        };
        let next = (from + delta).clamp(0, len as isize - 1) as usize;
        if self.cursor == Some(next) {
            return;
        }
        self.cursor = Some(next);
        self.scroll
            .scroll_to_item(self.item_of(next), ScrollStrategy::Top);
        if let Some(path) = self.preset_at(next).cloned() {
            cx.emit(BrowserEvent::Pick(path));
        }
        cx.notify();
    }

    fn enter(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.cursor else {
            return;
        };
        if let Some(path) = self.preset_at(at).cloned() {
            cx.emit(BrowserEvent::Pick(path));
            return;
        }
        match self.view {
            View::Tree => {
                if let Some(PresetRow::Folder { path, .. }) = self.listed.get(at) {
                    let path = path.clone();
                    self.toggle_folder(path, cx);
                }
            }
            View::Grid => {
                if let Some(Cell::Folder { path, .. }) = self.cells.get(at) {
                    let path = path.clone();
                    self.enter_folder(Some(path), cx);
                }
            }
        }
    }

    fn current_index(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        (0..self.len()).find(|&index| self.preset_at(index) == Some(current))
    }

    /// Unfold the tree to the current preset, or step the grid into its folder.
    fn open_to_current(&mut self) {
        let Some(folder) = self
            .current
            .as_deref()
            .and_then(Path::parent)
            .map(|parent| parent.to_string_lossy().into_owned())
        else {
            return;
        };
        match self.view {
            View::Tree => {
                let before = self.expanded.len();
                self.expanded.extend(self.tree.ancestors(&folder));
                if self.expanded.len() != before {
                    self.expanded_gen += 1;
                }
            }
            View::Grid => {
                if self.tree.node(&folder).is_some() {
                    self.folder = Some(folder);
                }
            }
        }
    }

    /// The Go to Active button.
    fn locate(&mut self, cx: &mut Context<Self>) {
        self.reveal = true;
        cx.notify();
    }

    /// A change repaints, since the layout that measured it is already drawn.
    fn measured(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let moved = self
            .width
            .is_none_or(|had| (f32::from(had) - f32::from(width)).abs() > 0.5);
        if moved {
            self.width = Some(width);
            cx.notify();
        }
    }

    /// A canvas that reads its own bounds at layout and hands the width back
    /// deferred, since the view can't be touched inside its own frame.
    fn ruler(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let width = bounds.size.width;
                cx.defer(move |cx| {
                    view.update(cx, |this, cx| this.measured(width, cx)).ok();
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// A walk that's still good is kept, scroll position and all.
    fn rewalk(&mut self, query: &str) {
        let key = WalkKey {
            query: query.to_string(),
            expanded_gen: self.expanded_gen,
            favorites_only: self.favorites_only,
            lists_gen: core_settings::milkdrop_gen(),
            nested: self.nested,
            view: self.view,
            folder: self.folder.clone(),
            presets_gen: self.presets_gen,
        };
        if self.walk.as_ref() == Some(&key) {
            return;
        }
        self.favorites = core_settings::milkdrop_favorites().into_iter().collect();
        let only = self.favorites_only.then_some(&self.favorites);
        // Matching the file stem, not the path: a pack's folder name is in
        // every one of its paths, so a query would match the whole pack.
        match self.view {
            View::Tree => {
                let listed = if self.nested {
                    self.tree.rows(query, &self.expanded, only, usize::MAX)
                } else {
                    let (hits, matched) = self.tree.flat(query, only, usize::MAX);
                    PresetRows {
                        shown: hits.len(),
                        rows: hits
                            .into_iter()
                            .map(|entry| PresetRow::Preset { entry, depth: 0 })
                            .collect(),
                        matched,
                    }
                };
                self.matched = listed.matched;
                self.listed = listed.rows;
                self.cells.clear();
            }
            View::Grid => {
                // A query searches the whole library whatever folder the grid
                // is in, the way a search opens the whole tree.
                if self.nested && query.is_empty() {
                    // Folder counts read the favorites switch, and an empty
                    // folder drops out, like the tree.
                    let (matched, hits) = self.tree.count(query, only);
                    self.matched = matched;
                    let (folders, _) = self.tree.contents(self.folder.as_deref());
                    let own: Vec<usize> = self
                        .folder
                        .as_deref()
                        .and_then(|path| hits.get(path).cloned())
                        .unwrap_or_default();
                    self.cells = folders
                        .into_iter()
                        .filter(|node| node.matched > 0)
                        .map(|node| Cell::Folder {
                            path: node.path.clone(),
                            label: SharedString::from(node.label.clone()),
                            count: node.matched,
                        })
                        .chain(own.into_iter().map(Cell::Preset))
                        .collect();
                } else {
                    let (hits, matched) = self.tree.flat(query, only, usize::MAX);
                    self.cells = hits.into_iter().map(Cell::Preset).collect();
                    self.matched = matched;
                }
                self.listed.clear();
            }
        }
        self.walk = Some(key);
        if self.cursor.is_some_and(|at| at >= self.len()) {
            self.cursor = None;
        }
    }

    fn row_base(
        id: SharedString,
        depth: usize,
        target: MenuTarget,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_target = Some(target.clone())),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .h(ROW_H)
            .pl(tokens::SPACE_SM + px(depth as f32 * PRESET_INDENT))
            .pr(tokens::SPACE_SM)
            .rounded(tokens::RADIUS)
            .text_xs()
            .hover(|row| row.bg(palette::bg_control()))
    }

    /// Takes the press before the click, so starring a preset doesn't also
    /// load it.
    fn star(&self, path: &Path, cx: &mut Context<Self>) -> Div {
        let starred = self.favorites.contains(path);
        let star = path.to_path_buf();
        div()
            .flex_none()
            .cursor_pointer()
            .child(
                Icon::default()
                    .path(if starred {
                        icons::STAR_FILLED
                    } else {
                        icons::STAR
                    })
                    .xsmall()
                    .text_color(if starred {
                        palette::accent()
                    } else {
                        palette::text_muted()
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_favorite(&star, cx);
                }),
            )
    }

    fn row(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.cursor == Some(index);
        match self.listed.get(index) {
            Some(PresetRow::Folder {
                path,
                label,
                matched,
                depth,
                open,
                has_children,
            }) => {
                let toggle = path.clone();
                Self::row_base(
                    SharedString::from(format!("milkdrop-folder:{path}")),
                    *depth,
                    MenuTarget::Folder(path.clone()),
                    cx,
                )
                .when(selected, |row| row.bg(palette::bg_menu_hover()))
                .child(
                    Icon::default()
                        .path(if *open && *has_children {
                            icons::CHEVRON_DOWN
                        } else {
                            icons::CHEVRON_RIGHT
                        })
                        .xsmall()
                        .text_color(palette::text_muted()),
                )
                .child(div().min_w_0().child(label.clone()))
                .child(
                    div()
                        .text_color(palette::text_muted())
                        .child(matched.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(toggle.clone(), cx)))
            }
            Some(PresetRow::Preset { entry, depth }) => {
                let entry = &self.tree.entries[*entry];
                let picked = self.current.as_ref() == Some(&entry.path);
                let target = entry.path.clone();
                Self::row_base(
                    SharedString::from(format!("milkdrop-preset:{}", entry.path.display())),
                    *depth,
                    MenuTarget::Preset(entry.path.clone()),
                    cx,
                )
                .when(picked, |row| {
                    row.bg(palette::bg_control()).text_color(palette::text())
                })
                .when(selected, |row| row.bg(palette::bg_menu_hover()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(entry.label.clone()),
                )
                .child(self.star(&entry.path, cx))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(BrowserEvent::Pick(target.clone()));
                }))
            }
            None => div().id(("milkdrop-row-gone", index)).h(ROW_H),
        }
    }

    fn thumb(&self, index: usize, preset: &Path) -> Stateful<Div> {
        let slot = div()
            .id(("milkdrop-thumb", index))
            .flex_none()
            .w_full()
            .h(self.cell.thumb_h)
            .rounded(tokens::RADIUS)
            .overflow_hidden()
            .bg(palette::bg_control());
        match self.thumbs.as_ref().map(|thumbs| thumbs.thumb(preset)) {
            Some(Thumb::Ready(path)) => slot.child(
                img(path)
                    .w_full()
                    .h(self.cell.thumb_h)
                    .object_fit(ObjectFit::Cover),
            ),
            Some(Thumb::Failed(message)) => {
                let message = SharedString::from(message);
                slot.flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::default()
                            .path(icons::ALERT)
                            .small()
                            .text_color(palette::text_faint()),
                    )
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx))
            }
            Some(Thumb::Pending) | None => slot,
        }
    }

    fn cell_base(
        &self,
        id: SharedString,
        selected: bool,
        target: MenuTarget,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_target = Some(target.clone())),
            )
            .flex()
            .flex_col()
            .flex_none()
            .w(self.cell.w)
            .h(self.cell.h)
            .p(CELL_PAD)
            .gap(CELL_PAD)
            .rounded(tokens::RADIUS)
            .text_xs()
            .when(selected, |cell| cell.bg(palette::bg_menu_hover()))
            .hover(|cell| cell.bg(palette::bg_menu_hover()))
    }

    fn grid_cell(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.cursor == Some(index);
        match self.cells.get(index) {
            Some(Cell::Folder { path, label, count }) => {
                let target = path.clone();
                self.cell_base(
                    SharedString::from(format!("milkdrop-cell-folder:{path}")),
                    selected,
                    MenuTarget::Folder(path.clone()),
                    cx,
                )
                .cursor_pointer()
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(self.cell.thumb_h)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_control())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::default()
                                .path(icons::FOLDER)
                                .text_color(palette::text_muted()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .child(
                            div()
                                .flex_none()
                                .text_color(palette::text_muted())
                                .child(count.to_string()),
                        ),
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.enter_folder(Some(target.clone()), cx)),
                )
            }
            Some(Cell::Preset(entry)) => {
                let entry = &self.tree.entries[*entry];
                let picked = self.current.as_ref() == Some(&entry.path);
                let target = entry.path.clone();
                self.cell_base(
                    SharedString::from(format!("milkdrop-cell:{}", entry.path.display())),
                    selected,
                    MenuTarget::Preset(entry.path.clone()),
                    cx,
                )
                .when(picked, |cell| cell.bg(palette::bg_control()))
                .child(
                    div()
                        .rounded(tokens::RADIUS)
                        .when(picked, |slot| {
                            slot.border_1().border_color(palette::accent())
                        })
                        .child(self.thumb(index, &entry.path)),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if picked {
                                    palette::text()
                                } else {
                                    palette::text_muted()
                                })
                                .child(entry.label.clone()),
                        )
                        .child(self.star(&entry.path, cx)),
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(BrowserEvent::Pick(target.clone()));
                }))
            }
            None => div()
                .id(("milkdrop-cell-gone", index))
                .w(self.cell.w)
                .h(self.cell.h),
        }
    }

    fn grid_row(&self, row: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let columns = self.cell.columns.max(1);
        let first = row * columns;
        let last = (first + columns).min(self.cells.len());
        let mut line = div()
            .id(("milkdrop-grid-row", row))
            .flex()
            .flex_row()
            .items_start()
            .gap(CELL_GAP)
            .pb(CELL_GAP)
            .h(self.cell.h + CELL_GAP);
        for index in first..last {
            line = line.child(self.grid_cell(index, cx));
        }
        line
    }

    /// Ask for the missing thumbnails of the grid rows in `range`, the ones
    /// about to draw.
    fn want_thumbs(&self, range: std::ops::Range<usize>) {
        let Some(thumbs) = self.thumbs.as_ref() else {
            return;
        };
        let columns = self.cell.columns.max(1);
        let cells = range.start * columns..(range.end * columns).min(self.cells.len());
        let wanted: Vec<PathBuf> = cells
            .filter_map(|index| match self.cells.get(index) {
                Some(Cell::Preset(entry)) => Some(&self.tree.entries[*entry].path),
                _ => None,
            })
            .filter(|path| thumbs.thumb(path) == Thumb::Pending)
            .cloned()
            .collect();
        thumbs.want(wanted);
    }

    /// The crumbs over the nested grid, each a step back out.
    fn crumbs(&self, cx: &mut Context<Self>) -> Div {
        let mut chain: Vec<(Option<String>, SharedString)> =
            vec![(None, rox_i18n::t!("milkdrop-presets"))];
        if let Some(folder) = self.folder.as_deref() {
            let mut ancestors = self.tree.ancestors(folder);
            ancestors.sort_by_key(String::len);
            chain.extend(ancestors.into_iter().filter_map(|path| {
                let label = SharedString::from(self.tree.node(&path)?.label.clone());
                Some((Some(path), label))
            }));
        }
        let last = chain.len() - 1;
        let mut row = div()
            .flex()
            .flex_row()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs();
        for (index, (target, label)) in chain.into_iter().enumerate() {
            let here = index == last;
            row = row.child(
                div()
                    .id(("milkdrop-crumb", index))
                    .px(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .text_color(if here {
                        palette::text()
                    } else {
                        palette::text_muted()
                    })
                    .when(!here, |crumb| {
                        crumb
                            .cursor_pointer()
                            .hover(|crumb| crumb.bg(palette::bg_control()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.enter_folder(target.clone(), cx)
                            }))
                    })
                    .child(label),
            );
            if !here {
                row = row.child(
                    Icon::default()
                        .path(icons::CHEVRON_RIGHT)
                        .xsmall()
                        .text_color(palette::text_faint()),
                );
            }
        }
        row
    }

    fn switch(
        &self,
        id: &'static str,
        label: SharedString,
        on: bool,
        set: fn(&mut Self, bool, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| set(this, !on, cx)),
            )
            .child(settings_ui::checkbox(on))
            .child(div().text_color(palette::text_muted()).child(label))
    }
}

impl Render for PresetBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().trim().to_lowercase();

        // Measured last frame. Until then, the window's width less the host's
        // inset stands in.
        let width = self.width.unwrap_or_else(|| {
            let inset = match self.height {
                None => tokens::SPACE_MD * 2.,
                Some(_) => settings_ui::SIDEBAR_W + tokens::SPACE_MD * 4.,
            };
            window.viewport_size().width - inset
        });
        self.cell = CellSize::fit(width);

        // Open the way before the walk so the current preset gets a row now.
        let reveal = std::mem::take(&mut self.reveal);
        if reveal && self.nested {
            self.open_to_current();
        }
        self.rewalk(&query);

        // Park the cursor on the revealed preset so the arrows carry on.
        if reveal && let Some(index) = self.current_index() {
            self.cursor = Some(index);
            self.scroll
                .scroll_to_item(self.item_of(index), ScrollStrategy::Center);
        }

        let count = rox_i18n::t!(
            "milkdrop-preset-count",
            count = self.matched.to_string(),
            total = self.tree.entries.len().to_string()
        );
        let crumbs =
            (self.view == View::Grid && self.nested && query.is_empty()).then(|| self.crumbs(cx));

        let head = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_XS)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.filter).small()),
                    )
                    .child(self.switch(
                        "milkdrop-browser-favorites",
                        rox_i18n::t!("milkdrop-favorites-only"),
                        self.favorites_only,
                        Self::set_favorites_only,
                        cx,
                    ))
                    .child(self.switch(
                        "milkdrop-browser-nested",
                        rox_i18n::t!("milkdrop-nested"),
                        self.nested,
                        Self::set_nested,
                        cx,
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("milkdrop-locate-current"),
                        icons::LOCATE,
                        self.current.is_none(),
                        cx.listener(|this, _, _, cx| this.locate(cx)),
                    ))
                    .child(crate::panel::choices_icons(
                        &[
                            (icons::LIST_MUSIC, View::Tree),
                            (icons::LAYOUT_GRID, View::Grid),
                        ],
                        self.view,
                        |this: &mut Self, view, cx| this.set_view(view, cx),
                        cx,
                    )),
            )
            .children(crumbs)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(count),
            );

        // The folder tree panel's shape: the ruler under the list and the
        // scrollbar over it. The grid asks for thumbnails for the range the
        // list is about to draw.
        let view = self.view;
        let columns = self.cell.columns.max(1);
        let items = match view {
            View::Tree => self.listed.len(),
            View::Grid => self.cells.len().div_ceil(columns),
        };
        let rows = uniform_list(
            "milkdrop-browser-rows",
            items,
            cx.processor(
                move |this, range: std::ops::Range<usize>, _, cx| match view {
                    View::Tree => range.map(|index| this.row(index, cx)).collect(),
                    View::Grid => {
                        this.want_thumbs(range.clone());
                        range.map(|row| this.grid_row(row, cx)).collect()
                    }
                },
            ),
        )
        .track_scroll(self.scroll.clone())
        .size_full();
        // The bounded cache, since the window's never lets go of an image.
        let rows = image_cache(self.cache.clone()).size_full().child(rows);
        let list = div()
            .id("milkdrop-browser-list")
            .w_full()
            .relative()
            .child(self.ruler(cx))
            .child(rows)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.scroll)),
            );
        let list = match self.height {
            None => list.flex_1().min_h_0(),
            Some(height) => list.h(height),
        };
        // A right press off the rows clears the target, so the menu stays empty.
        let list = list
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_target = None;
                }
            }))
            .context_menu({
                let weak = cx.entity().downgrade();
                move |menu, _, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    let Some(target) = this.read(cx).menu_target.clone() else {
                        return menu;
                    };
                    match target {
                        MenuTarget::Preset(path) => {
                            let starred = core_settings::is_milkdrop_favorite(&path);
                            let load = weak.clone();
                            let load_path = path.clone();
                            let star = weak.clone();
                            let star_path = path.clone();
                            menu.item(
                                PopupMenuItem::new(rox_i18n::t!("milkdrop-load-preset"))
                                    .icon(Icon::default().path(icons::PLAY))
                                    .on_click(move |_, _, cx| {
                                        load.update(cx, |_, cx| {
                                            cx.emit(BrowserEvent::Pick(load_path.clone()));
                                        })
                                        .ok();
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(rox_i18n::t!(if starred {
                                    "milkdrop-unfavorite"
                                } else {
                                    "milkdrop-favorite"
                                }))
                                .icon(Icon::default().path(if starred {
                                    icons::STAR_FILLED
                                } else {
                                    icons::STAR
                                }))
                                .on_click(move |_, _, cx| {
                                    star.update(cx, |this, cx| {
                                        this.toggle_favorite(&star_path, cx);
                                    })
                                    .ok();
                                }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new(rox_i18n::t!("milkdrop-reveal"))
                                    .icon(Icon::default().path(icons::EXTERNAL_LINK))
                                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                        }
                        MenuTarget::Folder(path) => menu.item(
                            PopupMenuItem::new(rox_i18n::t!("milkdrop-reveal-folder"))
                                .icon(Icon::default().path(icons::FOLDER))
                                .on_click(move |_, _, cx| cx.reveal_path(Path::new(&path))),
                        ),
                    }
                }
            });

        // The input only wires its arrow handlers on a multi-line box, so take
        // them on the way down to reach the list.
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .when(self.height.is_none(), |body| body.size_full())
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.step(-1, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.step(1, cx)))
            .capture_action(cx.listener(|this, _: &Enter, _, cx| this.enter(cx)))
            .child(head)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset_paths(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn folder_rows(rows: &[PresetRow]) -> Vec<(String, u32, usize, bool)> {
        rows.iter()
            .filter_map(|row| match row {
                PresetRow::Folder {
                    label,
                    matched,
                    depth,
                    open,
                    ..
                } => Some((label.to_string(), *matched, *depth, *open)),
                PresetRow::Preset { .. } => None,
            })
            .collect()
    }

    fn preset_labels(tree: &PresetTree, rows: &[PresetRow]) -> Vec<String> {
        rows.iter()
            .filter_map(|row| match row {
                PresetRow::Preset { entry, .. } => Some(tree.entries[*entry].label.to_string()),
                PresetRow::Folder { .. } => None,
            })
            .collect()
    }

    /// Each folder counts its whole subtree, not just the files directly in it.
    #[test]
    fn the_tree_groups_presets_under_their_folders() {
        let mut tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/Aderrasi.milk",
            "/packs/cream/Fractal/Geiss.milk",
            "/packs/cream/Waveform/Rovastar.milk",
        ]));

        let listed = tree.rows("", &HashSet::new(), None, PRESET_ROWS);
        // The chain above the pack collapses into one top row.
        assert_eq!(
            folder_rows(&listed.rows),
            vec![("cream".into(), 3, 0, false)]
        );
        assert!(
            preset_labels(&tree, &listed.rows).is_empty(),
            "nothing open"
        );
        assert_eq!(listed.matched, 3);
        assert_eq!(listed.shown, 0);

        let open: HashSet<String> = ["/packs/cream".to_string()].into_iter().collect();
        let listed = tree.rows("", &open, None, PRESET_ROWS);
        assert_eq!(
            folder_rows(&listed.rows),
            vec![
                ("cream".into(), 3, 0, true),
                ("Fractal".into(), 2, 1, false),
                ("Waveform".into(), 1, 1, false),
            ]
        );
    }

    /// A query reaches every preset, not just the ones an open folder shows.
    #[test]
    fn a_query_reaches_presets_inside_closed_folders() {
        let mut tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/Aderrasi - Spiral.milk",
            "/packs/cream/Fractal/Geiss - Bloom.milk",
            "/packs/cream/Waveform/Rovastar - Spiral Cage.milk",
        ]));

        let listed = tree.rows("spiral", &HashSet::new(), None, PRESET_ROWS);
        assert_eq!(listed.matched, 2);
        assert_eq!(listed.shown, 2);
        assert_eq!(
            preset_labels(&tree, &listed.rows),
            vec!["Aderrasi - Spiral", "Rovastar - Spiral Cage"]
        );
        assert_eq!(
            folder_rows(&listed.rows),
            vec![
                ("cream".into(), 2, 0, true),
                ("Fractal".into(), 1, 1, true),
                ("Waveform".into(), 1, 1, true),
            ],
            "the counts read the matches, and a search opens what it left"
        );

        // A folder with nothing left drops out rather than sitting at zero.
        let listed = tree.rows("bloom", &HashSet::new(), None, PRESET_ROWS);
        assert_eq!(
            folder_rows(&listed.rows),
            vec![("cream".into(), 1, 0, true), ("Fractal".into(), 1, 1, true)]
        );

        // Labels are folded at index time, so "Bloom" matches the folded query.
        assert_eq!(
            tree.rows("bloom", &HashSet::new(), None, PRESET_ROWS)
                .matched,
            1,
            "Bloom was indexed with a capital"
        );

        let listed = tree.rows("nothing here", &HashSet::new(), None, PRESET_ROWS);
        assert_eq!(listed.matched, 0);
        assert!(listed.rows.is_empty());
    }

    #[test]
    fn the_favorites_switch_keeps_only_the_starred_and_opens_to_them() {
        let mut tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/Aderrasi - Spiral.milk",
            "/packs/cream/Fractal/Geiss - Bloom.milk",
            "/packs/cream/Waveform/Rovastar - Spiral Cage.milk",
        ]));
        let starred: HashSet<PathBuf> =
            preset_paths(&["/packs/cream/Waveform/Rovastar - Spiral Cage.milk"])
                .into_iter()
                .collect();

        let listed = tree.rows("", &HashSet::new(), Some(&starred), PRESET_ROWS);
        assert_eq!(listed.matched, 1);
        assert_eq!(
            preset_labels(&tree, &listed.rows),
            vec!["Rovastar - Spiral Cage"]
        );
        assert_eq!(
            folder_rows(&listed.rows),
            vec![
                ("cream".into(), 1, 0, true),
                ("Waveform".into(), 1, 1, true)
            ]
        );

        let listed = tree.rows("bloom", &HashSet::new(), Some(&starred), PRESET_ROWS);
        assert_eq!(listed.matched, 0);
    }

    /// The cap limits rows drawn, never rows searched.
    #[test]
    fn the_row_cap_limits_the_drawing_and_not_the_search() {
        let paths: Vec<String> = (0..50)
            .map(|n| format!("/packs/cream/Fractal/preset {n:02}.milk"))
            .collect();
        let mut tree = PresetTree::build(&paths.iter().map(PathBuf::from).collect::<Vec<_>>());

        let listed = tree.rows("preset", &HashSet::new(), None, 10);
        assert_eq!(listed.matched, 50, "every preset was tested");
        assert_eq!(listed.shown, 10, "ten of them got a row");
        assert_eq!(preset_labels(&tree, &listed.rows).len(), 10);

        let listed = tree.rows("preset 0", &HashSet::new(), None, 10);
        assert_eq!(listed.matched, 10);
        assert_eq!(listed.shown, 10);
    }

    #[test]
    fn separate_roots_each_get_a_top_row() {
        let mut tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/one.milk",
            "/other/drive/pack/two.milk",
        ]));
        let listed = tree.rows("", &HashSet::new(), None, PRESET_ROWS);
        assert_eq!(
            folder_rows(&listed.rows),
            vec![
                ("Fractal".into(), 1, 0, false),
                ("pack".into(), 1, 0, false)
            ]
        );
        assert_eq!(listed.matched, 2);
    }

    #[test]
    fn the_way_down_to_a_preset_is_its_folder_chain() {
        let tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/Deep/Aderrasi.milk",
            "/packs/cream/Waveform/Rovastar.milk",
        ]));
        let mut chain = tree.ancestors("/packs/cream/Fractal/Deep");
        chain.sort();
        assert_eq!(
            chain,
            vec![
                "/packs/cream".to_string(),
                "/packs/cream/Fractal".to_string(),
                "/packs/cream/Fractal/Deep".to_string(),
            ]
        );
        // A sibling that merely shares a prefix string isn't on the way.
        assert!(tree.ancestors("/packs/creamy").is_empty());
    }

    #[test]
    fn a_folder_hands_the_grid_its_children_and_its_own_presets() {
        let tree = PresetTree::build(&preset_paths(&[
            "/packs/cream/Fractal/Deep/Aderrasi.milk",
            "/packs/cream/Fractal/Geiss.milk",
            "/packs/cream/Waveform/Rovastar.milk",
        ]));
        let (folders, presets) = tree.contents(None);
        assert_eq!(
            folders
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["cream"]
        );
        assert!(presets.is_empty());

        let (folders, presets) = tree.contents(Some("/packs/cream/Fractal"));
        assert_eq!(
            folders
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Deep"]
        );
        assert_eq!(
            presets
                .iter()
                .map(|&entry| tree.entries[entry].label.to_string())
                .collect::<Vec<_>>(),
            vec!["Geiss"]
        );

        let (folders, presets) = tree.contents(Some("/packs/nowhere"));
        assert!(folders.is_empty() && presets.is_empty());
    }

    #[test]
    fn a_preset_is_named_by_its_file_stem() {
        assert_eq!(
            preset_label(Path::new("/packs/cream/Geiss - Spiral Artifact.milk")),
            "Geiss - Spiral Artifact"
        );
        assert_eq!(preset_label(Path::new("/packs/cream/")), "cream");
    }
}
