//! The album grid panel: the catalog as a wall of cover tiles, NekoRoX's
//! grid gallery. One tile per album in the library's canonical order,
//! square, the last column bleeding off the panel edge like the reference
//! theme's grid, textures through the shared artwork service.
//! Rows virtualize through a uniform_list, so a huge library costs only
//! the tiles on screen. Clicking a tile publishes the album's tracks on
//! the shared selection; a double click queues the album on the player.
//! A per-view query narrows the wall to albums containing a matching
//! track, so two duplicates scope to different filters. Deliberately not
//! the library's table: per the workspace rule, browsing surfaces are
//! panels of their own, never library view modes.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, img, prelude::*, px, svg, uniform_list, AnyElement, App, Context, Div, Entity,
    EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseUpEvent, ObjectFit,
    Pixels, SharedString, Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
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

fn default_tile() -> f32 {
    192.
}

/// The grid panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct GridConfig {
    #[serde(default)]
    pub query: String,
    /// The preferred tile edge in px, within [`TILE_MIN`]..[`TILE_MAX`].
    #[serde(default = "default_tile")]
    pub tile: f32,
    /// Scroll to the playing album when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// The panel's palette override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            query: String::new(),
            tile: default_tile(),
            follow_playing: false,
            smooth_follow: false,
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
    /// The clicked album, the accent outline and the published selection.
    selected: Option<usize>,
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
            |this: &mut Self, _, _: &LibraryEvent, cx| {
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
            selected: None,
            hovered: None,
            width: px(0.),
            scroll: UniformListScrollHandle::new(),
            flick: FlickState::default(),
            glide_to: None,
            last_tick: Instant::now(),
            playing_path: None,
            tile_scrub: ScrubState::default(),
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

    /// Scroll the playing track's album into view: a glide when smooth is
    /// on, a centered jump otherwise.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let row = {
            let Some(path) = &self.playing_path else {
                return;
            };
            let library = self.state.library.read(cx);
            let Some(id) = library.id_for(path) else {
                return;
            };
            let Some(projection) = library.projection() else {
                return;
            };
            let Some(view_ix) = self
                .view
                .iter()
                .position(|&row| projection.db_id[row as usize] == id)
            else {
                return;
            };
            // Cells are contiguous runs over the view; the last one
            // starting at or before the hit holds it.
            let cell_ix = self
                .cells
                .partition_point(|cell| cell.start <= view_ix)
                .saturating_sub(1);
            cell_ix / self.cols()
        };
        // Both modes head for the same row through the per-frame stepping
        // in `body`: the row is the stable fact, its offset depends on a
        // layout that may still be settling (a launch's first frames), so
        // even the jump re-pins until the target holds still.
        self.glide_to = Some(row);
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
        self.selected = None;
        self.hovered = None;
        self.view = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) if !self.config.query.is_empty() => {
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

    /// Publish the clicked album's tracks on the shared selection.
    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        let ids = self.ids_for(ix, cx);
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, cx));
        cx.notify();
    }

    /// Queue the album on the shared player.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids = self.ids_for(ix, cx);
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

    /// How many tiles share a row at the current width: enough to cover
    /// it, the last one bleeding off the edge like the reference theme's
    /// grid. The ceil is what keeps the size knob continuous - tiles
    /// render at exactly the configured edge, no snapping to fit.
    fn cols(&self) -> usize {
        let width = f32::from(self.width);
        if width <= 0. {
            return FALLBACK_COLS;
        }
        ((width / self.config.tile).ceil() as usize).max(1)
    }

    /// A tile's edge: exactly the configured size.
    fn tile_side(&self) -> Pixels {
        px(self.config.tile)
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
        let content: AnyElement = match thumb {
            Thumb::Ready(image) => img(image)
                .size_full()
                .object_fit(ObjectFit::Cover)
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
                        this.select(ix, cx);
                    }
                }),
            )
            .child(content)
            .when(self.hovered == Some(ix), |d| d.child(self.label(ix, cx)))
            .when(self.selected == Some(ix), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
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
        range
            .map(|r| {
                let mut row = div().flex().flex_row();
                for ix in (r * cols)..((r + 1) * cols).min(self.cells.len()) {
                    row = row.child(self.tile(ix, side, cx));
                }
                row
            })
            .collect()
    }
}

impl PanelSettings for GridPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Content", "Behavior"]
    }

    fn page(
        &mut self,
        page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if page == "Behavior" {
            return div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
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
                .into_any_element();
        }
        let fraction = ((self.config.tile - TILE_MIN) / (TILE_MAX - TILE_MIN)).clamp(0., 1.);
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                "tile size",
                Some("the cover tiles' exact edge; the last column runs off the panel side"),
                settings_ui::slider_labeled(
                    &self.tile_scrub,
                    fraction,
                    format!("{}px", self.config.tile.round() as u32),
                    |this: &mut Self, fraction, cx| {
                        this.config.tile = TILE_MIN + fraction * (TILE_MAX - TILE_MIN);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
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
        SharedString::from("album grid")
    }

    /// The search box shares the title bar row, the library's move.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        Some(
            self.search
                .update(cx, |search, cx| search.element(cx))
                .w(px(180.)),
        )
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
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
        panel::themed(&theme, || self.body(window, cx).into_any_element())
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

        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(palette::bg_root())
            .track_focus(&self.focus);
        let content: AnyElement = if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(if self.config.query.is_empty() {
                    "the library is empty"
                } else {
                    "no matches"
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
