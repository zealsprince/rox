//! The album grid panel: the catalog as a wall of cover tiles, NekoRoX's
//! grid gallery. One tile per album in the library's canonical order,
//! square, the columns splitting the panel width evenly so the wall runs
//! edge to edge, textures through the shared artwork service.
//! Rows virtualize through a uniform_list, so a huge library costs only
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
use std::time::Instant;

use gpui::{
    canvas, div, img, prelude::*, px, svg, uniform_list, AnyElement, App, Context, Div, Entity,
    EventEmitter, FocusHandle, Focusable, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    ObjectFit, Pixels, SharedString, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, FlickState, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::panels::library::{LibraryEvent, QUEUE_CAP};
use crate::search::{SearchBox, SearchEvent};
use crate::settings_ui;
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

fn default_tile() -> f32 {
    192.
}

fn default_true() -> bool {
    true
}

/// The grid panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct GridConfig {
    /// The rename shown as the tab and title text; None shows the
    /// built-in name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub query: String,
    /// Show the search box; the query only applies while it shows.
    #[serde(default = "default_true")]
    pub search: bool,
    /// The preferred tile edge in px, within [`TILE_MIN`]..[`TILE_MAX`].
    #[serde(default = "default_tile")]
    pub tile: f32,
    /// Scroll to the playing album when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// Each cover tile's corner rounding, in percent of circular: zero
    /// keeps the wall square, 100 rounds each cover into a circle.
    #[serde(default)]
    pub rounding: f32,
    /// The space between tiles, in px; zero keeps the wall seamless.
    #[serde(default)]
    pub gap: f32,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            title: None,
            query: String::new(),
            search: true,
            tile: default_tile(),
            follow_playing: false,
            smooth_follow: false,
            rounding: 0.,
            gap: 0.,
            theme: PanelTheme::default(),
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
}

/// How many columns the grid falls back to before its first paint has
/// measured a width.
const FALLBACK_COLS: usize = 4;

/// Rows of covers asked for past each edge of the viewport, so a scroll
/// reveals loaded tiles instead of placeholders.
const PREFETCH_ROWS: usize = 2;

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
    /// The width the grid last laid out for. The dock hosts panels cached,
    /// so a resize repaints without re-rendering; the list closure compares
    /// the painted width against this and notifies on drift.
    width: Pixels,
    scroll: UniformListScrollHandle,
    /// The drag-to-scroll state: press anywhere on the wall, drag to
    /// scroll, release to coast. A drag past its dead zone swallows the
    /// tile click.
    flick: FlickState,
    /// The list row the follow-playing glide is headed to; stepped every
    /// frame in `body` and cleared on arrival or on a user drag.
    glide_to: Option<usize>,
    /// The last animation tick, the coast's and the glide's dt.
    last_tick: Instant,
    /// The playing track's path, the change detector for follow-playing.
    playing_path: Option<PathBuf>,
    /// The tile size slider's scrub strip, for the settings window.
    tile_scrub: ScrubState,
    /// The tile rounding slider's scrub strip, same window.
    rounding_scrub: ScrubState,
    /// The tile gap slider's scrub strip, same window.
    gap_scrub: ScrubState,
    /// A failed play, shown in a strip until the next one lands.
    error: Option<SharedString>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
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
        let search = cx.new(|cx| SearchBox::new("search", &config.query, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        let mut this = GridPanel {
            state,
            config,
            view: Arc::new(Vec::new()),
            cells: Vec::new(),
            search,
            selected: HashSet::new(),
            anchor: None,
            hovered: None,
            width: px(0.),
            scroll: UniformListScrollHandle::new(),
            flick: FlickState::default(),
            glide_to: None,
            last_tick: Instant::now(),
            playing_path: None,
            tile_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
            error: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _thumbs_changed,
            _search_events,
            _player_changed,
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, head for the album it lives
    /// in. The compare keeps the per-tick observer cheap, the player
    /// notifies every pump.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.playing_path {
            return;
        }
        self.playing_path = path;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
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
        let Some(cell_ix) = self.playing_cell(cx) else {
            return;
        };
        // Both modes head for the same row through the per-frame stepping
        // in `body`: the row is the stable fact, its offset depends on a
        // layout that may still be settling (a launch's first frames), so
        // even the jump re-pins until the target holds still.
        self.glide_to = Some(cell_ix / self.cols());
        cx.notify();
    }

    /// The menu's jump: select the playing track's album and head there
    /// with the panel's configured motion. The automatic follow never
    /// touches the selection; this deliberate move does.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_cell(cx) else {
            return;
        };
        self.selected = HashSet::from([cell_ix]);
        self.anchor = Some(cell_ix);
        self.publish_selection(cx);
        self.follow_playing(cx);
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
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) if self.searching() => {
                    let mut hit = vec![false; projection.len()];
                    for row in projection.search(&self.config.query) {
                        hit[row as usize] = true;
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
                Some(_) => library.order(),
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
                    });
                    last = Some(key);
                }
                self.cells.last_mut().unwrap().len += 1;
            }
        }
        cx.notify();
    }

    /// Whether the query narrows the wall: it only applies while the
    /// search box shows.
    fn searching(&self) -> bool {
        self.config.search && !self.config.query.is_empty()
    }

    /// Map the shared box's events onto the grid: a changed query rebuilds
    /// the view, and every visual change also repaints the title row,
    /// which only updates when the tab panel is notified.
    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                self.config.query = search.read(cx).query().to_string();
                self.rebuild(cx);
                self.refresh_title_bar(cx);
            }
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
            self.selected = (lo..=hi).collect();
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
    fn cols(&self) -> usize {
        let width = f32::from(self.width);
        if width <= 0. {
            return FALLBACK_COLS;
        }
        let gap = self.config.gap;
        (((width + gap) / (self.config.tile + gap)).ceil() as usize).max(1)
    }

    /// A tile's edge: the panel width split evenly over the columns with
    /// the gaps taken out, so the last column lands on the panel edge
    /// instead of bleeding past it.
    fn tile_side(&self) -> Pixels {
        let width = f32::from(self.width);
        if width <= 0. {
            return px(self.config.tile);
        }
        let cols = self.cols() as f32;
        px(((width - self.config.gap * (cols - 1.)) / cols).max(1.))
    }

    /// One album tile: the cover filling a square, the label overlay while
    /// hovered, the accent outline while selected. Pending and missing art
    /// wear the same quiet placeholder, so a landing cover fills the tile
    /// without a flash.
    fn tile(&mut self, ix: usize, side: Pixels, cx: &mut Context<Self>) -> AnyElement {
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
        div()
            .id(ix)
            .w(side)
            .h(side)
            .relative()
            .overflow_hidden()
            .rounded(radius)
            .bg(palette::bg_elevated())
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
            .child(content)
            .when(self.hovered == Some(ix), |d| d.child(self.label(ix, cx)))
            .when(self.selected.contains(&ix), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .rounded(radius)
                        .border_color(palette::accent()),
                )
            })
            .into_any_element()
    }

    /// The hover overlay: album over artist on a translucent strip along
    /// the tile's bottom edge.
    fn label(&self, ix: usize, cx: &App) -> Div {
        let (album, artist) = {
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
        };
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
    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let measured = self.scroll.0.borrow().base_handle.bounds().size.width;
        if measured > px(0.) && measured != self.width {
            self.width = measured;
            cx.notify();
        }
        let cols = self.cols();
        let side = self.tile_side();
        let gap = px(self.config.gap);
        let rows = range
            .clone()
            .map(|r| {
                // The bottom padding is the vertical gap; on every row so
                // the list's heights stay uniform.
                let mut row = div().flex().flex_row().gap(gap).pb(gap);
                for ix in (r * cols)..((r + 1) * cols).min(self.cells.len()) {
                    row = row.child(self.tile(ix, side, cx));
                }
                row
            })
            .collect();
        // Warm the margin: ask for the covers just past both edges so a
        // scroll reveals loaded tiles. Asked after the visible tiles,
        // which keeps those first in line for the load pool's slots.
        let above = (range.start * cols).saturating_sub(PREFETCH_ROWS * cols)..range.start * cols;
        let below = range.end * cols..((range.end + PREFETCH_ROWS) * cols).min(self.cells.len());
        for ix in above.chain(below) {
            if let Some(path) = self.art_path(ix, cx) {
                self.state.thumbs.update(cx, |thumbs, cx| {
                    thumbs.get(&path, cx);
                });
            }
        }
        rows
    }
}

impl PanelSettings for GridPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn custom_title(&self) -> Option<&str> {
        self.config.title.as_deref()
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Behavior", icons::SLIDERS)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "search",
                Some("show the search box; the query only applies while it shows"),
                toggle(
                    self.config.search,
                    |this: &mut Self, on, cx| {
                        this.config.search = on;
                        // The box keeps its text; the view snaps to the
                        // full catalog while hidden. Rebuild notifies, the
                        // tab panel repaints the vanishing suffix.
                        this.rebuild(cx);
                        this.refresh_title_bar(cx);
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                "follow playing",
                Some("scroll to the playing album whenever the track changes"),
                toggle(
                    self.config.follow_playing,
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        // Catch up right away instead of waiting for
                        // the next track change.
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.follow_playing, |d| {
                d.child(setting_row(
                    "smooth scrolling",
                    Some("glide to the album instead of jumping"),
                    toggle(
                        self.config.smooth_follow,
                        |this: &mut Self, on, cx| {
                            this.config.smooth_follow = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
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
                "tiles",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "tile size",
                        Some("the cover tiles' widest edge; columns split the panel width evenly"),
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
                        "gap",
                        Some("space between the covers; zero keeps the wall seamless"),
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
                        "art rounding",
                        Some("round each cover's corners; 100% is a circle"),
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

impl Panel for GridPanel {
    fn panel_name(&self) -> &'static str {
        "album grid"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.title.as_deref(), "album grid")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.title.clone().map(SharedString::from)
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu
            .item(
                PopupMenuItem::new("Jump to Playing")
                    .icon(Icon::default().path(icons::DISC))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.jump_to_playing(cx));
                        }
                    }),
            )
            .separator();
        let menu = panel_settings::rename_item(menu, &cx.entity());
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
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(window, cx))
    }
}

impl GridPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let cols = self.cols();
        let row_count = self.cells.len().div_ceil(cols);

        // The frame-by-frame motion: a released flick coasts down, a
        // follow glide eases toward its row. Both step here in render
        // (the cover panel's fade idiom) and request the next frame only
        // while something still moves.
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.05);
        self.last_tick = Instant::now();
        if let Some(dy) = self.flick.coast(dt) {
            let base = self.scroll.0.borrow().base_handle.clone();
            let mut offset = base.offset();
            offset.y += px(dy);
            base.set_offset(offset);
            window.request_animation_frame();
        }
        if let Some(row) = self.glide_to {
            let arrived = match panel::glide_target(&self.scroll, row, row_count) {
                Some(target) if self.config.smooth_follow => {
                    !panel::glide_step(&self.scroll, target, dt)
                }
                Some(target) => panel::glide_snap(&self.scroll, target),
                // Not laid out yet; wait for the list's first paint.
                None => false,
            };
            if arrived {
                self.glide_to = None;
            } else {
                window.request_animation_frame();
            }
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
                .child(if self.searching() {
                    "no matches"
                } else {
                    "the library is empty"
                })
                .into_any_element()
        } else {
            let this = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .relative()
                // Any press on the wall might be a drag-scroll; the tiles'
                // own actions moved to release so both can tell. It also
                // interrupts a running glide, the user wins.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.glide_to = None;
                        this.flick.begin(event.position.y);
                        cx.notify();
                    }),
                )
                .child(
                    uniform_list("album-grid", row_count, move |range, _, cx| {
                        this.upgrade()
                            .map(|this| this.update(cx, |this, cx| this.rows(range, cx)))
                            .unwrap_or_default()
                    })
                    .track_scroll(self.scroll.clone())
                    .size_full(),
                )
                // A live drag-scroll follows the pointer through window
                // handlers armed in a paint pass, the scrub strips' idiom.
                // The canvas exists for that paint hook; the list's rows
                // closure can't arm them, it also runs during layout.
                .child(
                    canvas(|_, _, _| (), {
                        let flick = self.flick.clone();
                        let scroll = self.scroll.clone();
                        let weak = cx.entity().downgrade();
                        move |_, _, window, _| {
                            let scroll = scroll.clone();
                            let weak = weak.clone();
                            panel::flick_on_paint(&flick, window, move |dy, cx| {
                                let base = scroll.0.borrow().base_handle.clone();
                                let mut offset = base.offset();
                                offset.y += px(dy);
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
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&self.scroll)),
                )
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
                        let menu = menu.item(
                            PopupMenuItem::new(label)
                                .icon(Icon::default().path(icons::PLAY))
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = panel.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.play_many(ixs.clone(), cx)
                                        });
                                    }
                                }),
                        );
                        // The primary editing flow: the selection into the
                        // tag editor window.
                        let state = this.read(cx).state.clone();
                        let reveal = ids.first().copied();
                        let menu = menu.item(
                            PopupMenuItem::new("Edit Tags...")
                                .icon(Icon::default().path(icons::PENCIL))
                                .on_click(move |_, _, cx| {
                                    crate::tag_editor::open(state.clone(), ids.clone(), cx);
                                }),
                        );
                        // Reveal follows the first album's first track,
                        // landing in that album's folder.
                        let menu =
                            panel::reveal_item(menu, this.read(cx).state.clone(), reveal);
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
