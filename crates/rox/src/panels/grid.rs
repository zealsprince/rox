//! The album grid panel: the catalog as a wall of cover tiles, NekoRoX's
//! grid gallery. One tile per album in the library's canonical order,
//! square, the lanes splitting the panel's cross extent evenly so the wall
//! runs edge to edge, textures through the shared artwork service. It scrolls
//! vertically by default, rows filling the width, or horizontally by a
//! setting, columns filling the height.
//! Lines virtualize through a virtual_list, so a huge library costs only
//! the tiles on screen. Clicking a tile publishes the album's tracks on
//! the shared selection; a double click queues the album on the player.
//! A per-view query narrows the wall to albums containing a matching
//! track, so two duplicates scope to different filters. Deliberately not
//! the library's table: per the workspace rule, browsing surfaces are
//! panels of their own, never library view modes.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::rc::Rc;

use gpui::{
    canvas, div, img, prelude::*, px, size, svg, Along, AnyElement, App, Axis, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, ObjectFit, Pixels, ScrollStrategy, ScrollWheelEvent, SharedString, Size,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{h_virtual_list, v_virtual_list, Icon, Side, VirtualListScrollHandle};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, setting_row, toggle, AppState, FlickState, PanelChrome, PanelSettings, ResumeIdle,
    ScrubState,
};
use crate::panel_settings;
use crate::panels::library::{LibraryEvent, QUEUE_CAP};
use crate::query::search::{SearchBox, SearchEvent};
use crate::settings_ui;
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::thumbs::Thumb;

/// The tile size knob's range: how wide a tile wants to be, in px. The
/// actual edge divides the panel width evenly so the grid runs edge to
/// edge with no slack column. The top of the range stays at the stored
/// thumbnail's long side, so no setting upscales past what the store
/// keeps.
const TILE_MIN: f32 = 96.;
const TILE_MAX: f32 = 256.;

/// The tile rounding knob's ceiling, in percent of circular: 100 rounds
/// a square tile all the way into a circle.
const TILE_ROUNDING_MAX: f32 = 100.;

/// The tile gap knob's ceiling, the panel frame sliders' scale.
const TILE_GAP_MAX: f32 = 24.;

/// The dim knob's ceiling, in percent of fully hidden: 100 fades the
/// other covers out entirely.
const TILE_DIM_MAX: f32 = 100.;

/// The caption block's height under a cover while titles are on, in px:
/// two truncated text lines plus a little top gap. Fixed so the tile's
/// total extent stays predictable for the virtual list's item sizes.
const TILE_LABEL_H: f32 = 40.;

fn default_tile() -> f32 {
    192.
}

fn default_dim() -> f32 {
    60.
}

fn default_true() -> bool {
    true
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How the always-on caption lines up under its cover.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// The grid panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct GridConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub query: String,
    /// Show the search box; the query only applies while it shows. Off by
    /// default; the per-view filter is opt-in, not always on.
    #[serde(default)]
    pub search: bool,
    /// Whether this wall filters by its own query or follows the shared
    /// app-wide one. Shared by default; switch a duplicated grid to its own
    /// query for an independent filter.
    #[serde(default)]
    pub query_source: QuerySource,
    /// Scroll the wall vertically, rows filling the width; off scrolls it
    /// horizontally, columns filling the height. On by default, the wall's
    /// long-standing shape.
    #[serde(default = "default_true")]
    pub vertical: bool,
    /// The preferred tile edge in px, within [`TILE_MIN`]..[`TILE_MAX`].
    #[serde(default = "default_tile")]
    pub tile: f32,
    /// Scroll to the playing album when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// After the wall sits untouched for a spell, slide back to the playing
    /// album on its own. Off by default; a browse surface only chases the
    /// player once you ask it to.
    #[serde(default)]
    pub resume_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// While a track plays, fade every cover but the playing album's;
    /// hovering lights a tile back up.
    #[serde(default)]
    pub dim_playing: bool,
    /// How far the dimmed covers fade, in percent of fully hidden.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Each cover tile's corner rounding, in percent of circular: zero
    /// keeps the wall square, 100 rounds each cover into a circle.
    #[serde(default)]
    pub rounding: f32,
    /// The space between tiles, in px; zero keeps the wall seamless.
    #[serde(default)]
    pub gap: f32,
    /// Show the album title and artist under every cover, iTunes style,
    /// instead of only on hover. Off by default; the bare wall is the
    /// grid's long-standing look.
    #[serde(default)]
    pub labels: bool,
    /// How those captions line up under their covers. Left by default.
    #[serde(default)]
    pub label_align: TitleAlign,
    /// The top-left album shown when the layout was saved, so a relaunch
    /// reopens the wall where it was left. A cell index, not pixels or a
    /// row: it survives a tile-size or width change, landing back on the
    /// same album whatever the column count works out to.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub scroll: usize,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            vertical: true,
            tile: default_tile(),
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            dim_playing: false,
            dim: default_dim(),
            rounding: 0.,
            gap: 0.,
            labels: false,
            label_align: TitleAlign::default(),
            scroll: 0,
        }
    }
}

/// One album's run in the current view: where it starts, how many
/// tracks it spans, and the first track's path once a paint resolved it
/// (the inner None is a track the store no longer knows).
struct Cell {
    start: usize,
    len: u32,
    art: Option<Option<PathBuf>>,
    /// The tile's current opacity under the dim mode, easing toward its
    /// target every frame. None until the tile's first paint, which
    /// lands at the target directly: only changes fade, a tile scrolled
    /// into a dimmed wall arrives already dimmed.
    dim: Option<f32>,
}

/// How many columns the grid falls back to before its first paint has
/// measured a width.
const FALLBACK_COLS: usize = 4;

/// Rows of covers asked for past each edge of the viewport, so a scroll
/// reveals loaded tiles instead of placeholders.
const PREFETCH_ROWS: usize = 2;

/// How long a type-ahead phrase keeps growing before the next keystroke
/// starts a fresh jump.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

pub struct GridPanel {
    state: AppState,
    config: GridConfig,
    /// The rows the cells index into: the canonical order while the query
    /// is empty, otherwise the search hits re-ordered canonically so an
    /// album's tracks stay one contiguous run.
    view: Arc<Vec<u32>>,
    /// The albums of the current view, rebuilt on library updates and
    /// query changes.
    cells: Vec<Cell>,
    /// The query editor, the shared search box; `config.query` mirrors
    /// its value via change events.
    search: Entity<SearchBox>,
    /// The clicked albums, the accent outlines and the published
    /// selection; grows by the library's click rules, per tile.
    selected: HashSet<usize>,
    /// Where a shift-extend grows from: the last plain or toggle click.
    anchor: Option<usize>,
    /// The tile under the pointer, wearing the label overlay.
    hovered: Option<usize>,
    /// The cross-axis extent the grid last laid out for: the width while it
    /// scrolls vertically, the height while it scrolls horizontally. The
    /// dock hosts panels cached, so a resize repaints without re-rendering;
    /// the list closure compares the painted extent against this and
    /// notifies on drift.
    cross: Pixels,
    scroll: VirtualListScrollHandle,
    /// The drag-to-scroll state: press anywhere on the wall, drag to
    /// scroll, release to coast. A drag past its dead zone swallows the
    /// tile click.
    flick: FlickState,
    /// The list row the follow-playing glide is headed to; stepped every
    /// frame in `body` and cleared on arrival or on a user drag.
    glide_to: Option<usize>,
    /// The saved top-left album waiting to be scrolled back into place on a
    /// relaunch. A cell index, held until the wall has both albums and a
    /// measured width, then applied once in `body` and cleared. A user drag
    /// clears it too, so a hand on the wall wins over the restore.
    restore: Option<usize>,
    /// The last animation tick, the coast's and the glide's dt.
    last_tick: Instant,
    /// The idle-resume clock: stamped on every scroll or press, it wakes
    /// the wall back to the playing album once `resume_playing` is on and
    /// the user has stepped away.
    resume_idle: ResumeIdle,
    /// The playing track's path, the change detector for follow-playing.
    playing_path: Option<PathBuf>,
    /// The playing album's cell in the current view, kept fresh by
    /// `sync_playing` and `rebuild` so per-frame dimming never rescans.
    playing_ix: Option<usize>,
    /// Whether audio is moving right now; pause lifts the dim.
    playing: bool,
    /// The tile size slider's scrub strip, for the settings window.
    tile_scrub: ScrubState,
    /// The tile rounding slider's scrub strip, same window.
    rounding_scrub: ScrubState,
    /// The tile gap slider's scrub strip, same window.
    gap_scrub: ScrubState,
    /// The dim amount slider's scrub strip, the behavior page.
    dim_scrub: ScrubState,
    /// A failed play, shown in a strip until the next one lands.
    error: Option<SharedString>,
    /// A pending box reset from a source toggle or a shared-query change;
    /// applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// The type-ahead phrase and when its last keystroke landed, so typing
    /// while the wall has focus jumps to the album by prefix, and a quick
    /// run of keys grows one phrase instead of restarting each stroke.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _player_changed: Subscription,
}

impl GridPanel {
    pub fn new(
        state: AppState,
        config: GridConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A rescan can rewrite the order, tags, and id -> path mappings;
        // rebuild the albums over the new projection.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.rebuild(cx);
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this first rebuild; rescans
                // re-land on the playing album the same way.
                if this.config.follow_playing {
                    this.follow_playing(cx);
                }
            },
        );
        // Landing thumbnails notify the service; repaint so tiles fill in.
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        // A grid restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: rebuild the wall and reset
        // the box to it on the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        // Follow-playing owns the position on launch, so it skips the saved
        // scroll; every other panel restores where it was left.
        let restore = (!config.follow_playing && config.scroll > 0).then_some(config.scroll);
        let mut this = GridPanel {
            state,
            config,
            view: Arc::new(Vec::new()),
            cells: Vec::new(),
            search,
            selected: HashSet::new(),
            anchor: None,
            hovered: None,
            cross: px(0.),
            scroll: VirtualListScrollHandle::new(),
            flick: FlickState::default(),
            glide_to: None,
            restore,
            last_tick: Instant::now(),
            resume_idle: ResumeIdle::default(),
            playing_path: None,
            playing_ix: None,
            playing: false,
            tile_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            error: None,
            resync_box: false,
            type_ahead: String::new(),
            type_ahead_at: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _thumbs_changed,
            _search_events,
            _query_changed,
            _player_changed,
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, head for the album it lives
    /// in, and keep the dim mode's facts fresh. The compares keep the
    /// per-tick observer cheap, the player notifies every pump.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let (playing, path) = {
            let player = self.state.player.read(cx);
            (
                player.is_playing(),
                player.now_playing().map(|now| now.path),
            )
        };
        if playing != self.playing {
            // Pause lifts the dim, resuming drops it back; render steps
            // the fade, this kicks it off.
            self.playing = playing;
            cx.notify();
        }
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        self.playing_ix = self.playing_cell(cx);
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// The playing track's album in the current view, when it holds one.
    fn playing_cell(&self, cx: &App) -> Option<usize> {
        let path = self.playing_path.as_ref()?;
        let library = self.state.library.read(cx);
        let id = library.id_for(path)?;
        let projection = library.projection()?;
        let view_ix = self
            .view
            .iter()
            .position(|&row| projection.db_id[row as usize] == id)?;
        // Cells are contiguous runs over the view; the last one
        // starting at or before the hit holds it.
        Some(
            self.cells
                .partition_point(|cell| cell.start <= view_ix)
                .saturating_sub(1),
        )
    }

    /// Scroll the playing track's album into view: a glide when smooth is
    /// on, a centered jump otherwise.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        // Both modes head for the same line through the per-frame stepping
        // in `body`: the line is the stable fact, its offset depends on a
        // layout that may still be settling (a launch's first frames), so
        // even the jump re-pins until the target holds still.
        self.glide_to = Some(cell_ix / self.lanes());
        cx.notify();
    }

    /// The menu's jump: select the playing track's album and head there
    /// with the panel's configured motion. The automatic follow never
    /// touches the selection; this deliberate move does.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.selected = HashSet::from([cell_ix]);
        self.anchor = Some(cell_ix);
        self.publish_selection(cx);
        self.follow_playing(cx);
    }

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the wall drifts back to the playing album once the user steps away.
    /// A no-op unless the resume behavior is on, so an off panel spends
    /// nothing per gesture.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The idle wake's landing: slide back to the playing album, so long as
    /// the resume is still on. The clock only fires this once the wall has
    /// sat untouched a full window, a gesture in between having pushed it
    /// out, so no extra idle check is needed here.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// The menu's follow toggle: flip the follow state and catch up right
    /// away when turning it on, the same move as the settings switch.
    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.config.follow_playing = !self.config.follow_playing;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Flip the scroll axis, from the context menu or the settings toggle.
    /// The lane count and tile edge both key off the cross extent, so drop
    /// the measured one and let the next paint re-measure; any coast or
    /// pending restore aimed at the old axis is stale, so clear them too.
    fn set_orientation(&mut self, vertical: bool, cx: &mut Context<Self>) {
        if self.config.vertical == vertical {
            return;
        }
        self.config.vertical = vertical;
        self.glide_to = None;
        self.restore = None;
        self.cross = px(0.);
        cx.notify();
    }

    /// Recompute the view and its album runs: the canonical order, cut to
    /// the query's hits when one is set. Search hits come back in
    /// projection row order, so they filter the canonical order rather
    /// than getting walked directly - otherwise an album's scattered rows
    /// would split into duplicate tiles. Breaks on the album artist, not
    /// the track artist, the library's grouping rule, so a compilation
    /// stays one tile.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.cells.clear();
        self.selected.clear();
        self.anchor = None;
        self.hovered = None;
        self.view = {
            let query = self.effective_query(cx);
            let filter = self.effective_filter(cx);
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => {
                    let mask = projection.filter_mask(&filter);
                    if query.is_empty() && mask.is_none() {
                        library.order()
                    } else {
                        let mut hit = vec![query.is_empty(); projection.len()];
                        if !query.is_empty() {
                            for row in projection.search(&query) {
                                hit[row as usize] = true;
                            }
                        }
                        if let Some(mask) = mask {
                            for (hit, ok) in hit.iter_mut().zip(&mask) {
                                *hit = *hit && *ok;
                            }
                        }
                        Arc::new(
                            library
                                .order()
                                .iter()
                                .copied()
                                .filter(|&row| hit[row as usize])
                                .collect(),
                        )
                    }
                }
                None => Arc::new(Vec::new()),
            }
        };
        if let Some(projection) = self.state.library.read(cx).projection() {
            let mut last = None;
            for (i, &row) in self.view.iter().enumerate() {
                let key = (
                    projection.album_artist[row as usize],
                    projection.album[row as usize],
                );
                if last != Some(key) {
                    self.cells.push(Cell {
                        start: i,
                        len: 0,
                        art: None,
                        dim: None,
                    });
                    last = Some(key);
                }
                self.cells.last_mut().unwrap().len += 1;
            }
        }
        self.playing_ix = self.playing_cell(cx);
        cx.notify();
    }

    /// Map the shared box's events onto the grid: a changed query rebuilds
    /// the view, and every visual change also repaints the title row,
    /// which only updates when the tab panel is notified.
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
                self.refresh_title_bar(cx);
            }
            // Escape on an empty query leaves the box, which hands the
            // playback keys back to the workspace.
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                self.refresh_title_bar(cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// An album's tracks as db ids in view order, capped for the player
    /// queue.
    fn ids_for(&self, ix: usize, cx: &App) -> Vec<i64> {
        let Some(cell) = self.cells.get(ix) else {
            return Vec::new();
        };
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        self.view[cell.start..]
            .iter()
            .take((cell.len as usize).min(QUEUE_CAP))
            .map(|&row| projection.db_id[row as usize])
            .collect()
    }

    /// The artist a tile filters by: its first track's, the album grid's
    /// stand-in for the album's shelf. None off the end of the cells.
    fn cell_artist(&self, ix: usize, cx: &App) -> Option<String> {
        let cell = self.cells.get(ix)?;
        let row = *self.view.get(cell.start)?;
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        Some(projection.resolve(row).artist.to_string())
    }

    /// The path a tile's thumbnail loads by: the album's first track,
    /// resolved through the store once, on the tile's first paint.
    fn art_path(&mut self, ix: usize, cx: &Context<Self>) -> Option<PathBuf> {
        if let Some(art) = self.cells.get(ix).and_then(|cell| cell.art.clone()) {
            return art;
        }
        let path = {
            let library = self.state.library.read(cx);
            let id = self.cells.get(ix).and_then(|cell| {
                let projection = library.projection()?;
                let row = *self.view.get(cell.start)?;
                // No album tag means this is the unknown bucket, not a real
                // album: keep the placeholder instead of whichever loose
                // track's art lands first.
                if projection.resolve(row).album.is_empty() {
                    return None;
                }
                Some(projection.db_id[row as usize])
            });
            id.and_then(|id| library.paths_for(&[id]).ok())
                .and_then(|mut paths| paths.pop())
        };
        if let Some(cell) = self.cells.get_mut(ix) {
            cell.art = Some(path.clone());
        }
        path
    }

    /// Put a click on an album tile: plain selects just it, shift extends
    /// from the anchor, cmd (ctrl elsewhere) toggles - the library's
    /// click rules, by tile. Publishes the selection either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            // Ctrl+Shift stacks the range onto the selection so you can
            // skip a run and grab a second block; plain shift replaces.
            if modifiers.secondary() {
                self.selected.extend(lo..=hi);
            } else {
                self.selected = (lo..=hi).collect();
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(ix) {
                self.selected.remove(&ix);
            }
            self.anchor = Some(ix);
        } else {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// Resolve the selected albums to db ids in view order and publish
    /// them on the shared selection.
    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let ids: Vec<i64> = ixs.iter().flat_map(|&ix| self.ids_for(ix, cx)).collect();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
    }

    /// Browse from the keyboard while the wall is focused: plain typing
    /// jumps to the album whose name starts with the phrase. Modifiers pass
    /// through so the workspace keeps its shortcuts, and a leading space
    /// stays its play/pause instead of starting a phrase with a blank.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        let Some(text) = &keystroke.key_char else {
            return;
        };
        if self.type_ahead.is_empty() && text == " " {
            return;
        }
        self.type_to(text.clone(), cx);
    }

    /// Grow or restart the type-ahead phrase and jump to the album it names.
    /// A fresh phrase starts past the current selection, so the same letter
    /// walks to the next match; a grown one re-tests the current album so
    /// refining a match stays put. Matches album names by prefix, the
    /// caption's own text.
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
        let len = self.cells.len();
        if len == 0 {
            return;
        }
        let needle = self.type_ahead.to_lowercase();
        // A grown phrase re-tests the current album; a fresh one starts past
        // it, so the same first letter steps to the next match.
        let anchor = self.selected.iter().copied().min().or(self.anchor);
        let start = match anchor {
            Some(ix) if grown => ix,
            Some(ix) => ix + 1,
            None => 0,
        };
        let hit = {
            let library = self.state.library.read(cx);
            library.projection().and_then(|projection| {
                (0..len).map(|off| (start + off) % len).find(|&ix| {
                    self.cells
                        .get(ix)
                        .and_then(|cell| self.view.get(cell.start))
                        .and_then(|&row| {
                            projection.albums.lower.get(projection.album[row as usize] as usize)
                        })
                        .is_some_and(|album| album.starts_with(&needle))
                })
            })
        };
        if let Some(ix) = hit {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
            self.publish_selection(cx);
            self.scroll_to_cell(ix, cx);
        }
    }

    /// Bring an album's tile into view, centered on the scroll axis. Clears
    /// any pending glide or restore so the jump wins over an automatic move.
    fn scroll_to_cell(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.glide_to = None;
        self.restore = None;
        let line = ix / self.lanes();
        self.scroll.scroll_to_item(line, ScrollStrategy::Center);
        cx.notify();
    }

    /// Queue the album on the shared player.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.play_many(vec![ix], cx);
    }

    /// Queue several albums on the shared player, in view order under
    /// the queue cap.
    fn play_many(&mut self, ixs: Vec<usize>, cx: &mut Context<Self>) {
        let ids: Vec<i64> = ixs
            .iter()
            .flat_map(|&ix| self.ids_for(ix, cx))
            .take(QUEUE_CAP)
            .collect();
        let result = self.state.library.read(cx).paths_for(&ids);
        match result {
            Ok(paths) => {
                self.error = None;
                self.state
                    .player
                    .update(cx, |player, cx| player.play(paths, cx));
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    /// How many tiles share a row at the current width: enough that the
    /// configured edge covers it. The ceil keeps the actual edge at or
    /// under the configured one, so nothing upscales past the stored
    /// thumbnail.
    fn lanes(&self) -> usize {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return FALLBACK_COLS;
        }
        let gap = self.config.gap;
        // The caption rides below the cover, so while the wall scrolls
        // horizontally each tile's footprint along the height grows by it;
        // vertical packing is unchanged, the caption extends the row down
        // into the scroll instead of eating a lane.
        let footprint = self.config.tile + self.cross_label();
        (((cross + gap) / (footprint + gap)).ceil() as usize).max(1)
    }

    /// The caption's height when titles are on, else zero.
    fn label_height(&self) -> f32 {
        if self.config.labels {
            TILE_LABEL_H
        } else {
            0.
        }
    }

    /// The caption's take out of the cross extent: only horizontal walls
    /// stack captions along the packing axis, a vertical wall sends them
    /// into the scroll.
    fn cross_label(&self) -> f32 {
        if self.config.vertical {
            0.
        } else {
            self.label_height()
        }
    }

    /// The scroll axis: down the rows while vertical, across the columns
    /// otherwise.
    fn axis(&self) -> Axis {
        if self.config.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The leading album currently in view, for the saved layout: the list's
    /// first line spread back over the lanes. A restore still pending (the
    /// panel never painted) reports its own target, so an unshown panel
    /// round-trips its position instead of dropping to zero.
    fn first_cell(&self) -> usize {
        if let Some(cell) = self.restore {
            return cell;
        }
        let lanes = self.lanes();
        let extent = f32::from(self.tile_side()) + self.config.gap;
        if extent <= 0. {
            return 0;
        }
        // The leading line is the scroll offset over one line's pitch; the
        // offset runs negative as the list scrolls.
        let offset = f32::from(-self.scroll.base_handle().offset().along(self.axis()));
        let line = (offset / extent).floor().max(0.) as usize;
        (line * lanes).min(self.cells.len().saturating_sub(1))
    }

    /// A tile's edge: the cross extent split evenly over the lanes with the
    /// gaps taken out, so the last lane lands on the panel edge instead of
    /// bleeding past it.
    fn tile_side(&self) -> Pixels {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return px(self.config.tile);
        }
        let lanes = self.lanes() as f32;
        px((((cross - self.config.gap * (lanes - 1.)) / lanes) - self.cross_label()).max(1.))
    }

    /// A tile's resting opacity under the dim mode: full for the playing
    /// album and the hovered tile, the configured floor for everything
    /// else while audio moves, full for all when it stops.
    fn dim_target(&self, ix: usize) -> f32 {
        if self.config.dim_playing
            && self.playing
            && self.playing_ix != Some(ix)
            && self.hovered != Some(ix)
        {
            1.0 - self.config.dim / TILE_DIM_MAX
        } else {
            1.0
        }
    }

    /// One album tile: the cover filling a square, the label overlay while
    /// hovered, the accent outline while selected. Pending and missing art
    /// wear the same quiet placeholder, so a landing cover fills the tile
    /// without a flash.
    fn tile(&mut self, ix: usize, side: Pixels, cx: &mut Context<Self>) -> AnyElement {
        // The first paint lands at the target directly; from then on the
        // stepping in `body` owns the value.
        let dim = match self.cells.get(ix).and_then(|cell| cell.dim) {
            Some(dim) => dim,
            None => {
                let target = self.dim_target(ix);
                if let Some(cell) = self.cells.get_mut(ix) {
                    cell.dim = Some(target);
                }
                target
            }
        };
        let path = self.art_path(ix, cx);
        let thumb = match path {
            Some(path) => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx)),
            None => Thumb::Missing,
        };
        // The knob is percent of circular, so the radius scales with the
        // tile: 100 turns the square into a circle. It clips the cover
        // itself, not just the tile's background: gpui content masks stay
        // rectangular, so a rounded tile under a square image would paint
        // over its own corners.
        let radius = side * (self.config.rounding / 200.);
        let content: AnyElement = match thumb {
            Thumb::Ready(image) => img(image)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .rounded(radius)
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icons::MUSIC)
                        .size(px(24.))
                        .text_color(palette::text_faint()),
                )
                .into_any_element(),
        };
        let labels = self.config.labels;
        // The cover square: the art, its hover overlay while captions are
        // off, and the selection outline. The caption, when on, sits below
        // it in the tile wrapper rather than over the art.
        let cover = div()
            .w(side)
            .h(side)
            .relative()
            .overflow_hidden()
            .rounded(radius)
            .bg(palette::bg_elevated())
            .child(content)
            .when(!labels && self.hovered == Some(ix), |d| {
                d.child(self.label(ix, cx))
            })
            .when(self.selected.contains(&ix), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .rounded(radius)
                        .border_color(palette::accent()),
                )
            });
        div()
            .id(ix)
            .w(side)
            .flex()
            .flex_col()
            .when(dim < 1., |d| d.opacity(dim))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let target = hovered.then_some(ix);
                if this.hovered != target && (this.hovered == Some(ix) || *hovered) {
                    this.hovered = target;
                    cx.notify();
                }
            }))
            // Actions land on release, not press: a press might be the
            // start of a drag-scroll, and one that traveled is not a click.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if this.flick.scrolled() {
                        return;
                    }
                    this.focus.focus(window);
                    if event.click_count > 1 {
                        this.play(ix, cx);
                    } else {
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .child(cover)
            .when(labels, |d| d.child(self.caption(ix, side, cx)))
            .into_any_element()
    }

    /// A tile's album and artist strings: the first track's, with the
    /// album artist standing in when the row carries one. Empty off the
    /// end of the cells or before a projection lands.
    fn cell_labels(&self, ix: usize, cx: &App) -> (SharedString, SharedString) {
        let library = self.state.library.read(cx);
        match (self.cells.get(ix), library.projection()) {
            (Some(cell), Some(projection)) => self
                .view
                .get(cell.start)
                .map(|&row| {
                    let v = projection.resolve(row);
                    // Rows from before the album artist column carry an
                    // empty one; the first track's artist stands in.
                    let artist = if v.album_artist.is_empty() {
                        v.artist
                    } else {
                        v.album_artist
                    };
                    (
                        SharedString::from(v.album.to_string()),
                        SharedString::from(artist.to_string()),
                    )
                })
                .unwrap_or_default(),
            _ => Default::default(),
        }
    }

    /// The hover overlay: album over artist on a translucent strip along
    /// the tile's bottom edge.
    fn label(&self, ix: usize, cx: &App) -> Div {
        let (album, artist) = self.cell_labels(ix, cx);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .bg(palette::alpha(palette::bg_root(), 0xCC))
            .flex()
            .flex_col()
            .when(!album.is_empty(), |d| {
                d.child(
                    div()
                        .truncate()
                        .text_color(palette::text_bright())
                        .child(album),
                )
            })
            .when(!artist.is_empty(), |d| {
                d.child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_secondary())
                        .child(artist),
                )
            })
    }

    /// The always-on caption under a cover: album over artist in a fixed
    /// block, so the tile's total height stays predictable for the virtual
    /// list. Widths match the cover so long titles truncate at its edge.
    fn caption(&self, ix: usize, side: Pixels, cx: &App) -> Div {
        let (album, artist) = self.cell_labels(ix, cx);
        let base = div()
            .w(side)
            .h(px(TILE_LABEL_H))
            .pt(tokens::SPACE_XS)
            .flex()
            .flex_col()
            .overflow_hidden();
        // The text alignment cascades to both lines; each line truncates at
        // the cover's edge, so a centered or right title stays under its art.
        match self.config.label_align {
            TitleAlign::Left => base.text_left(),
            TitleAlign::Center => base.text_center(),
            TitleAlign::Right => base.text_right(),
        }
            .child(
                div()
                    .truncate()
                    .text_sm()
                    .text_color(palette::text_bright())
                    .child(album),
            )
            .when(!artist.is_empty(), |d| {
                d.child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_secondary())
                        .child(artist),
                )
            })
    }

    /// Solo or popped out there is no title bar to host the search, so it
    /// renders as a toolbar row above the wall instead, the library's move.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .child(
                self.search
                    .update(cx, |search, cx| search.element(cx))
                    .flex_1(),
            )
    }

    /// The visible rows of the grid, each a run of tiles. Also where the
    /// painted width reconciles: the dock hosts panels cached, so a resize
    /// repaints this closure without re-running render, and a notify here
    /// is what recomputes the column count next frame.
    fn lines(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let axis = self.axis();
        let measured = self.scroll.base_handle().bounds().size.along(axis.invert());
        if measured > px(0.) && measured != self.cross {
            self.cross = measured;
            cx.notify();
        }
        let lanes = self.lanes();
        let side = self.tile_side();
        let gap = px(self.config.gap);
        let vertical = self.config.vertical;
        let lines = range
            .clone()
            .map(|line| {
                // A line is a row of tiles filling the width while vertical,
                // a column filling the height otherwise; the cross gap sits
                // between the tiles, the scroll gap between the lines through
                // the list's own spacing.
                let mut lane = if vertical {
                    div().flex().flex_row().gap(gap)
                } else {
                    div().flex().flex_col().gap(gap)
                };
                for ix in (line * lanes)..((line + 1) * lanes).min(self.cells.len()) {
                    lane = lane.child(self.tile(ix, side, cx));
                }
                lane
            })
            .collect();
        // Warm the margin: ask for the covers just past both edges so a
        // scroll reveals loaded tiles. Asked after the visible tiles, which
        // keeps those first in line for the load pool's slots.
        let above =
            (range.start * lanes).saturating_sub(PREFETCH_ROWS * lanes)..range.start * lanes;
        let below = range.end * lanes..((range.end + PREFETCH_ROWS) * lanes).min(self.cells.len());
        for ix in above.chain(below) {
            if let Some(path) = self.art_path(ix, cx) {
                self.state.thumbs.update(cx, |thumbs, cx| {
                    thumbs.get(&path, cx);
                });
            }
        }
        lines
    }
}

impl PanelSettings for GridPanel {
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    "Orientation",
                    None,
                    setting_row(
                        "Vertical Layout",
                        Some("Scroll the wall up and down, rows filling the width; off scrolls it left and right, columns filling the height"),
                        toggle(
                            self.config.vertical,
                            |this: &mut Self, on, cx| {
                                this.set_orientation(on, cx);
                            },
                            cx,
                        ),
                    ),
                ))
                .child(crate::query::shared_query::search_section(
                    self.config.search,
                    |this: &mut Self, on, cx| {
                        this.config.search = on;
                        // The box keeps its text; the view snaps to the
                        // full catalog while hidden. Rebuild notifies, the
                        // tab panel repaints the vanishing suffix.
                        this.rebuild(cx);
                        this.refresh_title_bar(cx);
                    },
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    "Scroll to the playing album whenever the track changes",
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        // Catch up right away instead of waiting for
                        // the next track change.
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.config.resume_playing,
                    "Slide back to the playing album after you stop browsing",
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    "Glide to the album instead of jumping",
                    |this: &mut Self, on, cx| {
                        this.config.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .child(settings_ui::section(
                    "Dimming",
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(setting_row(
                            "Dim While Playing",
                            Some("Fade every cover but the playing album's; hovering lights a tile back up"),
                            toggle(
                                self.config.dim_playing,
                                |this: &mut Self, on, cx| {
                                    this.config.dim_playing = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .when(self.config.dim_playing, |d| {
                            d.child(setting_row(
                                "Dim Amount",
                                Some("How far the other covers fade; 100% hides them"),
                                settings_ui::slider_labeled(
                                    &self.dim_scrub,
                                    (self.config.dim / TILE_DIM_MAX).clamp(0., 1.),
                                    format!("{:.0} %", self.config.dim),
                                    |this: &mut Self, fraction, cx| {
                                        this.config.dim = (fraction * TILE_DIM_MAX).round();
                                        cx.notify();
                                    },
                                    cx,
                                ),
                            ))
                        }),
                ))
                .into_any_element(),
        )
    }

    /// The grid's own appearance rows on the shared page: the tiles'
    /// size, gap, and art rounding, look knobs that live on the config
    /// rather than the theme because they shape the covers, not the
    /// panel frame.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tile_fraction = ((self.config.tile - TILE_MIN) / (TILE_MAX - TILE_MIN)).clamp(0., 1.);
        let rounding = self.config.rounding;
        let rounding_fraction = (rounding / TILE_ROUNDING_MAX).clamp(0., 1.);
        Some(
            settings_ui::section(
                "Tiles",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "Show Titles",
                        Some("Print the album and artist under every cover, iTunes style, instead of only on hover"),
                        toggle(
                            self.config.labels,
                            |this: &mut Self, on, cx| {
                                this.config.labels = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .when(self.config.labels, |d| {
                        d.child(setting_row(
                            "Title Alignment",
                            Some("Line the captions up under their covers"),
                            panel::icon_choices(
                                &[
                                    (icons::ALIGN_LEFT, TitleAlign::Left),
                                    (icons::ALIGN_CENTER, TitleAlign::Center),
                                    (icons::ALIGN_RIGHT, TitleAlign::Right),
                                ],
                                self.config.label_align,
                                |this: &mut Self, align, cx| {
                                    this.config.label_align = align;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                    })
                    .child(setting_row(
                        "Tile Size",
                        Some("The cover tiles' widest edge; columns split the panel width evenly"),
                        settings_ui::slider_labeled(
                            &self.tile_scrub,
                            tile_fraction,
                            format!("{}px", self.config.tile.round() as u32),
                            |this: &mut Self, fraction, cx| {
                                this.config.tile = TILE_MIN + fraction * (TILE_MAX - TILE_MIN);
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Gap",
                        Some("Space between the covers; zero keeps the wall seamless"),
                        settings_ui::slider_labeled(
                            &self.gap_scrub,
                            (self.config.gap / TILE_GAP_MAX).clamp(0., 1.),
                            format!("{:.0} px", self.config.gap),
                            |this: &mut Self, fraction, cx| {
                                this.config.gap = (fraction * TILE_GAP_MAX).round();
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Art Rounding",
                        Some("Round each cover's corners; 100% is a circle"),
                        settings_ui::slider_labeled(
                            &self.rounding_scrub,
                            rounding_fraction,
                            format!("{:.0} %", rounding),
                            |this: &mut Self, fraction, cx| {
                                this.config.rounding = (fraction * TILE_ROUNDING_MAX).round();
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            )
            .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for GridPanel {}

impl Focusable for GridPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueryFilter for GridPanel {
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
        self.rebuild(cx);
    }
    fn set_query_resync(&mut self, pending: bool) {
        self.resync_box = pending;
    }
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        self.refresh_title_bar(cx);
    }
}

impl Panel for GridPanel {
    fn panel_name(&self) -> &'static str {
        "album grid"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Album Grid")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The search box shares the title bar row, the library's move.
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

    /// The wall serves tile context menus over the whole body, so the
    /// tab panel's body right-click stays out; the panel dropdown rides
    /// along after the play items, the library's arrangement.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        let mut config = self.config.clone();
        config.scroll = self.first_cell();
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
        let weak = cx.entity().downgrade();
        let weak_f = cx.entity().downgrade();
        let follow = self.config.follow_playing;
        // Checks on the right so the orientation pair keeps its icons; the
        // default left side would swap them out for the checkmark.
        let menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::new("Jump to Playing")
                    .icon(Icon::default().path(icons::DISC))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.jump_to_playing(cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("Follow Playing")
                    .icon(Icon::default().path(icons::LOCATE))
                    .checked(follow)
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak_f.upgrade() {
                            this.update(cx, |this, cx| this.toggle_follow_playing(cx));
                        }
                    }),
            );

        // Display section: the view knobs group under flyouts so the menu
        // stays short, the same shape as the library's.
        let menu = menu.separator().label("Display");
        // The scroll direction, a checked pair so the current axis reads at
        // a glance.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (name, icon, is_vertical) in [
                ("Vertical Scroll", icons::MOVE_VERTICAL, true),
                ("Horizontal Scroll", icons::MOVE_HORIZONTAL, false),
            ] {
                submenu = submenu.item(panel::check_row(
                    name,
                    Some(icon),
                    move |this: &Self| this.config.vertical == is_vertical,
                    move |this, cx| this.set_orientation(is_vertical, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu("Scroll", submenu));
        // Follow the shared search query, or filter by this wall's own box.
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this, source, cx| this.pick_query_source(source, cx),
            |this, on, cx| {
                this.config.search = on;
                // The box keeps its text; the view snaps to the full catalog
                // while hidden. Rebuild notifies, the tab panel repaints the
                // vanishing suffix.
                this.rebuild(cx);
                this.refresh_title_bar(cx);
            },
            window,
            cx,
        );
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the cover panel's.
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
                    let dup = cx.new(|cx| GridPanel::new(state, config, window, cx));
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

impl Render for GridPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl GridPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // A pending box reset (a source toggle or a shared-query change)
        // lands here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let axis = self.axis();
        let lanes = self.lanes();
        let line_count = self.cells.len().div_ceil(lanes);
        let side = self.tile_side();

        // The frame-by-frame motion: a released flick coasts on, a follow
        // glide eases toward its line. Both step here in render (the cover
        // panel's fade idiom) and request the next frame only while
        // something still moves.
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.05);
        self.last_tick = Instant::now();
        if let Some(d) = self.flick.coast(dt) {
            let base = self.scroll.base_handle().clone();
            let offset = base.offset().apply_along(axis, |v| v + px(d));
            base.set_offset(offset);
            window.request_animation_frame();
        }
        if let Some(line) = self.glide_to {
            let handle = self.scroll.base_handle().clone();
            let arrived = match panel::glide_target_axis(&handle, axis, line, line_count) {
                Some(target) if self.config.smooth_follow => {
                    !panel::glide_step_axis(&handle, axis, target, dt)
                }
                Some(target) => panel::glide_snap_axis(&handle, axis, target),
                // Not laid out yet; wait for the list's first paint.
                None => false,
            };
            if arrived {
                self.glide_to = None;
            } else {
                window.request_animation_frame();
            }
        }
        // Restore the saved scroll once the wall has albums and a measured
        // extent: the lane count only lands after the first paint, and the
        // cell -> line map rides on it, so restoring any earlier would aim at
        // the fallback grid. Skipped while a follow glide runs, which owns
        // the position.
        if let Some(cell) = self.restore {
            if self.glide_to.is_none() && !self.cells.is_empty() && self.cross > px(0.) {
                let line = (cell / lanes).min(line_count.saturating_sub(1));
                self.scroll.scroll_to_item(line, ScrollStrategy::Top);
                self.restore = None;
            }
        }
        // The dim fade: every painted tile's opacity eases toward its
        // target, the glide's exponential approach. Frames only while
        // one is still moving; a settled wall costs a linear scan.
        let step = 1.0 - (0.08_f32).powf(dt * 10.0);
        let mut fading = false;
        for ix in 0..self.cells.len() {
            let Some(current) = self.cells[ix].dim else {
                continue;
            };
            let target = self.dim_target(ix);
            let diff = target - current;
            self.cells[ix].dim = Some(if diff.abs() < 0.005 {
                target
            } else {
                fading = true;
                current + diff * step
            });
        }
        if fading {
            window.request_animation_frame();
        }

        // The search lives in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there is no header at all, so
        // it renders as a toolbar in the body instead.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // Type-to-jump while the wall itself holds focus. The guard keeps
            // it off while the search box is focused, whose keys bubble up
            // through the toolbar child.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.focus.is_focused(window) {
                    this.on_panel_key(event, cx);
                }
            }))
            .when(headerless && self.config.search, |d| {
                d.child(self.toolbar(cx))
            });
        let content: AnyElement = if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(
                    if self.effective_query(cx).is_empty()
                        && self.effective_filter(cx).is_empty()
                    {
                        "The library is empty"
                    } else {
                        "No matches"
                    },
                )
                .into_any_element()
        } else {
            let entity = cx.entity();
            // Each line spans the cover plus, on a vertical wall, the caption
            // that trails it into the scroll; a horizontal wall stacks the
            // caption inside the cross extent, so its scroll pitch stays the
            // bare cover width.
            let line_extent = if self.config.vertical {
                side + px(self.label_height())
            } else {
                side
            };
            let item_sizes: Rc<Vec<Size<Pixels>>> =
                Rc::new(vec![size(side, line_extent); line_count]);
            let list = match axis {
                Axis::Vertical => {
                    v_virtual_list(entity, "album-grid", item_sizes, |this, range, _, cx| {
                        this.lines(range, cx)
                    })
                }
                Axis::Horizontal => {
                    h_virtual_list(entity, "album-grid", item_sizes, |this, range, _, cx| {
                        this.lines(range, cx)
                    })
                }
            }
            .track_scroll(&self.scroll)
            .gap(px(self.config.gap))
            .size_full();
            let scrollbar = match axis {
                Axis::Vertical => Scrollbar::vertical(&self.scroll),
                Axis::Horizontal => Scrollbar::horizontal(&self.scroll),
            };
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .relative()
                // Any press on the wall might be a drag-scroll; the tiles'
                // own actions moved to release so both can tell. It also
                // interrupts a running glide, the user wins.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        // Take focus so the type-to-jump keys land on the wall
                        // rather than whatever held focus before the press.
                        window.focus(&this.focus);
                        this.glide_to = None;
                        this.restore = None;
                        this.flick.begin(event.position.along(axis));
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
                // Every wheel over the wall, whichever axis the list scrolls,
                // counts as browsing; this stamp only restarts the idle clock
                // and leaves the scroll itself to the list and the gap-filler
                // below, so nothing scrolls twice.
                .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                    this.touch_resume(cx);
                }))
                // A plain wheel only carries a vertical delta, and the list
                // ignores it while it scrolls horizontally: both its overflow
                // axes are Scroll, so gpui never cross-maps y onto x. Fill
                // exactly that gap here; a trackpad's real x deltas stay with
                // the list's own handler, so nothing applies twice.
                .when(axis == Axis::Horizontal, |d| {
                    d.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let delta = event.delta.pixel_delta(window.line_height());
                        if delta.x != px(0.) || delta.y == px(0.) {
                            return;
                        }
                        this.glide_to = None;
                        this.restore = None;
                        let base = this.scroll.base_handle().clone();
                        let offset = base.offset().apply_along(Axis::Horizontal, |x| x + delta.y);
                        base.set_offset(offset);
                        cx.notify();
                    }))
                })
                .child(list)
                // A live drag-scroll follows the pointer through window
                // handlers armed in a paint pass, the scrub strips' idiom.
                // The canvas exists for that paint hook; the list's lines
                // closure can't arm them, it also runs during layout.
                .child(
                    canvas(|_, _, _| (), {
                        let flick = self.flick.clone();
                        let scroll = self.scroll.clone();
                        let weak = cx.entity().downgrade();
                        move |_, _, window, _| {
                            let scroll = scroll.clone();
                            let weak = weak.clone();
                            panel::flick_on_paint_axis(&flick, axis, window, move |d, cx| {
                                let base = scroll.base_handle().clone();
                                let offset = base.offset().apply_along(axis, |v| v + px(d));
                                base.set_offset(offset);
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |_, cx| cx.notify());
                                }
                            });
                        }
                    })
                    .absolute()
                    .size_full(),
                )
                .child(div().absolute().inset_0().child(scrollbar))
                // The wall's right-click menu, keyed off the hovered tile
                // since the builder gets no position: a click inside the
                // selection acts on the whole set, outside it the click
                // reselects just that tile first, so the menu always acts
                // on what is highlighted - the library's rule. Off any
                // tile the panel menu stands alone.
                .context_menu({
                    let weak = cx.entity().downgrade();
                    move |menu, window, cx| {
                        let Some(this) = weak.upgrade() else {
                            return menu;
                        };
                        let Some(ix) = this.read(cx).hovered else {
                            return this
                                .update(cx, |this, cx| this.dropdown_menu(menu, window, cx));
                        };
                        let ixs = this.update(cx, |this, cx| {
                            if !this.selected.contains(&ix) {
                                this.selected = HashSet::from([ix]);
                                this.anchor = Some(ix);
                                this.publish_selection(cx);
                                cx.notify();
                            }
                            let mut ixs: Vec<usize> = this.selected.iter().copied().collect();
                            ixs.sort_unstable();
                            ixs
                        });
                        let single_tile = ixs.len() == 1;
                        let label = if ixs.len() > 1 {
                            format!("Play {} Albums", ixs.len())
                        } else {
                            "Play".to_string()
                        };
                        // The selected albums' tracks as db ids, resolved
                        // now for the editor, the library rows' move.
                        let ids: Vec<i64> = this.update(cx, |this, cx| {
                            ixs.iter().flat_map(|&ix| this.ids_for(ix, cx)).collect()
                        });
                        let panel = weak.clone();
                        let state = this.read(cx).state.clone();
                        let menu = panel::track_actions(
                            menu,
                            state,
                            ids,
                            label,
                            window,
                            cx,
                            move |_, cx| {
                                if let Some(this) = panel.upgrade() {
                                    this.update(cx, |this, cx| this.play_many(ixs.clone(), cx));
                                }
                            },
                        );
                        // Faceted browse: pin the search to the tile's artist,
                        // the album grid's stand-in for the artist's shelf.
                        // Only a single tile has one artist to pin.
                        let menu = match this
                            .read(cx)
                            .cell_artist(ix, cx)
                            .filter(|_| single_tile)
                            .filter(|artist| !artist.is_empty())
                        {
                            Some(artist) => {
                                let artist_panel = weak.clone();
                                menu.separator().item(
                                    PopupMenuItem::new("Filter by Artist")
                                        .icon(Icon::default().path(icons::MIC))
                                        .on_click(move |_, _, cx| {
                                            let Some(this) = artist_panel.upgrade() else {
                                                return;
                                            };
                                            let artist = artist.clone();
                                            this.update(cx, |this, cx| {
                                                this.jump_to_query("artist", &artist, cx)
                                            });
                                        }),
                                )
                            }
                            None => menu,
                        };
                        this.update(cx, |this, cx| {
                            this.dropdown_menu(menu.separator(), window, cx)
                        })
                    }
                })
                .into_any_element()
        };
        root.child(content)
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .border_t_1()
                        .border_color(palette::border())
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }
}
